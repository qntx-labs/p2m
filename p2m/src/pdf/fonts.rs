//! Font encoding, width extraction, and CID decoding.
//!
//! This module handles:
//! - Extracting glyph widths from Type1, TrueType, and CID (Type0) fonts.
//! - Building font encoding maps from `/Differences` arrays.
//! - Decoding text strings using multi-layer fallback (CMap, Differences,
//!   UTF-16BE, UTF-8, Latin-1).
//! - Heuristic scoring to choose between primary and remapped CMaps.

use std::collections::{BTreeMap, HashMap};

use lopdf::{Dictionary, Document, Object};

use crate::pdf::glyph_names::glyph_to_char;
use crate::pdf::tounicode::{CMapEntry, FontCMaps};
use crate::text::unicode::decode_text_string;
use crate::types::{FontEncodingMap, FontWidthInfo, PageFontEncodings, PageFontWidths};

// ── CMap choice ────────────────────────────────────────────────────

/// Which CMap to use for a given font: the original `ToUnicode` or a
/// GID-remapped variant built from the `FontFile2` glyph table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum CMapChoice {
    /// Use the font's original `ToUnicode` CMap.
    Primary,
    /// Use the remapped (GID-based) CMap.
    Remapped,
}

// ── CMap decision cache ────────────────────────────────────────────

/// Minimum number of bytes that must be sampled before committing to a
/// [`CMapChoice`].
const MIN_SAMPLE_BYTES: usize = 240;

/// Accumulated sample data for a single font.
#[derive(Debug, Clone)]
struct CMapSample {
    /// Sum of scores for the primary CMap across all samples.
    primary_score: i64,
    /// Sum of scores for the remapped CMap across all samples.
    remapped_score: i64,
    /// Total byte count sampled so far.
    total_bytes: usize,
    /// Cached final decision (once enough bytes are collected).
    decision: Option<CMapChoice>,
}

/// Accumulates decoded-text samples from multiple strings to decide
/// whether the primary or remapped CMap produces better output for each
/// font object.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub(crate) struct CMapDecisionCache {
    /// Per-font-object-number sample data.
    samples: HashMap<u32, CMapSample>,
}

impl Default for CMapDecisionCache {
    fn default() -> Self {
        Self::new()
    }
}

impl CMapDecisionCache {
    /// Create a new, empty decision cache.
    pub(crate) fn new() -> Self {
        Self {
            samples: HashMap::new(),
        }
    }

    /// Return the cached decision for `obj_num`, if one has been made.
    pub(crate) fn get_choice(&self, obj_num: u32) -> Option<CMapChoice> {
        self.samples.get(&obj_num).and_then(|s| s.decision)
    }

    /// Feed a decoded-text sample from both CMaps and (once enough data
    /// has been collected) return the chosen CMap variant.
    ///
    /// * `obj_num` — the PDF object number identifying the font.
    /// * `primary_text` — text decoded via the primary CMap.
    /// * `remapped_text` — text decoded via the remapped CMap.
    /// * `bytes_len` — number of raw bytes in the operand that produced
    ///   these texts.
    #[allow(clippy::cast_possible_wrap)]
    pub(crate) fn consider(
        &mut self,
        obj_num: u32,
        primary_text: &str,
        remapped_text: &str,
        bytes_len: usize,
    ) -> Option<CMapChoice> {
        let sample = self.samples.entry(obj_num).or_insert_with(|| CMapSample {
            primary_score: 0,
            remapped_score: 0,
            total_bytes: 0,
            decision: None,
        });

        // Already decided — return immediately.
        if let Some(choice) = sample.decision {
            return Some(choice);
        }

        sample.primary_score += i64::from(score_text(primary_text));
        sample.remapped_score += i64::from(score_text(remapped_text));
        sample.total_bytes += bytes_len;

        if sample.total_bytes >= MIN_SAMPLE_BYTES {
            let choice = if sample.primary_score >= sample.remapped_score {
                CMapChoice::Primary
            } else {
                CMapChoice::Remapped
            };
            sample.decision = Some(choice);
            Some(choice)
        } else {
            None
        }
    }
}

// ── Font width extraction ──────────────────────────────────────────

