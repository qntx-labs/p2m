//! Per-page extraction orchestrator.
//!
//! Iterates over document pages, extracts text items via the content stream
//! parser, and merges results into a unified [`Extraction`](crate::types::Extraction).

pub mod layout;
pub mod text;

use std::collections::{HashMap, HashSet};

use log::debug;
use lopdf::{Document, ObjectId};

use crate::error::Result;
use crate::pdf::content_stream::extract_page_text_items;
use crate::pdf::links::{extract_form_fields, extract_page_links};
use crate::pdf::tounicode::FontCMaps;
use crate::text::spacing::fix_letterspaced_items;
use crate::types::{PageExtraction, PageNum};

/// Per-page adaptive thresholds from letter-spacing detection.
pub(crate) type PageThresholds = HashMap<u32, f32>;

/// Extract positioned text, rectangles, and lines from a document.
///
/// Returns `(items+rects+lines, page_thresholds)`.
pub(crate) fn extract_positioned_text(
    doc: &Document,
    font_cmaps: &FontCMaps,
    page_filter: Option<&HashSet<u32>>,
) -> Result<(PageExtraction, PageThresholds)> {
    let pages = doc.get_pages();
    let mut all_items = Vec::new();
    let mut all_rects = Vec::new();
    let mut all_lines = Vec::new();
    let mut page_thresholds = PageThresholds::new();

    let page_id_to_num: HashMap<ObjectId, u32> =
        pages.iter().map(|(num, &id)| (id, *num)).collect();

    for (page_num, &page_id) in pages.iter() {
        if let Some(filter) = page_filter {
            if !filter.contains(page_num) {
                continue;
            }
        }

        let pn = PageNum::new(*page_num);
        let ((mut items, rects, lines), _has_gid, _rotated) =
            extract_page_text_items(doc, page_id, pn, font_cmaps, false)?;

        let threshold = fix_letterspaced_items(&mut items);
        if threshold > 0.10 {
            page_thresholds.insert(*page_num, threshold);
        }

        debug!(
            "page {}: {} items, {} rects, {} lines",
            page_num,
            items.len(),
            rects.len(),
            lines.len(),
        );

        all_items.extend(items);
        all_rects.extend(rects);
        all_lines.extend(lines);

        // Extract hyperlinks from page annotations.
        let links = extract_page_links(doc, page_id, *page_num);
        all_items.extend(links);
    }

    // Extract AcroForm field values.
    let form_items = extract_form_fields(doc, &page_id_to_num);
    all_items.extend(form_items);

    Ok(((all_items, all_rects, all_lines), page_thresholds))
}
