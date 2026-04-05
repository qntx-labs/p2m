//! PDF content-stream operator state machine.
//!
//! Walks the page's content stream, tracking the graphics state and text
//! matrix, and emits [`TextItem`]s, [`Rect`]s, and [`Line`]s for each page.
//!
//! This is the core extraction engine: it handles every PDF drawing operator
//! relevant to text and table-boundary detection, including font selection,
//! coordinate transforms, path construction, marked content, and colour
//! state (for invisible-text filtering).

use std::collections::HashMap;

use log::trace;
use lopdf::{Document, Object, ObjectId};

use crate::error::{Error, Result};
use crate::pdf::fonts::{
    build_font_encodings, build_font_widths, compute_string_width_ts, extract_text_from_operand,
    get_font_file2_obj_num, get_operand_bytes, CMapDecisionCache,
};
use crate::pdf::tounicode::FontCMaps;
use crate::text::unicode::{
    decode_text_string, effective_font_size, expand_ligatures, is_bold_font, is_italic_font,
};
use crate::types::{
    FontWidthInfo, ItemKind, Line, PageExtraction, PageFontEncodings, PageNum, Rect, TextItem,
};

// ── Constants ──────────────────────────────────────────────────────

/// Maximum number of content-stream operations to process per page.
/// Pages exceeding this are skipped to avoid excessive CPU usage on
/// pathological PDFs (e.g. vector art with millions of path ops).
const MAX_OPERATIONS: usize = 1_000_000;

// ── Public API ─────────────────────────────────────────────────────