/// Build width tables for every font resource in `fonts`.
///
/// `fonts` is the `/Font` sub-dictionary of a page's `/Resources`.
#[allow(clippy::too_many_lines)]
pub(crate) fn build_font_widths(
    doc: &Document,
    fonts: &BTreeMap<Vec<u8>, &Dictionary>,
) -> PageFontWidths {
    let mut page_widths = PageFontWidths::new();

    for (name_bytes, &font_dict) in fonts {
        let font_name = String::from_utf8_lossy(name_bytes).into_owned();

        let subtype = font_dict.get(b"Subtype").ok().and_then(|o| match o {
            Object::Name(n) => Some(n.as_slice()),
            _ => None,
        });

        let info = match subtype {
            Some(b"Type0") => parse_type0_widths(doc, font_dict),
            Some(b"Type1" | b"TrueType" | b"MMType1") => parse_simple_font_widths(doc, font_dict),
            _ => parse_simple_font_widths(doc, font_dict),
        };

        if let Some(wi) = info {
            page_widths.insert(font_name, wi);
        }
    }

    page_widths
}

/// Parse widths for a simple font (Type1, TrueType, `MMType1`).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_simple_font_widths(doc: &Document, font_dict: &Dictionary) -> Option<FontWidthInfo> {
    let first_char = font_dict
        .get(b"FirstChar")
        .ok()
        .and_then(|o| obj_to_u32(o))
        .unwrap_or(0);

    let widths_array = font_dict
        .get(b"Widths")
        .ok()
        .and_then(|o| resolve_array(doc, o));

    let mut widths = HashMap::new();
    let mut space_width: u16 = 0;

    if let Some(arr) = widths_array {
        for (i, obj) in arr.iter().enumerate() {
            let w = resolve_number(doc, obj).unwrap_or(0.0);
            if w > 0.0 {
                let code = first_char + i as u32;
                let w16 = (w as u32).min(u32::from(u16::MAX)) as u16;
                widths.insert(code as u16, w16);
                if code == 32 {
                    space_width = w16;
                }
            }
        }
    }

    // Fall back to font descriptor for missing width.
    let default_width = font_descriptor_default_width(doc, font_dict);

    if space_width == 0 {
        space_width = widths.get(&32).copied().unwrap_or(default_width);
    }

    Some(FontWidthInfo {
        widths,
        default_width,
        space_width,
        is_cid: false,
        units_scale: 0.001,
        wmode: 0,
    })
}

/// Parse widths for a Type0 (CID) font.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_type0_widths(doc: &Document, font_dict: &Dictionary) -> Option<FontWidthInfo> {
    let descendants = font_dict
        .get(b"DescendantFonts")
        .ok()
        .and_then(|o| resolve_array(doc, o))?;

    let cid_dict = descendants
        .first()
        .and_then(|o| resolve_object_to_dict(doc, o))?;

    // Default width (DW entry).
    let dw = cid_dict
        .get(b"DW")
        .ok()
        .and_then(|o| resolve_number(doc, o))
        .unwrap_or(1000.0);
    let default_width = (dw as u32).min(u32::from(u16::MAX)) as u16;

    // Writing mode.
    let wmode = cid_dict
        .get(b"WMode")
        .ok()
        .and_then(|o| obj_to_u32(o))
        .unwrap_or(0);

    let mut widths = HashMap::new();

    // Parse the W array.
    if let Some(w_obj) = cid_dict.get(b"W").ok() {
        if let Some(w_arr) = resolve_array(doc, w_obj) {
            parse_cid_w_array(doc, w_arr, &mut widths);
        }
    }

    let space_width = widths.get(&32).copied().unwrap_or(default_width);

    Some(FontWidthInfo {
        widths,
        default_width,
        space_width,
        is_cid: true,
        units_scale: 0.001,
        wmode: wmode.min(1) as u8,
    })
}

