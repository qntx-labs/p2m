//! Spanning-line detection: items that cross multiple column boundaries.

use super::columns::ColumnRegion;
use crate::text::unicode::effective_width;
use crate::types::TextItem;

/// Identify items that belong to lines spanning across detected columns.
///
/// Groups items into rough lines by Y-proximity and marks items whose line's
/// combined X-span exceeds 1.3× the widest column AND has no gap located at
/// a detected gutter boundary. Returns a boolean mask parallel to `items`.
pub(super) fn identify_spanning_lines(items: &[TextItem], columns: &[ColumnRegion]) -> Vec<bool> {
    let n = items.len();
    let mut mask = vec![false; n];

    if n < 3 || columns.len() < 2 {
        return mask;
    }

    let max_col_width = columns
        .iter()
        .map(|c| c.x_max - c.x_min)
        .fold(0.0_f32, f32::max);
    let span_threshold = max_col_width * 1.3;

    // Gutter centers: boundaries between adjacent columns
    let gutters: Vec<f32> = columns.windows(2).map(|c| c[0].x_max).collect();
    let gutter_tol = 15.0;
    let y_tol = 5.0;

    // Build (original_index, y) pairs sorted by Y descending for grouping
    let mut indexed: Vec<(usize, f32)> =
        items.iter().enumerate().map(|(i, it)| (i, it.y)).collect();
    indexed.sort_by(|a, b| b.1.total_cmp(&a.1));

    // Group by Y-proximity into rough lines (as index sets)
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut current_group: Vec<usize> = Vec::new();
    let mut current_y = f32::NAN;

    for (idx, y) in indexed {
        if current_group.is_empty() || (current_y - y).abs() < y_tol {
            if current_group.is_empty() {
                current_y = y;
            }
            current_group.push(idx);
        } else {
            groups.push(std::mem::take(&mut current_group));
            current_y = y;
            current_group.push(idx);
        }
    }
    if !current_group.is_empty() {
        groups.push(current_group);
    }

    for group in groups {
        if group.len() < 2 {
            continue;
        }

        // Sort group indices by X to compute span
        let mut sorted_by_x: Vec<usize> = group;
        sorted_by_x.sort_by(|&a, &b| {
            items[a]
                .x
                .partial_cmp(&items[b].x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let line_left = items[sorted_by_x[0]].x;
        let last = *sorted_by_x.last().unwrap();
        let line_right = items[last].x + effective_width(&items[last]);
        let span = line_right - line_left;

        if span <= span_threshold {
            continue;
        }

        // Check if any inter-item gap falls at a detected gutter boundary.
        // If so, this is items from different columns at the same Y, not a
        // true spanning line (like a title or section header).
        let has_gutter_gap = sorted_by_x.windows(2).any(|pair| {
            let left_end = items[pair[0]].x + effective_width(&items[pair[0]]);
            let right_start = items[pair[1]].x;
            let gap = right_start - left_end;
            if gap < 5.0 {
                return false;
            }
            // Check if any gutter falls within the gap interval (with tolerance)
            gutters
                .iter()
                .any(|&g| g > left_end - gutter_tol && g < right_start + gutter_tol)
        });

        if !has_gutter_gap {
            for &idx in &sorted_by_x {
                mask[idx] = true;
            }
        }
    }

    mask
}

/// Determines if a text item spans across multiple column regions (e.g. full-width headers/titles).
pub(super) fn spans_multiple_columns(item: &TextItem, columns: &[ColumnRegion]) -> bool {
    let w = effective_width(item);
    let item_right = item.x + w;
    let overlap_count = columns
        .iter()
        .filter(|col| {
            let overlap_start = item.x.max(col.x_min);
            let overlap_end = item_right.min(col.x_max);
            let overlap = (overlap_end - overlap_start).max(0.0);
            overlap > (col.x_max - col.x_min) * 0.10 || overlap > 20.0
        })
        .count();
    overlap_count >= 2
}

/// Check if a text item is likely a page number
pub(super) fn is_page_number(item: &TextItem) -> bool {
    let text = item.text.trim();

    // Must be 1-4 digits only
    if text.is_empty() || text.len() > 4 {
        return false;
    }
    if !text.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    // Must be at top or bottom of page.
    // US Letter = 792pt, A4 = 841pt. Page numbers are typically in the
    // top ~5% or bottom ~12% of the page.
    item.y > 720.0 || item.y < 100.0
}