/// Extract text items, rectangles, and lines from a single page's
/// content stream.
///
/// Returns `(page_extraction, has_gid_fonts, coords_rotated)` where:
/// - `page_extraction` is `(Vec<TextItem>, Vec<Rect>, Vec<Line>)`
/// - `has_gid_fonts` indicates fonts with unresolvable glyph-ID encodings
/// - `coords_rotated` indicates the page text was predominantly rotated
///   90° and coordinates were swapped to compensate
///
/// # Errors
///
/// Returns [`Error::Parse`] when the content stream cannot be decoded.
#[allow(clippy::too_many_lines)]
pub(crate) fn extract_page_text_items(
    doc: &Document,
    page_id: ObjectId,
    page_num: PageNum,
    font_cmaps: &FontCMaps,
    include_invisible: bool,
) -> Result<(PageExtraction, bool, bool)> {
    use lopdf::content::Content;

    let mut items: Vec<TextItem> = Vec::new();
    let mut rects: Vec<Rect> = Vec::new();
    let mut clip_rects: Vec<Rect> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();

    // ── Path construction state ────────────────────────────────
    let mut path_subpath_start: Option<(f32, f32)> = None;
    let mut path_current: Option<(f32, f32)> = None;
    let mut pending_lines: Vec<(f32, f32, f32, f32)> = Vec::new();
    let mut pending_subpaths: Vec<Vec<(f32, f32, f32, f32)>> = Vec::new();
    let mut fill_rects: Vec<Rect> = Vec::new();

    // ── Font setup ─────────────────────────────────────────────
    let fonts = doc.get_page_fonts(page_id).unwrap_or_default();
    let (font_encodings, has_gid_fonts) = build_font_encodings(doc, &fonts);
    let font_widths = build_font_widths(doc, &fonts);

    // Build maps of font resource names to base font names and
    // `ToUnicode` object references.
    let mut font_base_names: HashMap<String, String> = HashMap::new();
    let mut font_tounicode_refs: HashMap<String, u32> = HashMap::new();
    let mut inline_cmaps: HashMap<String, crate::pdf::tounicode::CMapEntry> = HashMap::new();

    for (font_name, font_dict) in &fonts {
        let resource_name = String::from_utf8_lossy(font_name).to_string();
        if let Ok(base_font) = font_dict.get(b"BaseFont") {
            if let Ok(name) = base_font.as_name() {
                let base_name = String::from_utf8_lossy(name).to_string();
                font_base_names.insert(resource_name.clone(), base_name);
            }
        }
        match font_dict.get(b"ToUnicode") {
            Ok(tounicode) => {
                if let Ok(obj_ref) = tounicode.as_reference() {
                    font_tounicode_refs.insert(resource_name, obj_ref.0);
                } else if let Object::Stream(s) = tounicode {
                    let data = s
                        .decompressed_content()
                        .unwrap_or_else(|_| s.content.clone());
                    if let Some(entry) =
                        crate::pdf::tounicode::build_cmap_entry_from_stream(&data, font_dict, doc, 0)
                    {
                        inline_cmaps.insert(resource_name, entry);
                    }
                }
            }
            Err(_) => {
                if let Some(ff2_obj_num) = get_font_file2_obj_num(doc, font_dict) {
                    font_tounicode_refs.insert(resource_name, ff2_obj_num);
                }
            }
        }
    }

    // Cache lopdf font encodings once per font.
    let mut encoding_cache: HashMap<String, lopdf::Encoding<'_>> = HashMap::new();
    for (font_name, font_dict) in &fonts {
        let name = String::from_utf8_lossy(font_name).to_string();
        if let Ok(enc) = font_dict.get_font_encoding(doc) {
            encoding_cache.insert(name, enc);
        }
    }

    let mut cmap_decisions = CMapDecisionCache::new();

    // ── XObject map ────────────────────────────────────────────
    let xobjects = get_page_xobjects(doc, page_id);

    // ── Content stream loading ─────────────────────────────────
    let content_data = doc
        .get_page_content(page_id)
        .map_err(|e| Error::Parse(e.to_string()))?;
    let content_data = strip_pdf_comments(&content_data);
    let content = Content::decode(&content_data).map_err(|e| Error::Parse(e.to_string()))?;

    if content.operations.len() > MAX_OPERATIONS {
        log::warn!(
            "page {page_num}: skipping extraction — {} operations exceeds limit ({MAX_OPERATIONS})",
            content.operations.len(),
        );
        return Ok(((Vec::new(), Vec::new(), Vec::new()), false, false));
    }

    // ── Graphics state ─────────────────────────────────────────
    let mut ctm = [1.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut text_rendering_mode: i32 = 0;
    let mut gstate_stack: Vec<GraphicsState> = Vec::new();

    // ── Text state ─────────────────────────────────────────────
    let mut current_font = String::new();
    let mut current_font_size: f32 = 12.0;
    let mut text_leading: f32 = 0.0;
    let mut char_spacing: f32 = 0.0;
    let mut word_spacing: f32 = 0.0;
    let mut text_matrix = [1.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut line_matrix = [1.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0];
    let mut in_text_block = false;

    // ── Fill colour state (invisible-text detection) ───────────
    let mut fill_is_white = false;

    // ── Rotation detection ─────────────────────────────────────
    let mut rotation_votes = RotationVotes {
        horizontal: 0,
        rotated: 0,
    };

    // ── Marked content tracking ────────────────────────────────
    let mut marked_content_stack: Vec<MarkedContentEntry> = Vec::new();
    let mut suppress_glyph_extraction = false;
    let mut actual_text_start_tm: Option<[f32; 6]> = None;

    // ── Main operator loop ─────────────────────────────────────
    for op in &content.operations {
        trace!("{} {:?}", op.operator, op.operands);
        match op.operator.as_str() {
            // ── Graphics state save / restore ──────────────────
            "q" => {
                gstate_stack.push(GraphicsState {
                    ctm,
                    text_rendering_mode,
                    char_spacing,
                    word_spacing,
                });
            }
            "Q" => {
                if let Some(saved) = gstate_stack.pop() {
                    ctm = saved.ctm;
                    text_rendering_mode = saved.text_rendering_mode;
                    char_spacing = saved.char_spacing;
                    word_spacing = saved.word_spacing;
                }
            }

            // ── CTM concatenation ──────────────────────────────
            "cm" => {
                if op.operands.len() >= 6 {
                    let new_matrix = [
                        get_number(&op.operands[0]).unwrap_or(1.0),
                        get_number(&op.operands[1]).unwrap_or(0.0),
                        get_number(&op.operands[2]).unwrap_or(0.0),
                        get_number(&op.operands[3]).unwrap_or(1.0),
                        get_number(&op.operands[4]).unwrap_or(0.0),
                        get_number(&op.operands[5]).unwrap_or(0.0),
                    ];
                    ctm = multiply_matrices(&new_matrix, &ctm);
                }
            }

            // ── Text block begin / end ─────────────────────────
            "BT" => {
                in_text_block = true;
                text_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
                line_matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
                text_rendering_mode = 0;
            }
            "ET" => {
                in_text_block = false;
            }

            // ── Font selection ─────────────────────────────────
            "Tf" => {
                if op.operands.len() >= 2 {
                    if let Ok(name) = op.operands[0].as_name() {
                        current_font = String::from_utf8_lossy(name).to_string();
                    }
                    if let Ok(size) = op.operands[1].as_f32() {
                        current_font_size = size;
                    } else if let Ok(size) = op.operands[1].as_i64() {
                        #[allow(clippy::cast_precision_loss)]
                        {
                            current_font_size = size as f32;
                        }
                    }
                }
            }

            // ── Text leading ───────────────────────────────────
            "TL" => {
                if let Some(tl) = op.operands.first().and_then(get_number) {
                    text_leading = tl;
                }
            }

            // ── Text rendering mode ────────────────────────────
            "Tr" => {
                if let Some(mode) = op.operands.first().and_then(get_number) {
                    #[allow(clippy::cast_possible_truncation)]
                    {
                        text_rendering_mode = mode as i32;
                    }
                }
            }

            // ── Character spacing ──────────────────────────────
            "Tc" => {
                if let Some(tc) = op.operands.first().and_then(get_number) {
                    char_spacing = tc;
                }
            }

            // ── Word spacing ───────────────────────────────────
            "Tw" => {
                if let Some(tw) = op.operands.first().and_then(get_number) {
                    word_spacing = tw;
                }
            }

            // ── Text position offset ───────────────────────────
            "Td" | "TD" => {
                if op.operands.len() >= 2 {
                    let tx = get_number(&op.operands[0]).unwrap_or(0.0);
                    let ty = get_number(&op.operands[1]).unwrap_or(0.0);
                    line_matrix[4] += tx * line_matrix[0] + ty * line_matrix[2];
                    line_matrix[5] += tx * line_matrix[1] + ty * line_matrix[3];
                    text_matrix = line_matrix;
                    if op.operator == "TD" {
                        text_leading = -ty;
                    }
                }
            }

            // ── Set text matrix directly ───────────────────────
            "Tm" => {
                if op.operands.len() >= 6 {
                    for (i, operand) in op.operands.iter().take(6).enumerate() {
                        text_matrix[i] =
                            get_number(operand).unwrap_or(if i == 0 || i == 3 { 1.0 } else { 0.0 });
                    }
                    line_matrix = text_matrix;
                }
            }

            // ── Next line ──────────────────────────────────────
            "T*" => {
                let tl = if text_leading != 0.0 {
                    text_leading
                } else {
                    current_font_size * 1.2
                };
                line_matrix[4] += (-tl) * line_matrix[2];
                line_matrix[5] += (-tl) * line_matrix[3];
                text_matrix = line_matrix;
            }

            // ── Show text string ───────────────────────────────
            "Tj" => {
                if in_text_block && !op.operands.is_empty() {
                    handle_tj(
                        &op.operands[0],
                        &mut text_matrix,
                        &ctm,
                        &current_font,
                        current_font_size,
                        char_spacing,
                        word_spacing,
                        text_rendering_mode,
                        include_invisible,
                        suppress_glyph_extraction,
                        fill_is_white,
                        page_num,
                        &font_widths,
                        &font_base_names,
                        font_cmaps,
                        &font_tounicode_refs,
                        &inline_cmaps,
                        &font_encodings,
                        &encoding_cache,
                        &mut cmap_decisions,
                        &marked_content_stack,
                        &mut rotation_votes,
                        &mut items,
                    );
                }
            }

            // ── Show text array with positioning ───────────────
            "TJ" => {
                if in_text_block && !op.operands.is_empty() {
                    handle_tj_array(
                        &op.operands[0],
                        &mut text_matrix,
                        &ctm,
                        &current_font,
                        current_font_size,
                        char_spacing,
                        word_spacing,
                        text_rendering_mode,
                        include_invisible,
                        suppress_glyph_extraction,
                        fill_is_white,
                        page_num,
                        &font_widths,
                        &font_base_names,
                        font_cmaps,
                        &font_tounicode_refs,
                        &inline_cmaps,
                        &font_encodings,
                        &encoding_cache,
                        &mut cmap_decisions,
                        &marked_content_stack,
                        &mut rotation_votes,
                        &mut items,
                    );
                }
            }

            // ── Next line + show text ──────────────────────────
            "'" => {
                // Equivalent to T* then Tj.
                let tl = if text_leading != 0.0 {
                    text_leading
                } else {
                    current_font_size * 1.2
                };
                line_matrix[4] += (-tl) * line_matrix[2];
                line_matrix[5] += (-tl) * line_matrix[3];
                text_matrix = line_matrix;

                if !((text_rendering_mode == 3 && !include_invisible)
                    || suppress_glyph_extraction
                    || op.operands.is_empty())
                {
                    handle_show_text_simple(
                        &op.operands[0],
                        &text_matrix,
                        &ctm,
                        &current_font,
                        current_font_size,
                        page_num,
                        &font_base_names,
                        font_cmaps,
                        &font_tounicode_refs,
                        &inline_cmaps,
                        &font_encodings,
                        &encoding_cache,
                        &mut cmap_decisions,
                        &marked_content_stack,
                        &mut rotation_votes,
                        &mut items,
                    );
                }
            }

            // ── Set spacing + next line + show text ────────────
            "\"" => {
                // " aw ac string: set Tw, Tc, then T* Tj
                if op.operands.len() >= 3 {
                    if let Some(tw) = get_number(&op.operands[0]) {
                        word_spacing = tw;
                    }
                    if let Some(tc) = get_number(&op.operands[1]) {
                        char_spacing = tc;
                    }

                    let tl = if text_leading != 0.0 {
                        text_leading
                    } else {
                        current_font_size * 1.2
                    };
                    line_matrix[4] += (-tl) * line_matrix[2];
                    line_matrix[5] += (-tl) * line_matrix[3];
                    text_matrix = line_matrix;

                    if !((text_rendering_mode == 3 && !include_invisible)
                        || suppress_glyph_extraction)
                    {
                        handle_show_text_simple(
                            &op.operands[2],
                            &text_matrix,
                            &ctm,
                            &current_font,
                            current_font_size,
                            page_num,
                            &font_base_names,
                            font_cmaps,
                            &font_tounicode_refs,
                            &inline_cmaps,
                            &font_encodings,
                            &encoding_cache,
                            &mut cmap_decisions,
                            &marked_content_stack,
                            &mut rotation_votes,
                            &mut items,
                        );
                    }
                }
            }

            // ── Rectangle operator ─────────────────────────────
            "re" => {
                if op.operands.len() >= 4 {
                    let rx = get_number(&op.operands[0]).unwrap_or(0.0);
                    let ry = get_number(&op.operands[1]).unwrap_or(0.0);
                    let rw = get_number(&op.operands[2]).unwrap_or(0.0);
                    let rh = get_number(&op.operands[3]).unwrap_or(0.0);
                    let x_dev = rx * ctm[0] + ry * ctm[2] + ctm[4];
                    let y_dev = rx * ctm[1] + ry * ctm[3] + ctm[5];
                    let w_dev = rw * ctm[0];
                    let h_dev = rh * ctm[3];
                    rects.push(Rect {
                        x: x_dev,
                        y: y_dev,
                        width: w_dev,
                        height: h_dev,
                        page: page_num,
                    });
                }
            }

            // ── Path construction: moveto ──────────────────────
            "m" => {
                if op.operands.len() >= 2 {
                    let px = get_number(&op.operands[0]).unwrap_or(0.0);
                    let py = get_number(&op.operands[1]).unwrap_or(0.0);
                    path_subpath_start = Some((px, py));
                    path_current = Some((px, py));
                }
            }

            // ── Path construction: lineto ──────────────────────
            "l" => {
                if op.operands.len() >= 2 {
                    if let Some((cx, cy)) = path_current {
                        let px = get_number(&op.operands[0]).unwrap_or(0.0);
                        let py = get_number(&op.operands[1]).unwrap_or(0.0);
                        pending_lines.push((cx, cy, px, py));
                        path_current = Some((px, py));
                    }
                }
            }

            // ── Path construction: closepath ───────────────────
            "h" => {
                if let (Some((cx, cy)), Some((sx, sy))) = (path_current, path_subpath_start) {
                    if (cx - sx).abs() > 0.01 || (cy - sy).abs() > 0.01 {
                        pending_lines.push((cx, cy, sx, sy));
                    }
                    path_current = path_subpath_start;
                }
                if !pending_lines.is_empty() {
                    pending_subpaths.push(std::mem::take(&mut pending_lines));
                }
            }

            // ── Stroke / close-and-stroke ──────────────────────
            "S" | "s" => {
                if op.operator == "s" {
                    if let (Some((cx, cy)), Some((sx, sy))) = (path_current, path_subpath_start) {
                        if (cx - sx).abs() > 0.01 || (cy - sy).abs() > 0.01 {
                            pending_lines.push((cx, cy, sx, sy));
                        }
                    }
                }
                emit_stroked_lines(&ctm, &mut pending_lines, page_num, &mut lines);
                pending_subpaths.clear();
                path_subpath_start = None;
                path_current = None;
            }

            // ── Fill + stroke ──────────────────────────────────
            "B" | "B*" | "b" | "b*" => {
                if op.operator == "b" || op.operator == "b*" {
                    if let (Some((cx, cy)), Some((sx, sy))) = (path_current, path_subpath_start) {
                        if (cx - sx).abs() > 0.01 || (cy - sy).abs() > 0.01 {
                            pending_lines.push((cx, cy, sx, sy));
                        }
                    }
                }
                emit_stroked_lines(&ctm, &mut pending_lines, page_num, &mut lines);
                pending_subpaths.clear();
                path_subpath_start = None;
                path_current = None;
            }

            // ── Fill only ──────────────────────────────────────
            "f" | "F" | "f*" => {
                if !pending_lines.is_empty() {
                    pending_subpaths.push(std::mem::take(&mut pending_lines));
                }
                extract_fill_rects(
                    &ctm,
                    &mut pending_subpaths,
                    page_num,
                    &mut fill_rects,
                );
                pending_lines.clear();
                path_subpath_start = None;
                path_current = None;
            }

            // ── Clip operator ──────────────────────────────────
            "W" | "W*" => {
                let segs: Vec<(f32, f32, f32, f32)> = if pending_lines.is_empty() {
                    pending_subpaths.last().cloned().unwrap_or_default()
                } else {
                    pending_lines.clone()
                };
                if let Some(r) = try_extract_clip_rect(&segs, path_subpath_start, &ctm, page_num)
                {
                    clip_rects.push(r);
                }
                // Do NOT clear pending_lines — the following `n` does that.
            }

            // ── End path (no-op paint) ─────────────────────────
            "n" => {
                pending_lines.clear();
                pending_subpaths.clear();
                path_subpath_start = None;
                path_current = None;
            }

            // ── XObject invocation ─────────────────────────────
            "Do" => {
                if !op.operands.is_empty() {
                    if let Ok(name) = op.operands[0].as_name() {
                        let xobj_name = String::from_utf8_lossy(name).to_string();
                        if let Some(xobj_type) = xobjects.get(&xobj_name) {
                            match xobj_type {
                                XObjectKind::Image => {
                                    // Emit a placeholder TextItem for image XObjects
                                    // so downstream layout knows an image occupies space.
                                    let combined = multiply_matrices(&text_matrix, &ctm);
                                    let (x, y) = (combined[4], combined[5]);
                                    items.push(TextItem {
                                        text: String::new(),
                                        x,
                                        y,
                                        width: 0.0,
                                        height: 0.0,
                                        font: String::new(),
                                        font_size: 0.0,
                                        page: page_num,
                                        bold: false,
                                        italic: false,
                                        kind: ItemKind::Image,
                                        mcid: current_mcid(&marked_content_stack),
                                    });
                                }
                                XObjectKind::Form(_form_id) => {
                                    // Form XObject text extraction is handled by
                                    // the `xobjects` module; currently a stub.
                                    // TODO: extract_form_xobject_text(doc, *form_id, ...)
                                }
                            }
                        }
                    }
                }
            }

            // ── Marked content: begin (no properties) ──────────
            "BMC" => {
                marked_content_stack.push(MarkedContentEntry {
                    actual_text: None,
                    mcid: None,
                });
            }

            // ── Marked content: begin with properties ──────────
            "BDC" => {
                let mut actual_text: Option<String> = None;
                let mut mcid: Option<i64> = None;
                if op.operands.len() >= 2 {
                    let dict = match &op.operands[1] {
                        Object::Dictionary(d) => Some(d.clone()),
                        Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
                        _ => None,
                    };
                    if let Some(d) = dict {
                        if let Ok(val) = d.get(b"ActualText") {
                            actual_text = match val {
                                Object::String(bytes, _) => Some(decode_text_string(bytes)),
                                _ => None,
                            };
                        }
                        if let Ok(Object::Integer(id)) = d.get(b"MCID") {
                            mcid = Some(*id);
                        }
                    }
                }
                if actual_text.is_some() {
                    suppress_glyph_extraction = true;
                    actual_text_start_tm = Some(text_matrix);
                }
                marked_content_stack.push(MarkedContentEntry { actual_text, mcid });
            }

            // ── Marked content: end ────────────────────────────
            "EMC" => {
                if let Some(entry) = marked_content_stack.pop() {
                    if let Some(at) = entry.actual_text {
                        if let Some(start_tm) = actual_text_start_tm.take() {
                            let combined = multiply_matrices(&start_tm, &ctm);
                            vote_rotation(&combined, &mut rotation_votes);
                            let rendered_size = effective_font_size(current_font_size, &combined);
                            let (x, y) = (combined[4], combined[5]);
                            let delta_ts = text_matrix[4] - start_tm[4];
                            let scale_x = start_tm[0] * ctm[0] + start_tm[1] * ctm[2];
                            let width = (delta_ts * scale_x).abs();
                            if !at.trim().is_empty() {
                                let base_font = font_base_names
                                    .get(&current_font)
                                    .map(String::as_str)
                                    .unwrap_or(&current_font);
                                items.push(TextItem {
                                    text: expand_ligatures(&at),
                                    x,
                                    y,
                                    width,
                                    height: rendered_size,
                                    font: current_font.clone(),
                                    font_size: rendered_size,
                                    page: page_num,
                                    bold: is_bold_font(base_font),
                                    italic: is_italic_font(base_font),
                                    kind: ItemKind::Text,
                                    mcid: entry
                                        .mcid
                                        .or_else(|| current_mcid(&marked_content_stack)),
                                });
                            }
                        }
                        suppress_glyph_extraction =
                            marked_content_stack.iter().any(|e| e.actual_text.is_some());
                    }
                }
            }

            // ── Fill colour operators (detect white/invisible) ─
            "g" => {
                if let Some(gray) = op.operands.first().and_then(get_number) {
                    fill_is_white = gray > 0.95;
                }
            }
            "rg" => {
                if op.operands.len() >= 3 {
                    let r = get_number(&op.operands[0]).unwrap_or(0.0);
                    let g = get_number(&op.operands[1]).unwrap_or(0.0);
                    let b = get_number(&op.operands[2]).unwrap_or(0.0);
                    fill_is_white = r > 0.95 && g > 0.95 && b > 0.95;
                }
            }
            "k" => {
                if op.operands.len() >= 4 {
                    let c = get_number(&op.operands[0]).unwrap_or(1.0);
                    let m_val = get_number(&op.operands[1]).unwrap_or(1.0);
                    let y_val = get_number(&op.operands[2]).unwrap_or(1.0);
                    let k_val = get_number(&op.operands[3]).unwrap_or(1.0);
                    fill_is_white = c < 0.05 && m_val < 0.05 && y_val < 0.05 && k_val < 0.05;
                }
            }
            "sc" | "scn" => {
                let nums: Vec<f32> = op.operands.iter().filter_map(get_number).collect();
                match nums.len() {
                    3 => {
                        fill_is_white = nums[0] > 0.95 && nums[1] > 0.95 && nums[2] > 0.95;
                    }
                    4 => {
                        fill_is_white =
                            nums[0] < 0.05 && nums[1] < 0.05 && nums[2] < 0.05 && nums[3] < 0.05;
                    }
                    _ => fill_is_white = false,
                }
            }
            // Stroke colour operators — we only track fill for invisibility.
            "cs" | "G" | "RG" | "K" => {}

            _ => {}
        }
    }

    // ── Fallback rect sources ──────────────────────────────────
    // Only use clip/fill rects when no `re` rects exist on this page.
    if rects.is_empty() {
        dedup_rects(&mut clip_rects);
        if clip_rects.len() >= 4 {
            rects = clip_rects;
        } else if !fill_rects.is_empty() {
            rects = fill_rects;
        } else if !clip_rects.is_empty() {
            rects = clip_rects;
        }
    }

    // ── Rotation correction ────────────────────────────────────
    let (items, rects, lines, coords_rotated) =
        correct_rotated_page(items, rects, lines, &rotation_votes);

    Ok(((items, rects, lines), has_gid_fonts, coords_rotated))
}

// ── Internal types ─────────────────────────────────────────────────

/// Saved graphics state for q/Q nesting.
#[derive(Debug, Clone, Copy)]
struct GraphicsState {
    /// Current transformation matrix.
    ctm: [f32; 6],
    /// Text rendering mode (0=fill, 1=stroke, 2=both, 3=invisible).
    text_rendering_mode: i32,
    /// Extra spacing per character (Tc).
    char_spacing: f32,
    /// Extra spacing per space character (Tw).
    word_spacing: f32,
}

/// Marked content stack entry for `BDC`/`BMC`/`EMC` tracking.
struct MarkedContentEntry {
    /// `ActualText` replacement string from `BDC` properties.
    actual_text: Option<String>,
    /// Marked Content ID for structure-tree linking.
    mcid: Option<i64>,
}

/// Counts of text operators with horizontal vs. rotated combined matrices.
struct RotationVotes {
    /// Text operators where `|combined[0]| >= |combined[1]|` (horizontal).
    horizontal: u32,
    /// Text operators where `|combined[1]| > |combined[0]|` (rotated 90°).
    rotated: u32,
}

/// Categorised XObject from page resources.
enum XObjectKind {
    /// Raster image.
    Image,
    /// Form XObject (reusable content stream) with its object ID.
    Form(ObjectId),
}

// ── Operator handlers ──────────────────────────────────────────────

/// Handle the `Tj` (show text string) operator.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn handle_tj(
    operand: &Object,
    text_matrix: &mut [f32; 6],
    ctm: &[f32; 6],
    current_font: &str,
    current_font_size: f32,
    char_spacing: f32,
    word_spacing: f32,
    text_rendering_mode: i32,
    include_invisible: bool,
    suppress_glyph_extraction: bool,
    fill_is_white: bool,
    page_num: PageNum,
    font_widths: &HashMap<String, FontWidthInfo>,
    font_base_names: &HashMap<String, String>,
    font_cmaps: &FontCMaps,
    font_tounicode_refs: &HashMap<String, u32>,
    inline_cmaps: &HashMap<String, crate::pdf::tounicode::CMapEntry>,
    font_encodings: &PageFontEncodings,
    encoding_cache: &HashMap<String, lopdf::Encoding<'_>>,
    cmap_decisions: &mut CMapDecisionCache,
    marked_content_stack: &[MarkedContentEntry],
    rotation_votes: &mut RotationVotes,
    items: &mut Vec<TextItem>,
) {
    // Compute string width regardless of visibility — we always advance
    // the text matrix so subsequent positioning is correct.
    let w_ts_opt = font_widths.get(current_font).and_then(|fi| {
        get_operand_bytes(operand).map(|raw| {
            compute_string_width_ts(raw, fi, current_font_size, char_spacing, word_spacing)
        })
    });

    // `ActualText` suppression: advance matrix only.
    if suppress_glyph_extraction {
        if let Some(w_ts) = w_ts_opt {
            text_matrix[4] += w_ts * text_matrix[0];
            text_matrix[5] += w_ts * text_matrix[1];
        }
        return;
    }

    // Invisible text: advance matrix, skip item emission.
    let is_invisible = (text_rendering_mode == 3 && !include_invisible) || fill_is_white;
    if is_invisible {
        if let Some(w_ts) = w_ts_opt {
            text_matrix[4] += w_ts * text_matrix[0];
            text_matrix[5] += w_ts * text_matrix[1];
        }
        return;
    }

    if let Some(text) = extract_text_from_operand(
        operand,
        current_font,
        font_base_names.get(current_font).map(String::as_str),
        font_cmaps,
        font_tounicode_refs,
        inline_cmaps,
        font_encodings,
        encoding_cache,
        cmap_decisions,
    ) {
        let combined = multiply_matrices(text_matrix, ctm);
        vote_rotation(&combined, rotation_votes);
        let rendered_size = effective_font_size(current_font_size, &combined);
        let (x, y) = (combined[4], combined[5]);
        let width = if let Some(w_ts) = w_ts_opt {
            text_matrix[4] += w_ts * text_matrix[0];
            text_matrix[5] += w_ts * text_matrix[1];
            (w_ts * (text_matrix[0] * ctm[0] + text_matrix[1] * ctm[2])).abs()
        } else {
            0.0
        };
        if !text.trim().is_empty() {
            let base_font = font_base_names
                .get(current_font)
                .map(String::as_str)
                .unwrap_or(current_font);
            items.push(TextItem {
                text: expand_ligatures(&text),
                x,
                y,
                width,
                height: rendered_size,
                font: current_font.to_owned(),
                font_size: rendered_size,
                page: page_num,
                bold: is_bold_font(base_font),
                italic: is_italic_font(base_font),
                kind: ItemKind::Text,
                mcid: current_mcid(marked_content_stack),
            });
        }
    }
}

/// Handle the `TJ` (show text array with positioning) operator.
///
/// Positioning adjustments exceeding 4x the space width split the array
/// into separate [`TextItem`]s (column-boundary detection). Adjustments
/// exceeding 1x the space width insert a synthetic space character.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn handle_tj_array(
    operand: &Object,
    text_matrix: &mut [f32; 6],
    ctm: &[f32; 6],
    current_font: &str,
    current_font_size: f32,
    char_spacing: f32,
    word_spacing: f32,
    text_rendering_mode: i32,
    include_invisible: bool,
    suppress_glyph_extraction: bool,
    fill_is_white: bool,
    page_num: PageNum,
    font_widths: &HashMap<String, FontWidthInfo>,
    font_base_names: &HashMap<String, String>,
    font_cmaps: &FontCMaps,
    font_tounicode_refs: &HashMap<String, u32>,
    inline_cmaps: &HashMap<String, crate::pdf::tounicode::CMapEntry>,
    font_encodings: &PageFontEncodings,
    encoding_cache: &HashMap<String, lopdf::Encoding<'_>>,
    cmap_decisions: &mut CMapDecisionCache,
    marked_content_stack: &[MarkedContentEntry],
    rotation_votes: &mut RotationVotes,
    items: &mut Vec<TextItem>,
) {
    let Ok(array) = operand.as_array() else {
        return;
    };

    let font_info = font_widths.get(current_font);
    let is_invisible = (text_rendering_mode == 3 && !include_invisible)
        || suppress_glyph_extraction
        || fill_is_white;

    // Compute thresholds for space insertion and column-gap splitting
    // based on font metrics when available.
    let space_threshold = font_info.map_or(120.0, |fi| {
        let space_em = fi.space_width as f32 * fi.units_scale;
        (space_em * 1000.0 * 0.4).max(80.0)
    });
    let column_gap_threshold = space_threshold * 4.0;

    // Track sub-items for column-gap splitting: (text, start_width_ts, end_width_ts).
    let mut sub_items: Vec<(String, f32, f32)> = Vec::new();
    let mut current_text = String::new();
    let mut sub_start_width_ts: f32 = 0.0;
    let mut total_width_ts: f32 = 0.0;

    for element in array {
        // Handle numeric positioning adjustments.
        let n_val = match element {
            Object::Integer(n) => {
                #[allow(clippy::cast_precision_loss)]
                Some(*n as f32)
            }
            Object::Real(n) => Some(*n),
            _ => None,
        };
        if let Some(n_val) = n_val {
            let displacement = -n_val / 1000.0 * current_font_size;
            if !is_invisible && n_val < -column_gap_threshold && !current_text.is_empty() {
                // Column gap: flush current segment.
                sub_items.push((
                    std::mem::take(&mut current_text),
                    sub_start_width_ts,
                    total_width_ts,
                ));
                total_width_ts += displacement;
                sub_start_width_ts = total_width_ts;
            } else {
                total_width_ts += displacement;
                if !is_invisible
                    && n_val < -space_threshold
                    && !current_text.is_empty()
                    && !current_text.ends_with(' ')
                {
                    current_text.push(' ');
                }
            }
            continue;
        }

        // Accumulate string width.
        if let Some(fi) = font_info {
            if let Some(raw_bytes) = get_operand_bytes(element) {
                total_width_ts +=
                    compute_string_width_ts(raw_bytes, fi, current_font_size, char_spacing, word_spacing);
            }
        }

        // Decode and accumulate text.
        if !is_invisible {
            if let Some(text) = extract_text_from_operand(
                element,
                current_font,
                font_base_names.get(current_font).map(String::as_str),
                font_cmaps,
                font_tounicode_refs,
                inline_cmaps,
                font_encodings,
                encoding_cache,
                cmap_decisions,
            ) {
                current_text.push_str(&text);
            }
        }
    }

    // Flush remaining text.
    if !is_invisible && !current_text.trim().is_empty() {
        sub_items.push((current_text, sub_start_width_ts, total_width_ts));
    }

    // Emit one TextItem per sub-item.
    if !sub_items.is_empty() {
        let combined = multiply_matrices(text_matrix, ctm);
        vote_rotation(&combined, rotation_votes);
        let rendered_size = effective_font_size(current_font_size, &combined);
        let base_font = font_base_names
            .get(current_font)
            .map(String::as_str)
            .unwrap_or(current_font);
        let scale_x = text_matrix[0] * ctm[0] + text_matrix[1] * ctm[2];

        for (text, start_w, end_w) in &sub_items {
            let offset_tm = [
                text_matrix[0],
                text_matrix[1],
                text_matrix[2],
                text_matrix[3],
                text_matrix[4] + start_w * text_matrix[0],
                text_matrix[5] + start_w * text_matrix[1],
            ];
            let combined_sub = multiply_matrices(&offset_tm, ctm);
            let (x, y) = (combined_sub[4], combined_sub[5]);
            let width = if font_info.is_some() {
                ((end_w - start_w) * scale_x).abs()
            } else {
                0.0
            };
            items.push(TextItem {
                text: expand_ligatures(text),
                x,
                y,
                width,
                height: rendered_size,
                font: current_font.to_owned(),
                font_size: rendered_size,
                page: page_num,
                bold: is_bold_font(base_font),
                italic: is_italic_font(base_font),
                kind: ItemKind::Text,
                mcid: current_mcid(marked_content_stack),
            });
        }
    }

    // Always advance text matrix by total width.
    if font_info.is_some() {
        text_matrix[4] += total_width_ts * text_matrix[0];
        text_matrix[5] += total_width_ts * text_matrix[1];
    }
}