/// Parse a CID font `W` (widths) array.
///
/// The array alternates between two formats:
/// - `[c [w1 w2 w3 ...]]` — consecutive widths starting at CID `c`.
/// - `[c1 c2 w]` — all CIDs from `c1` to `c2` share width `w`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn parse_cid_w_array(
    doc: &Document,
    w_array: &[Object],
    widths: &mut HashMap<u16, u16>,
) {
    let len = w_array.len();
    let mut i = 0;

    while i < len {
        let start_cid = match obj_to_u32_resolved(doc, &w_array[i]) {
            Some(v) => v,
            None => {
                i += 1;
                continue;
            }
        };
        i += 1;
        if i >= len {
            break;
        }

        // Format 1: [c [w1 w2 w3 ...]]
        if let Some(arr) = resolve_array(doc, &w_array[i]) {
            for (j, w_obj) in arr.iter().enumerate() {
                let w = resolve_number(doc, w_obj).unwrap_or(0.0);
                let cid = start_cid + j as u32;
                if cid <= u32::from(u16::MAX) {
                    let w16 = (w as u32).min(u32::from(u16::MAX)) as u16;
                    widths.insert(cid as u16, w16);
                }
            }
            i += 1;
            continue;
        }

        // Format 2: [c1 c2 w]
        if i + 1 < len {
            let end_cid = match obj_to_u32_resolved(doc, &w_array[i]) {
                Some(v) => v,
                None => {
                    i += 1;
                    continue;
                }
            };
            i += 1;
            let w = resolve_number(doc, &w_array[i]).unwrap_or(0.0);
            let w16 = (w as u32).min(u32::from(u16::MAX)) as u16;
            i += 1;

            let end = end_cid.min(start_cid + 0xFFFF);
            for cid in start_cid..=end {
                if cid <= u32::from(u16::MAX) {
                    widths.insert(cid as u16, w16);
                }
            }
        }
    }
}

// ── String width computation ───────────────────────────────────────

/// Compute the width of a byte string in text-space units.
///
/// Accounts for per-glyph widths, character spacing, and word spacing
/// (applied to the space byte `0x20`).
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub(crate) fn compute_string_width_ts(
    bytes: &[u8],
    font_info: &FontWidthInfo,
    font_size: f32,
    char_spacing: f32,
    word_spacing: f32,
) -> f32 {
    let scale = font_info.units_scale;
    let mut width: f32 = 0.0;

    if font_info.is_cid {
        // Two-byte character codes.
        let mut i = 0;
        while i + 1 < bytes.len() {
            let code = u16::from_be_bytes([bytes[i], bytes[i + 1]]);
            let glyph_w = font_info
                .widths
                .get(&code)
                .copied()
                .unwrap_or(font_info.default_width);
            width += f32::from(glyph_w) * scale * font_size;
            width += char_spacing;
            // Word spacing applies when the 2-byte code is 0x0020.
            if code == 0x0020 {
                width += word_spacing;
            }
            i += 2;
        }
    } else {
        // Single-byte character codes.
        for &byte in bytes {
            let code = u16::from(byte);
            let glyph_w = font_info
                .widths
                .get(&code)
                .copied()
                .unwrap_or(font_info.default_width);
            width += f32::from(glyph_w) * scale * font_size;
            width += char_spacing;
            if byte == b' ' {
                width += word_spacing;
            }
        }
    }

    width
}

// ── Font encoding ──────────────────────────────────────────────────

/// Result of parsing an `/Encoding` dictionary.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub(crate) struct EncodingResult {
    /// Byte-code to Unicode character map built from `/Differences`.
    pub(crate) map: FontEncodingMap,
    /// Number of entries that look like raw GID names (e.g. "glyph00042").
    pub(crate) gid_glyph_count: u32,
}

/// Build encoding maps for every font resource in `fonts`.
///
/// Returns the page-level encoding map and a boolean flag indicating whether
/// any font uses GID-style glyph names (which may require `FontFile2`
/// fallback).
pub(crate) fn build_font_encodings(
    doc: &Document,
    fonts: &BTreeMap<Vec<u8>, &Dictionary>,
) -> (PageFontEncodings, bool) {
    let mut page_encodings = PageFontEncodings::new();
    let mut has_gid_fonts = false;

    for (name_bytes, &font_dict) in fonts {
        let font_name = String::from_utf8_lossy(name_bytes).into_owned();

        // Check for Encoding entry.
        let enc_obj = match font_dict.get(b"Encoding").ok() {
            Some(o) => o,
            None => continue,
        };

        let result = match enc_obj {
            Object::Dictionary(d) => parse_encoding_dictionary(doc, d),
            Object::Reference(r) => {
                let resolved = doc.get_object(*r).ok();
                match resolved {
                    Some(Object::Dictionary(d)) => parse_encoding_dictionary(doc, d),
                    _ => None,
                }
            }
            // Named encoding like /WinAnsiEncoding — skip (handled by CMap).
            _ => None,
        };

        if let Some(enc_result) = result {
            if enc_result.gid_glyph_count > 0 {
                has_gid_fonts = true;
            }
            if !enc_result.map.is_empty() {
                page_encodings.insert(font_name, enc_result.map);
            }
        }
    }

    (page_encodings, has_gid_fonts)
}

