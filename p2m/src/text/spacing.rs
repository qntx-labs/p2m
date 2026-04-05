//! Position-based word boundary detection and Otsu threshold for
//! letter-spacing anomaly correction.

use crate::types::TextItem;

/// Determine whether two adjacent items should be joined without a space.
///
/// Uses the gap between the end of `prev` and the start of `curr` relative
/// to the font size. The `threshold` parameter is the adaptive letter-spacing
/// threshold (default 0.10, higher for Canva-style PDFs).
pub fn should_join_items(prev: &TextItem, curr: &TextItem, threshold: f32) -> bool {
    // If prev has zero width, don't join (can't measure gap).
    if prev.width <= 0.0 || prev.font_size <= 0.0 {
        return false;
    }

    let gap = curr.x - (prev.x + prev.width);
    let font_size = prev.font_size;

    // Normalise gap relative to font size.
    let ratio = gap / font_size;

    // If the gap is small enough relative to font size, join them.
    ratio < threshold
}