/// Handle simple text-show operators (`'`, `"`) that don't need width
/// tracking or TJ-style gap splitting.
#[allow(clippy::too_many_arguments)]
fn handle_show_text_simple(
    operand: &Object,
    text_matrix: &[f32; 6],
    ctm: &[f32; 6],
    current_font: &str,
    current_font_size: f32,
    page_num: PageNum,
    font_base_names: &HashMap<String, String>,
    font_cmaps: &FontCMaps,
    font_tounicode_refs: &HashMap<String, u32>,
    inline_cmaps: &HashMap<String, crate::pdf::tounicode::CMapEntry>,
    font_encodings: &PageFontEncodings,
    encoding_cache: &HashMap<String, lopdf::Encoding<'_>>,
    cmap_decisions: &mut CMapDecisionCache,
    marked_content_stack: &[MarkedContentEntry],
    rotation_votes: &mut RotationVotes,
    items: &mut Vec<TextItem>,
) {
    if let Some(text) = extract_text_from_operand(
        operand,
        current_font,
        font_base_names.get(current_font).map(String::as_str),
        font_cmaps,
        font_tounicode_refs,
        inline_cmaps,
        font_encodings,
        encoding_cache,
        cmap_decisions,
    ) {
        if !text.trim().is_empty() {
            let combined = multiply_matrices(text_matrix, ctm);
            vote_rotation(&combined, rotation_votes);
            let rendered_size = effective_font_size(current_font_size, &combined);
            let (x, y) = (combined[4], combined[5]);
            let base_font = font_base_names
                .get(current_font)
                .map(String::as_str)
                .unwrap_or(current_font);
            items.push(TextItem {
                text: expand_ligatures(&text),
                x,
                y,
                width: 0.0,
                height: rendered_size,
                font: current_font.to_owned(),
                font_size: rendered_size,
                page: page_num,
                bold: is_bold_font(base_font),
                italic: is_italic_font(base_font),
                kind: ItemKind::Text,
                mcid: current_mcid(marked_content_stack),
            });
        }
    }
}