/// Parse an `/Encoding` dictionary, extracting the `/Differences` array.
///
/// Returns `None` if no usable `/Differences` array is present.
#[allow(clippy::cast_possible_truncation)]
fn parse_encoding_dictionary(doc: &Document, enc_dict: &Dictionary) -> Option<EncodingResult> {
    let diff_obj = enc_dict.get(b"Differences").ok()?;
    let diff_arr = resolve_array(doc, diff_obj)?;

    let mut map = FontEncodingMap::new();
    let mut gid_glyph_count: u32 = 0;
    let mut current_code: u32 = 0;

    for item in diff_arr {
        match item {
            Object::Integer(n) => {
                current_code = (*n).max(0) as u32;
            }
            Object::Name(name_bytes) => {
                let glyph_name = String::from_utf8_lossy(name_bytes);
                let glyph_name_str = glyph_name.as_ref();

                // Detect GID-style names.
                if is_gid_glyph_name(glyph_name_str) {
                    gid_glyph_count += 1;
                }

                if let Some(ch) = glyph_name_to_char(glyph_name_str) {
                    if current_code <= 255 {
                        map.insert(current_code as u8, ch);
                    }
                }

                current_code = current_code.saturating_add(1);
            }
            // References in Differences arrays: resolve to Name.
            Object::Reference(r) => {
                if let Ok(Object::Name(name_bytes)) = doc.get_object(*r) {
                    let glyph_name = String::from_utf8_lossy(name_bytes);
                    let glyph_name_str = glyph_name.as_ref();

                    if is_gid_glyph_name(glyph_name_str) {
                        gid_glyph_count += 1;
                    }

                    if let Some(ch) = glyph_name_to_char(glyph_name_str) {
                        if current_code <= 255 {
                            map.insert(current_code as u8, ch);
                        }
                    }

                    current_code = current_code.saturating_add(1);
                }
            }
            _ => {}
        }
    }

    Some(EncodingResult {
        map,
        gid_glyph_count,
    })
}

/// Check if a glyph name looks like a raw GID placeholder (e.g.
/// "glyph00042", "cid12345").
fn is_gid_glyph_name(name: &str) -> bool {
    // Common GID patterns: "glyphNNNN", "cidNNNNN", "GNNNN", "uniXXXX" with
    // no useful mapping.
    let lower = name.to_lowercase();
    (lower.starts_with("glyph") && lower[5..].chars().all(|c| c.is_ascii_digit()))
        || (lower.starts_with("cid") && lower[3..].chars().all(|c| c.is_ascii_digit()))
        || (lower.starts_with('g')
            && lower.len() > 1
            && lower[1..].chars().all(|c| c.is_ascii_digit()))
}

/// Convert a glyph name to a Unicode character.
///
/// Tries the Adobe Glyph List first, then falls back to `uniXXXX` parsing
/// and Symbol/Wingdings PUA mapping.
fn glyph_name_to_char(name: &str) -> Option<char> {
    // .notdef produces nothing.
    if name == ".notdef" || name == "notdef" {
        return None;
    }

    // Try the Adobe Glyph List.
    if let Some(ch) = glyph_to_char(name) {
        return Some(ch);
    }

    // Try uniXXXX format.
    if name.starts_with("uni") && name.len() >= 7 {
        if let Ok(code) = u32::from_str_radix(&name[3..7], 16) {
            return char::from_u32(code);
        }
    }

    // Try uXXXX / uXXXXX format.
    if name.starts_with('u') && name.len() >= 5 {
        let hex_part = &name[1..];
        if hex_part.chars().all(|c| c.is_ascii_hexdigit()) {
            if let Ok(code) = u32::from_str_radix(hex_part, 16) {
                return char::from_u32(code);
            }
        }
    }

    None
}

// ── Text extraction from operand ───────────────────────────────────

