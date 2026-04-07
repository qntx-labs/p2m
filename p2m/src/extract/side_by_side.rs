//! Side-by-side layout detection.
//!
//! Detects pages where two independent regions (e.g. two tables, or a table +
//! text) are placed side by side, separated by a significant X-position gap.

use crate::types::TextItem;

/// Detect side-by-side layout by finding a significant X-position gap.
///
/// Returns X-band boundaries `[(x_min, split_x), (split_x, x_max)]` when a
/// clear vertical gap separates two groups of items, or an empty vec if the
/// page has a single-region layout.
#[allow(clippy::cast_precision_loss)]
pub fn split_side_by_side(items: &[TextItem]) -> Vec<(f32, f32)> {
    if items.len() < 40 {
        return vec![];
    }

    let mut xs: Vec<f32> = items.iter().map(|i| i.x).collect();
    xs.sort_by(|a, b| a.total_cmp(b));

    let x_min = xs[0];
    let x_max = *xs.last().unwrap();
    let x_range = x_max - x_min;
    let center_lo = x_min + x_range * 0.2;
    let center_hi = x_min + x_range * 0.8;

    let mut candidates: Vec<f32> = Vec::new();
    for i in 1..xs.len() {
        let gap = xs[i] - xs[i - 1];
        let split_x = (xs[i - 1] + xs[i]) / 2.0;
        if gap >= 30.0
            && i >= 20
            && (xs.len() - i) >= 20
            && split_x >= center_lo
            && split_x <= center_hi
        {
            candidates.push(split_x);
        }
    }

    if candidates.is_empty() {
        return vec![];
    }

    let min_side = items.len() / 5;
    let mut best_split = 0.0f32;
    let mut best_crossing = usize::MAX;

    for &split_x in &candidates {
        let left_count = items
            .iter()
            .filter(|i| i.x + i.width / 2.0 < split_x)
            .count();
        let right_count = items.len() - left_count;
        if left_count.min(right_count) < min_side {
            continue;
        }
        let crossing = items
            .iter()
            .filter(|item| item.x < split_x && (item.x + item.width) > split_x)
            .count();
        if crossing < best_crossing {
            best_crossing = crossing;
            best_split = split_x;
        }
    }

    if best_crossing == usize::MAX {
        return vec![];
    }

    let max_crossing = (items.len() / 20).max(2);
    if best_crossing > max_crossing {
        return vec![];
    }

    // Reject multi-column single tables: only accept one cluster of balanced
    // candidate positions.
    let mut balanced_positions: Vec<f32> = candidates
        .iter()
        .filter(|&&sx| {
            let lc = items.iter().filter(|i| i.x + i.width / 2.0 < sx).count();
            let rc = items.len() - lc;
            lc.min(rc) >= min_side
        })
        .copied()
        .collect();
    balanced_positions.sort_by(|a, b| a.total_cmp(b));
    balanced_positions.dedup_by(|a, b| (*a - *b).abs() < 50.0);
    if balanced_positions.len() > 1 {
        return vec![];
    }

    // Reject label+number table layout
    let left_items: Vec<&TextItem> = items
        .iter()
        .filter(|i| i.x + i.width / 2.0 < best_split)
        .collect();
    let right_items: Vec<&TextItem> = items
        .iter()
        .filter(|i| i.x + i.width / 2.0 >= best_split)
        .collect();

    if !left_items.is_empty() && !right_items.is_empty() {
        let is_numeric = |item: &&TextItem| -> bool {
            let text = item.text.trim();
            if text.is_empty() {
                return false;
            }
            let data_chars = text
                .chars()
                .filter(|c| c.is_ascii_digit() || ",.-+%€$£¥()".contains(*c))
                .count();
            data_chars as f32 / text.chars().count() as f32 >= 0.6
        };

        let left_numeric_ratio =
            left_items.iter().filter(|i| is_numeric(i)).count() as f32 / left_items.len() as f32;
        let right_numeric_ratio =
            right_items.iter().filter(|i| is_numeric(i)).count() as f32 / right_items.len() as f32;

        if left_numeric_ratio < 0.30 && right_numeric_ratio >= 0.70 {
            let y_tol = 5.0;
            let y_matches = right_items
                .iter()
                .filter(|ri| left_items.iter().any(|li| (li.y - ri.y).abs() < y_tol))
                .count();
            if y_matches as f32 / right_items.len() as f32 >= 0.5 {
                return vec![];
            }
        }
    }

    vec![(x_min, best_split), (best_split, x_max)]
}