// ── Path helpers ───────────────────────────────────────────────────

/// Transform and emit stroked line segments from `pending_lines`.
fn emit_stroked_lines(
    ctm: &[f32; 6],
    pending: &mut Vec<(f32, f32, f32, f32)>,
    page_num: PageNum,
    lines: &mut Vec<Line>,
) {
    for (x1, y1, x2, y2) in pending.drain(..) {
        let x1d = x1 * ctm[0] + y1 * ctm[2] + ctm[4];
        let y1d = x1 * ctm[1] + y1 * ctm[3] + ctm[5];
        let x2d = x2 * ctm[0] + y2 * ctm[2] + ctm[4];
        let y2d = x2 * ctm[1] + y2 * ctm[3] + ctm[5];
        lines.push(Line {
            x1: x1d,
            y1: y1d,
            x2: x2d,
            y2: y2d,
            page: page_num,
        });
    }
}

/// Extract axis-aligned rectangles from filled subpaths.
fn extract_fill_rects(
    ctm: &[f32; 6],
    pending_subpaths: &mut Vec<Vec<(f32, f32, f32, f32)>>,
    page_num: PageNum,
    fill_rects: &mut Vec<Rect>,
) {
    for subpath in pending_subpaths.drain(..) {
        let mut segs = subpath;
        // Synthesize closing segment if only 3 segments.
        if segs.len() == 3 {
            let (x0, y0, _, _) = segs[0];
            let (_, _, ex, ey) = segs[2];
            if (ex - x0).abs() > 0.01 || (ey - y0).abs() > 0.01 {
                segs.push((ex, ey, x0, y0));
            }
        }
        if segs.len() == 4 {
            if let Some(r) = axis_aligned_rect_from_segs(&segs, ctm, page_num) {
                fill_rects.push(r);
            }
        }
    }
}