/// Decode a PDF string operand into a Unicode `String`.
///
/// Applies a multi-layer fallback strategy:
/// 1. `ToUnicode` CMap (primary or remapped, chosen via scoring).
/// 2. `/Differences` encoding from the font dictionary.
/// 3. UTF-16BE detection (BOM `FE FF`).
/// 4. UTF-8 heuristic.
/// 5. Latin-1 (ISO 8859-1) as last resort.
///
/// # Arguments
///
/// * `obj` — the raw PDF `Object` (expected `String` or `HexString`).
/// * `current_font` — the font resource name currently in effect.
/// * `base_font_name` — the `/BaseFont` value, used for Symbol/Wingdings.
/// * `font_cmaps` — page-level CMap lookup table.
/// * `font_tounicode_refs` — maps font names to `ToUnicode` stream object numbers.
/// * `inline_cmaps` — CMaps embedded inline (not via reference).
/// * `font_encodings` — per-font `/Differences` encoding maps.
/// * `encoding_cache` — mutable cache for lazily-parsed encodings.
/// * `cmap_decisions` — mutable cache for primary-vs-remapped CMap choice.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    clippy::similar_names
)]
pub(crate) fn extract_text_from_operand(
    obj: &Object,
    current_font: &str,
    base_font_name: &str,
    font_cmaps: &FontCMaps,
    font_tounicode_refs: &HashMap<String, u32>,
    inline_cmaps: &HashMap<String, CMapEntry>,
    font_encodings: &PageFontEncodings,
    encoding_cache: &mut HashMap<String, FontEncodingMap>,
    cmap_decisions: &mut CMapDecisionCache,
) -> Option<String> {
    let bytes = match obj {
        Object::String(b, _) => b.as_slice(),
        _ => return None,
    };

    if bytes.is_empty() {
        return None;
    }

    // ── Layer 1: ToUnicode CMap ────────────────────────────────────
    let obj_num = font_tounicode_refs.get(current_font).copied().unwrap_or(0);

    // Try inline CMap first, then global FontCMaps by object number.
    let entry: Option<&CMapEntry> = inline_cmaps
        .get(current_font)
        .or_else(|| font_cmaps.get_by_obj(obj_num));

    if let Some(cmap_entry) = entry {
        let primary = &cmap_entry.primary;

        if let Some(remapped) = &cmap_entry.remapped {
            // Decide between primary and remapped CMap.
            if let Some(choice) = cmap_decisions.get_choice(obj_num) {
                let cmap = match choice {
                    CMapChoice::Primary => primary,
                    CMapChoice::Remapped => remapped,
                };
                let text = cmap.decode_cids(bytes);
                if !text.is_empty() {
                    return Some(clean_pua_chars(&text));
                }
            } else {
                let primary_text = primary.decode_cids(bytes);
                let remapped_text = remapped.decode_cids(bytes);

                if !primary_text.is_empty() || !remapped_text.is_empty() {
                    let choice = cmap_decisions.consider(
                        obj_num,
                        &primary_text,
                        &remapped_text,
                        bytes.len(),
                    );
                    let chosen = match choice {
                        Some(CMapChoice::Remapped) => &remapped_text,
                        _ => &primary_text,
                    };
                    if !chosen.is_empty() {
                        return Some(clean_pua_chars(chosen));
                    }
                }
            }
        } else {
            let text = primary.decode_cids(bytes);
            if !text.is_empty() {
                return Some(clean_pua_chars(&text));
            }
        }

        // Try fallback CMap.
        if let Some(fallback) = &cmap_entry.fallback {
            let text = fallback.decode_cids(bytes);
            if !text.is_empty() {
                return Some(clean_pua_chars(&text));
            }
        }
    }

    // ── Layer 2: /Differences encoding ─────────────────────────────
    let enc_map = font_encodings
        .get(current_font)
        .or_else(|| encoding_cache.get(current_font));

    if let Some(enc) = enc_map {
        let mut text = String::with_capacity(bytes.len());
        let mut all_mapped = true;

        for &byte in bytes {
            if let Some(&ch) = enc.get(&byte) {
                text.push(ch);
            } else if byte >= 0x20 && byte < 0x7F {
                // ASCII printable range — use as-is.
                text.push(byte as char);
            } else {
                all_mapped = false;
                break;
            }
        }

        if all_mapped && !text.is_empty() {
            return Some(apply_symbol_pua(base_font_name, &text));
        }
    }

    // ── Layer 3: UTF-16BE (BOM detected) ───────────────────────────
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let decoded = decode_text_string(bytes);
        if !decoded.is_empty() {
            return Some(decoded);
        }
    }

    // ── Layer 4: UTF-8 heuristic ───────────────────────────────────
    if let Ok(text) = std::str::from_utf8(bytes) {
        if !text.is_empty() && text.chars().all(|c| c != '\0') {
            return Some(apply_symbol_pua(base_font_name, text));
        }
    }

    // ── Layer 5: Latin-1 fallback ──────────────────────────────────
    #[allow(clippy::cast_lossless)]
    let text: String = bytes.iter().map(|&b| b as char).collect();
    if text.is_empty() {
        None
    } else {
        Some(apply_symbol_pua(base_font_name, &text))
    }
}

// decode_with_cmap removed — use ToUnicodeCMap::decode_cids() directly.

/// Apply Private Use Area remapping for Symbol and Wingdings fonts.
///
/// These fonts encode standard characters in the PUA range `F000`–`F0FF`.
/// We strip the `F0` prefix to recover the intended code point.
fn apply_symbol_pua(base_font_name: &str, text: &str) -> String {
    let lower = base_font_name.to_lowercase();
    let is_symbol = lower.contains("symbol") || lower.contains("wingding");

    if !is_symbol {
        return text.to_string();
    }

    text.chars()
        .map(|ch| {
            let code = ch as u32;
            if (0xF000..=0xF0FF).contains(&code) {
                // Map PUA to the low byte equivalent.
                char::from_u32(code & 0xFF).unwrap_or(ch)
            } else {
                ch
            }
        })
        .collect()
}

// ── Text quality scoring ───────────────────────────────────────────

/// Heuristic quality score for decoded text.
///
/// Higher scores indicate text that looks more like natural language.
/// Used by [`CMapDecisionCache`] to choose between primary and remapped
/// CMaps.
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
pub(crate) fn score_text(text: &str) -> i32 {
    if text.is_empty() {
        return 0;
    }

    let mut score: i32 = 0;
    let mut letters: u32 = 0;
    let mut spaces: u32 = 0;
    let mut digits: u32 = 0;
    let mut pua_count: u32 = 0;
    let mut replacement_count: u32 = 0;
    let mut control_count: u32 = 0;
    let total_chars = text.chars().count() as u32;

    for ch in text.chars() {
        if ch.is_alphabetic() {
            letters += 1;
        } else if ch == ' ' {
            spaces += 1;
        } else if ch.is_ascii_digit() {
            digits += 1;
        } else if is_private_use_area(ch) {
            pua_count += 1;
        } else if ch == '\u{FFFD}' {
            replacement_count += 1;
        } else if ch.is_control() && ch != '\n' && ch != '\r' && ch != '\t' {
            control_count += 1;
        }
    }

    // Reward letters and spaces (hallmarks of natural text).
    score += letters as i32 * 2;
    score += spaces as i32 * 3;
    score += digits as i32;

    // Penalize garbage indicators.
    score -= pua_count as i32 * 5;
    score -= replacement_count as i32 * 10;
    score -= control_count as i32 * 5;

    // Bonus for common short words found in the text.
    let lower = text.to_lowercase();
    for word in &[
        " the ", " and ", " of ", " to ", " in ", " a ", " is ", " for ", " on ", " that ", " it ",
        " with ", " as ", " was ", " at ", " by ", " an ", " be ", " this ", " from ", " or ",
        " are ", " but ", " not ", " you ", " all ", " can ", " had ", " her ", " one ", " our ",
        " out ", " de ", " la ", " le ", " et ", " en ", " un ", " une ", " les ", " des ", " du ",
        " au ", " der ", " die ", " das ", " und ", " von ", " den ", " mit ",
    ] {
        if lower.contains(word) {
            score += 10;
        }
    }

    // Penalize if majority of characters are PUA or replacement.
    if total_chars > 0 && (pua_count + replacement_count) * 2 > total_chars {
        score -= 50;
    }

    score
}

// ── Clean PUA characters ───────────────────────────────────────────

/// Strip Unicode Private Use Area characters from text.
///
/// PUA codepoints (`E000`–`F8FF`, plus supplementary planes) are removed
/// because they have no standard meaning and typically indicate failed
/// decoding.
pub(crate) fn clean_pua_chars(text: &str) -> String {
    if !text.chars().any(is_private_use_area) {
        return text.to_string();
    }

    text.chars()
        .filter(|ch| !is_private_use_area(*ch))
        .collect()
}

/// Check if a character is in the Unicode Private Use Area.
const fn is_private_use_area(ch: char) -> bool {
    let code = ch as u32;
    // BMP PUA: U+E000..U+F8FF
    // Supplementary PUA-A: U+F0000..U+FFFFD
    // Supplementary PUA-B: U+100000..U+10FFFD
    (code >= 0xE000 && code <= 0xF8FF)
        || (code >= 0xF_0000 && code <= 0xF_FFFD)
        || (code >= 0x10_0000 && code <= 0x10_FFFD)
}

// ── Helper: resolve PDF objects ────────────────────────────────────