/// Try to extract a clipping rectangle from path segments.
fn try_extract_clip_rect(
    segs: &[(f32, f32, f32, f32)],
    subpath_start: Option<(f32, f32)>,
    ctm: &[f32; 6],
    page_num: PageNum,
) -> Option<Rect> {
    let mut segs = segs.to_vec();
    if segs.len() == 3 {
        if let Some((sx, sy)) = subpath_start {
            let (_, _, ex, ey) = segs[2];
            if (ex - sx).abs() > 0.01 || (ey - sy).abs() > 0.01 {
                segs.push((ex, ey, sx, sy));
            }
        }
    }
    if segs.len() == 4 {
        axis_aligned_rect_from_segs(&segs, ctm, page_num)
    } else {
        None
    }
}

/// Check if 4 line segments form an axis-aligned rectangle and, if so,
/// return it transformed to device space.
fn axis_aligned_rect_from_segs(
    segs: &[(f32, f32, f32, f32)],
    ctm: &[f32; 6],
    page_num: PageNum,
) -> Option<Rect> {
    let mut xs = Vec::with_capacity(8);
    let mut ys = Vec::with_capacity(8);
    for &(x1, y1, x2, y2) in segs {
        xs.push(x1);
        xs.push(x2);
        ys.push(y1);
        ys.push(y2);
    }
    let min_x = xs.iter().copied().fold(f32::INFINITY, f32::min);
    let max_x = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let min_y = ys.iter().copied().fold(f32::INFINITY, f32::min);
    let max_y = ys.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let w = max_x - min_x;
    let h = max_y - min_y;

    let eps: f32 = 0.5;
    let axis_aligned = xs
        .iter()
        .all(|&x| (x - min_x).abs() < eps || (x - max_x).abs() < eps)
        && ys
            .iter()
            .all(|&y| (y - min_y).abs() < eps || (y - max_y).abs() < eps);

    if axis_aligned && w > 1.0 && h > 1.0 {
        let x_dev = min_x * ctm[0] + min_y * ctm[2] + ctm[4];
        let y_dev = min_x * ctm[1] + min_y * ctm[3] + ctm[5];
        let w_dev = w * ctm[0];
        let h_dev = h * ctm[3];
        Some(Rect {
            x: x_dev,
            y: y_dev,
            width: w_dev,
            height: h_dev,
            page: page_num,
        })
    } else {
        None
    }
}