/// Resolve a PDF object that may be a `Reference` to an `Array`.
pub(crate) fn resolve_array<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Vec<Object>> {
    match obj {
        Object::Array(arr) => Some(arr),
        Object::Reference(r) => match doc.get_object(*r).ok()? {
            Object::Array(arr) => Some(arr),
            _ => None,
        },
        _ => None,
    }
}

/// Resolve a PDF object that may be a `Reference` to a `Dictionary`.
pub(crate) fn resolve_dict<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Reference(r) => match doc.get_object(*r).ok()? {
            Object::Dictionary(d) => Some(d),
            _ => None,
        },
        _ => None,
    }
}

/// Resolve a font dictionary entry (which may be a direct dict, a reference
/// to a dict, or a stream whose dictionary we want).
fn resolve_object_to_dict<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Reference(r) => match doc.get_object(*r).ok()? {
            Object::Dictionary(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        },
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

/// Get the object number of the `/FontFile2` entry in a font's descriptor.
///
/// This is used for TrueType fonts when we need to build a GID-to-Unicode
/// remapping from the `cmap` table inside the embedded font program.
pub(crate) fn get_font_file2_obj_num(doc: &Document, font_dict: &Dictionary) -> Option<u32> {
    let desc_obj = font_dict.get(b"FontDescriptor").ok()?;
    let desc = resolve_dict(doc, desc_obj)?;
    let ff2 = desc.get(b"FontFile2").ok()?;

    match ff2 {
        Object::Reference(r) => Some(r.0),
        _ => None,
    }
}

/// Extract the default width from a font descriptor's `/MissingWidth` entry.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn font_descriptor_default_width(doc: &Document, font_dict: &Dictionary) -> u16 {
    let desc_obj = match font_dict.get(b"FontDescriptor").ok() {
        Some(o) => o,
        None => return 600,
    };
    let desc = match resolve_dict(doc, desc_obj) {
        Some(d) => d,
        None => return 600,
    };
    let mw = desc
        .get(b"MissingWidth")
        .ok()
        .and_then(|o| resolve_number(doc, o))
        .unwrap_or(600.0);
    (mw as u32).min(u32::from(u16::MAX)) as u16
}

/// Resolve a PDF object to a numeric (f64) value, following references.
fn resolve_number(doc: &Document, obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(i) => Some(*i as f64),
        Object::Real(f) => Some(*f as f64),
        Object::Reference(r) => {
            let resolved = doc.get_object(*r).ok()?;
            resolve_number(doc, resolved)
        }
        _ => None,
    }
}