// ── Matrix and numeric helpers ─────────────────────────────────────

/// Multiply two 2D transformation matrices.
///
/// Matrix format: `[a, b, c, d, e, f]` representing:
/// ```text
/// | a  b  0 |
/// | c  d  0 |
/// | e  f  1 |
/// ```
pub(crate) fn multiply_matrices(m1: &[f32; 6], m2: &[f32; 6]) -> [f32; 6] {
    [
        m1[0] * m2[0] + m1[1] * m2[2],
        m1[0] * m2[1] + m1[1] * m2[3],
        m1[2] * m2[0] + m1[3] * m2[2],
        m1[2] * m2[1] + m1[3] * m2[3],
        m1[4] * m2[0] + m1[5] * m2[2] + m2[4],
        m1[4] * m2[1] + m1[5] * m2[3] + m2[5],
    ]
}

/// Extract an `f32` from a PDF numeric object.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn get_number(obj: &Object) -> Option<f32> {
    match obj {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

/// Get the innermost MCID from the marked content stack.
fn current_mcid(stack: &[MarkedContentEntry]) -> Option<i64> {
    stack.iter().rev().find_map(|e| e.mcid)
}

/// Record a rotation vote based on the combined text + CTM matrix.
fn vote_rotation(combined: &[f32; 6], votes: &mut RotationVotes) {
    if combined[0].abs() >= combined[1].abs() {
        votes.horizontal += 1;
    } else {
        votes.rotated += 1;
    }
}

// ── XObject helpers ────────────────────────────────────────────────

/// Collect XObjects from page resources, categorised by type.
fn get_page_xobjects(doc: &Document, page_id: ObjectId) -> HashMap<String, XObjectKind> {
    let mut xobject_types = HashMap::new();

    let page_dict = match doc.get_dictionary(page_id) {
        Ok(d) => d,
        Err(_) => return xobject_types,
    };

    let resources = page_dict
        .get(b"Resources")
        .ok()
        .and_then(|r| {
            r.as_reference()
                .ok()
                .and_then(|obj_ref| doc.get_dictionary(obj_ref).ok())
                .or_else(|| r.as_dict().ok())
        });

    let Some(resources) = resources else {
        return xobject_types;
    };

    let xobjects_dict = resources
        .get(b"XObject")
        .ok()
        .and_then(|r| {
            r.as_reference()
                .ok()
                .and_then(|obj_ref| doc.get_dictionary(obj_ref).ok())
                .or_else(|| r.as_dict().ok())
        });

    let Some(xobjects) = xobjects_dict else {
        return xobject_types;
    };

    for (name, value) in xobjects.iter() {
        let name_str = String::from_utf8_lossy(name).to_string();
        if let Ok(obj_ref) = value.as_reference() {
            if let Ok(Object::Stream(stream)) = doc.get_object(obj_ref) {
                if let Ok(subtype) = stream.dict.get(b"Subtype") {
                    if let Ok(subtype_name) = subtype.as_name() {
                        if subtype_name == b"Image" {
                            xobject_types.insert(name_str, XObjectKind::Image);
                        } else if subtype_name == b"Form" {
                            xobject_types.insert(name_str, XObjectKind::Form(obj_ref));
                        }
                    }
                }
            }
        }
    }

    xobject_types
}

// ── PDF comment stripping ──────────────────────────────────────────

/// Strip PDF comments (`%` to end of line) from content-stream bytes.
///
/// Some PDF generators (e.g. PD4ML) embed comments in content streams
/// that confuse `lopdf`'s `Content::decode` parser. Comments inside
/// string literals (parentheses) are NOT stripped — only top-level
/// comments.
fn strip_pdf_comments(data: &[u8]) -> Vec<u8> {
    // Quick check: if no '%' present, return as-is (common case).
    if !data.contains(&b'%') {
        return data.to_vec();
    }

    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;
    let mut in_string: i32 = 0; // parenthesis nesting depth
    let mut in_hex_string = false;

    while i < data.len() {
        let b = data[i];
        match b {
            b'(' if !in_hex_string => {
                in_string += 1;
                result.push(b);
            }
            b')' if !in_hex_string && in_string > 0 => {
                in_string -= 1;
                result.push(b);
            }
            b'<' if in_string == 0 && !in_hex_string => {
                in_hex_string = true;
                result.push(b);
            }
            b'>' if in_hex_string => {
                in_hex_string = false;
                result.push(b);
            }
            b'%' if in_string == 0 && !in_hex_string => {
                // Skip until end of line.
                while i < data.len() && data[i] != b'\n' && data[i] != b'\r' {
                    i += 1;
                }
                // Replace comment with a space to preserve token separation.
                result.push(b' ');
                continue; // Don't increment i again.
            }
            _ => {
                result.push(b);
            }
        }
        i += 1;
    }

    result
}

// ── Rectangle deduplication ────────────────────────────────────────

/// Remove near-duplicate rects (same coordinates within 0.5 pt tolerance).
///
/// Some PDFs emit a full-page clip path for every text block, producing
/// thousands of identical rects. After dedup these collapse to one rect,
/// which is too few for table detection and gets naturally skipped.
#[allow(clippy::cast_possible_truncation)]
fn dedup_rects(rects: &mut Vec<Rect>) {
    if rects.len() <= 1 {
        return;
    }
    // Round to 0.5-pt grid for tolerance, then sort and dedup.
    rects.sort_by(|a, b| {
        let ak = (
            a.page.get(),
            (a.x * 2.0) as i32,
            (a.y * 2.0) as i32,
            (a.width * 2.0) as i32,
            (a.height * 2.0) as i32,
        );
        let bk = (
            b.page.get(),
            (b.x * 2.0) as i32,
            (b.y * 2.0) as i32,
            (b.width * 2.0) as i32,
            (b.height * 2.0) as i32,
        );
        ak.cmp(&bk)
    });
    rects.dedup_by(|a, b| {
        a.page == b.page
            && (a.x - b.x).abs() < 0.5
            && (a.y - b.y).abs() < 0.5
            && (a.width - b.width).abs() < 0.5
            && (a.height - b.height).abs() < 0.5
    });
}

// ── Rotation detection and correction ──────────────────────────────

/// Detect if most text items on a page are rotated 90° or 270°, and if so,
/// swap x/y coordinates (plus widths/heights) so the layout engine sees
/// them as horizontal text on a landscape page.
#[allow(clippy::cast_precision_loss)]
fn correct_rotated_page(
    mut items: Vec<TextItem>,
    mut rects: Vec<Rect>,
    mut lines: Vec<Line>,
    votes: &RotationVotes,
) -> (Vec<TextItem>, Vec<Rect>, Vec<Line>, bool) {
    if items.len() < 2 {
        return (items, rects, lines, false);
    }

    let total_votes = votes.horizontal + votes.rotated;
    if total_votes == 0 || votes.rotated * 3 < total_votes * 2 {
        // Less than ~67% of text operators are rotated — not a rotated page.
        return (items, rects, lines, false);
    }

    log::debug!(
        "detected rotated page text: {}/{} text ops are rotated — swapping coordinates",
        votes.rotated,
        total_votes
    );

    // For 90° CCW rotation (Tm = [0, b, -b, 0, tx, ty]):
    //   device x increases = visual "down"   → negate when mapping to y
    //   device y increases = visual "right"   → use directly as x
    for item in &mut items {
        let new_x = item.y;
        let new_y = -item.x;
        item.x = new_x;
        item.y = new_y;
        // Estimate width for rotated text where width computation yielded ~0.
        if item.width < 0.5 {
            let char_count = item.text.chars().count() as f32;
            item.width = char_count * item.font_size * 0.5;
        }
    }

    for rect in &mut rects {
        let new_x = rect.y;
        let new_y = -(rect.x + rect.width.abs());
        rect.x = new_x;
        rect.y = new_y;
        std::mem::swap(&mut rect.width, &mut rect.height);
    }

    for line in &mut lines {
        let new_x1 = line.y1;
        let new_y1 = -line.x1;
        let new_x2 = line.y2;
        let new_y2 = -line.x2;
        line.x1 = new_x1;
        line.y1 = new_y1;
        line.x2 = new_x2;
        line.y2 = new_y2;
    }

    (items, rects, lines, true)
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32, page: PageNum) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
            page,
        }
    }

    #[test]
    fn test_dedup_rects_identical() {
        let p = PageNum::new(1);
        let mut rects = vec![rect(0.0, 0.0, 612.0, 792.0, p); 3759];
        dedup_rects(&mut rects);
        assert_eq!(rects.len(), 1);
    }

    #[test]
    fn test_dedup_rects_within_tolerance() {
        let p = PageNum::new(1);
        let mut rects = vec![
            rect(10.0, 20.0, 100.0, 50.0, p),
            rect(10.2, 20.1, 100.3, 50.4, p),
        ];
        dedup_rects(&mut rects);
        assert_eq!(rects.len(), 1);
    }

    #[test]
    fn test_dedup_rects_distinct_kept() {
        let p = PageNum::new(1);
        let mut rects = vec![
            rect(10.0, 20.0, 100.0, 50.0, p),
            rect(120.0, 20.0, 100.0, 50.0, p),
            rect(10.0, 80.0, 100.0, 50.0, p),
        ];
        dedup_rects(&mut rects);
        assert_eq!(rects.len(), 3);
    }

    #[test]
    fn test_dedup_rects_different_pages_kept() {
        let mut rects = vec![
            rect(0.0, 0.0, 612.0, 792.0, PageNum::new(1)),
            rect(0.0, 0.0, 612.0, 792.0, PageNum::new(2)),
        ];
        dedup_rects(&mut rects);
        assert_eq!(rects.len(), 2);
    }

    #[test]
    fn test_dedup_rects_empty_and_single() {
        let mut empty: Vec<Rect> = vec![];
        dedup_rects(&mut empty);
        assert!(empty.is_empty());

        let mut single = vec![rect(1.0, 2.0, 3.0, 4.0, PageNum::new(1))];
        dedup_rects(&mut single);
        assert_eq!(single.len(), 1);
    }

    #[test]
    fn test_multiply_matrices_identity() {
        let id = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let m = [2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
        let result = multiply_matrices(&m, &id);
        for i in 0..6 {
            assert!((result[i] - m[i]).abs() < 1e-6, "index {i}");
        }
    }

    #[test]
    fn test_multiply_matrices_translation() {
        let translate = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
        let scale = [2.0, 0.0, 0.0, 3.0, 0.0, 0.0];
        let result = multiply_matrices(&translate, &scale);
        // Translation applied through scale: e' = e*a2 + f*c2 + e2 = 10*2+20*0+0 = 20
        assert!((result[4] - 20.0).abs() < 1e-6);
        assert!((result[5] - 60.0).abs() < 1e-6);
    }

    #[test]
    fn test_strip_pdf_comments_basic() {
        let input = b"BT\n% comment\nTj\nET\n";
        let output = strip_pdf_comments(input);
        assert_eq!(output, b"BT\n \nTj\nET\n");
    }

    #[test]
    fn test_strip_pdf_comments_none() {
        let input = b"BT\nTj\nET\n";
        let output = strip_pdf_comments(input);
        assert_eq!(output, input.to_vec());
    }

    #[test]
    fn test_strip_pdf_comments_inside_string() {
        let input = b"(text with % not a comment)\n% real comment\n";
        let output = strip_pdf_comments(input);
        assert_eq!(output, b"(text with % not a comment)\n \n");
    }

    #[test]
    fn test_strip_pdf_comments_inside_hex_string() {
        let input = b"<0033% not a comment>\n% real comment\n";
        let output = strip_pdf_comments(input);
        assert_eq!(output, b"<0033% not a comment>\n \n");
    }

    #[test]
    fn test_strip_pdf_comments_pd4ml_style() {
        let input = b"<0033> Tj\n\t% Mission Statement\n\tET\n";
        let output = strip_pdf_comments(input);
        let output_str = String::from_utf8_lossy(&output);
        assert!(
            output_str.contains("ET"),
            "ET should be preserved after comment stripping"
        );
    }

    #[test]
    fn test_get_number_integer() {
        let obj = Object::Integer(42);
        assert!((get_number(&obj).unwrap() - 42.0).abs() < 1e-6);
    }

    #[test]
    fn test_get_number_real() {
        let obj = Object::Real(3.14);
        assert!((get_number(&obj).unwrap() - 3.14).abs() < 1e-6);
    }

    #[test]
    fn test_get_number_non_numeric() {
        let obj = Object::Boolean(true);
        assert!(get_number(&obj).is_none());
    }

    #[test]
    fn test_axis_aligned_rect() {
        let segs = vec![
            (0.0, 0.0, 10.0, 0.0),
            (10.0, 0.0, 10.0, 5.0),
            (10.0, 5.0, 0.0, 5.0),
            (0.0, 5.0, 0.0, 0.0),
        ];
        let identity = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let p = PageNum::new(1);
        let r = axis_aligned_rect_from_segs(&segs, &identity, p);
        assert!(r.is_some());
        let r = r.unwrap();
        assert!((r.x - 0.0).abs() < 0.01);
        assert!((r.y - 0.0).abs() < 0.01);
        assert!((r.width - 10.0).abs() < 0.01);
        assert!((r.height - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_axis_aligned_rect_too_small() {
        let segs = vec![
            (0.0, 0.0, 0.5, 0.0),
            (0.5, 0.0, 0.5, 0.5),
            (0.5, 0.5, 0.0, 0.5),
            (0.0, 0.5, 0.0, 0.0),
        ];
        let identity = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let p = PageNum::new(1);
        assert!(axis_aligned_rect_from_segs(&segs, &identity, p).is_none());
    }
}