/// Convert a PDF object to a `u32`, without following references.
#[allow(clippy::cast_sign_loss)]
fn obj_to_u32(obj: &Object) -> Option<u32> {
    match obj {
        Object::Integer(i) => {
            if *i >= 0 {
                Some(*i as u32)
            } else {
                None
            }
        }
        Object::Real(f) => {
            if *f >= 0.0 {
                Some(*f as u32)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Convert a PDF object to a `u32`, following references.
#[allow(clippy::cast_sign_loss)]
fn obj_to_u32_resolved(doc: &Document, obj: &Object) -> Option<u32> {
    match obj {
        Object::Integer(i) => {
            if *i >= 0 {
                Some(*i as u32)
            } else {
                None
            }
        }
        Object::Real(f) => {
            if *f >= 0.0 {
                Some(*f as u32)
            } else {
                None
            }
        }
        Object::Reference(r) => {
            let resolved = doc.get_object(*r).ok()?;
            obj_to_u32(resolved)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── CMapDecisionCache ──────────────────────────────────────────

    #[test]
    fn decision_cache_returns_none_until_threshold() {
        let mut cache = CMapDecisionCache::new();
        // 100 bytes is below the 240-byte threshold.
        let result = cache.consider(1, "hello world", "h3ll0 w0rld", 100);
        assert!(result.is_none());
        assert!(cache.get_choice(1).is_none());
    }

    #[test]
    fn decision_cache_decides_after_threshold() {
        let mut cache = CMapDecisionCache::new();
        // Feed enough bytes.
        cache.consider(1, "hello world test", "garbled text", 120);
        let result = cache.consider(1, "another good sentence", "more garbage", 130);
        assert!(result.is_some());
        assert!(cache.get_choice(1).is_some());
    }

    #[test]
    fn decision_cache_sticky_after_decision() {
        let mut cache = CMapDecisionCache::new();
        cache.consider(1, "good text here", "bad text", 250);
        let choice = cache.get_choice(1);
        assert_eq!(choice, Some(CMapChoice::Primary));
        // Subsequent calls return the same decision.
        let again = cache.consider(1, "bad text", "good text here", 100);
        assert_eq!(again, Some(CMapChoice::Primary));
    }

    // ── score_text ─────────────────────────────────────────────────

    #[test]
    fn score_natural_text_higher_than_garbage() {
        let good = score_text("The quick brown fox jumps over the lazy dog.");
        let bad = score_text("\u{FFFD}\u{FFFD}\u{FFFD}\u{E000}\u{E001}");
        assert!(good > bad, "good={good}, bad={bad}");
    }

    #[test]
    fn score_empty_is_zero() {
        assert_eq!(score_text(""), 0);
    }

    // ── clean_pua_chars ────────────────────────────────────────────

    #[test]
    fn clean_pua_strips_bmp_pua() {
        let input = "Hello\u{E000}World\u{F8FF}!";
        assert_eq!(clean_pua_chars(input), "HelloWorld!");
    }

    #[test]
    fn clean_pua_preserves_normal_text() {
        let input = "Hello World";
        assert_eq!(clean_pua_chars(input), "Hello World");
    }

    // ── is_gid_glyph_name ──────────────────────────────────────────

    #[test]
    fn gid_glyph_names_detected() {
        assert!(is_gid_glyph_name("glyph00042"));
        assert!(is_gid_glyph_name("cid12345"));
        assert!(is_gid_glyph_name("g42"));
        assert!(!is_gid_glyph_name("Agrave"));
        assert!(!is_gid_glyph_name("space"));
    }

    // ── glyph_name_to_char ─────────────────────────────────────────

    #[test]
    fn glyph_name_space() {
        assert_eq!(glyph_name_to_char("space"), Some(' '));
    }

    #[test]
    fn glyph_name_notdef() {
        assert_eq!(glyph_name_to_char(".notdef"), None);
        assert_eq!(glyph_name_to_char("notdef"), None);
    }

    #[test]
    fn glyph_name_uni_format() {
        assert_eq!(glyph_name_to_char("uni0041"), Some('A'));
        assert_eq!(glyph_name_to_char("uni00E9"), Some('é'));
    }

    #[test]
    fn glyph_name_u_format() {
        assert_eq!(glyph_name_to_char("u0041"), Some('A'));
    }

    // ── parse_cid_w_array ──────────────────────────────────────────

    #[test]
    fn parse_w_array_consecutive_format() {
        let doc = Document::new();
        // Format: [1 [500 600 700]]
        let w = vec![
            Object::Integer(1),
            Object::Array(vec![
                Object::Integer(500),
                Object::Integer(600),
                Object::Integer(700),
            ]),
        ];
        let mut widths = HashMap::new();
        parse_cid_w_array(&doc, &w, &mut widths);
        assert_eq!(widths.get(&1), Some(&500));
        assert_eq!(widths.get(&2), Some(&600));
        assert_eq!(widths.get(&3), Some(&700));
    }

    #[test]
    fn parse_w_array_range_format() {
        let doc = Document::new();
        // Format: [10 15 1000]
        let w = vec![
            Object::Integer(10),
            Object::Integer(15),
            Object::Integer(1000),
        ];
        let mut widths = HashMap::new();
        parse_cid_w_array(&doc, &w, &mut widths);
        for cid in 10..=15 {
            assert_eq!(widths.get(&cid), Some(&1000));
        }
    }

    // ── compute_string_width_ts ────────────────────────────────────

    #[test]
    fn width_simple_font_ascii() {
        let mut widths = HashMap::new();
        widths.insert(72, 700_u16); // 'H'
        widths.insert(101, 500_u16); // 'e'
        let info = FontWidthInfo {
            widths,
            default_width: 600,
            space_width: 250,
            is_cid: false,
            units_scale: 0.001,
            wmode: 0,
        };
        let w = compute_string_width_ts(b"He", &info, 12.0, 0.0, 0.0);
        // (700 * 0.001 * 12) + (500 * 0.001 * 12) = 8.4 + 6.0 = 14.4
        let expected = 14.4_f32;
        assert!((w - expected).abs() < 0.01, "w={w}, expected={expected}");
    }

    // ── private_use_area ───────────────────────────────────────────

    #[test]
    fn pua_detection() {
        assert!(is_private_use_area('\u{E000}'));
        assert!(is_private_use_area('\u{F8FF}'));
        assert!(!is_private_use_area('A'));
        assert!(!is_private_use_area('\u{FFFD}'));
    }
}
