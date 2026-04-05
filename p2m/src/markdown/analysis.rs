//! Font statistics and heading tier discovery.

use std::collections::HashMap;

use log::debug;

use crate::types::{PageNum, TextItem, TextLine};

/// Font statistics for a document.
pub(super) struct FontStats {
    /// The most common font size in the document.
    pub(super) most_common_size: f32,
    /// Font size frequency distribution (`size_key` → line count).
    pub(super) size_counts: HashMap<i32, usize>,
    /// Total number of lines counted.
    pub(super) total_lines: usize,
}

/// Compute how rare a font size is in the document (0.0 = most common, 1.0 = unique).
///
/// Heading fonts appear on far fewer lines than body text, so their rarity is high.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(super) fn font_size_rarity(font_size: f32, stats: &FontStats) -> f32 {
    if stats.total_lines == 0 {
        return 0.0;
    }
    let key = (font_size * 10.0) as i32;
    let count = stats.size_counts.get(&key).copied().unwrap_or(0);
    1.0 - (count as f32 / stats.total_lines as f32)
}

/// Calculate font stats directly from items (before grouping into lines).
#[allow(dead_code, clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(super) fn calculate_font_stats_from_items(items: &[TextItem]) -> FontStats {
    let mut size_counts: HashMap<i32, usize> = HashMap::new();

    for item in items {
        if item.font_size >= 9.0 {
            let size_key = (item.font_size * 10.0) as i32;
            *size_counts.entry(size_key).or_insert(0) += 1;
        }
    }

    let total_lines = size_counts.values().sum();

    let most_common_size = size_counts
        .iter()
        .max_by(|(size_a, count_a), (size_b, count_b)| {
            count_a.cmp(count_b).then_with(|| size_b.cmp(size_a))
        })
        .map_or(12.0, |(size, _)| *size as f32 / 10.0);

    FontStats {
        most_common_size,
        size_counts,
        total_lines,
    }
}

/// Calculate font stats from grouped lines.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(super) fn calculate_font_stats(lines: &[TextLine]) -> FontStats {
    let mut size_counts: HashMap<i32, usize> = HashMap::new();

    for line in lines {
        if let Some(first) = line.items.first()
            && first.font_size >= 9.0
        {
            let size_key = (first.font_size * 10.0) as i32;
            *size_counts.entry(size_key).or_insert(0) += 1;
        }
    }

    let total_lines = size_counts.values().sum();

    let most_common_size = size_counts
        .iter()
        .max_by(|(size_a, count_a), (size_b, count_b)| {
            count_a.cmp(count_b).then_with(|| size_b.cmp(size_a))
        })
        .map_or(12.0, |(size, _)| *size as f32 / 10.0);

    FontStats {
        most_common_size,
        size_counts,
        total_lines,
    }
}

/// Determine the heading level for a bold-only line that didn't meet the
/// font-size threshold.
///
/// Returns a level below the lowest font-size tier (or H2 when no tiers exist).
pub(super) fn bold_heading_level(heading_tiers: &[f32]) -> usize {
    let level = heading_tiers.len() + 1;
    level.clamp(2, 6)
}

/// Detect TOC-style lines that contain dot leaders (e.g. "Section Name .... 42").
pub(super) fn has_dot_leaders(text: &str) -> bool {
    if text.contains("....") {
        return true;
    }
    let mut dot_groups = 0;
    let mut consecutive_dots = 0;
    for ch in text.chars() {
        if ch == '.' {
            consecutive_dots += 1;
        } else {
            if consecutive_dots >= 3 {
                dot_groups += 1;
            }
            consecutive_dots = 0;
        }
    }
    if consecutive_dots >= 3 {
        dot_groups += 1;
    }
    dot_groups >= 2
}

/// Compute the Y-gap threshold for paragraph break detection.
///
/// Computes the document's typical (median) line spacing and uses a multiplier
/// on that.  Falls back to `base_size * 1.8` when insufficient data.
pub(super) fn compute_paragraph_threshold(lines: &[TextLine], base_size: f32) -> f32 {
    let fallback = base_size * 1.8;

    let mut gaps: Vec<f32> = Vec::new();
    let mut prev_y: Option<(PageNum, f32)> = None;

    for line in lines {
        if let Some((prev_page, py)) = prev_y
            && line.page == prev_page
        {
            let gap = py - line.y;
            if gap > 0.0 && gap < base_size * 10.0 {
                gaps.push(gap);
            }
        }
        prev_y = Some((line.page, line.y));
    }

    if gaps.len() < 5 {
        return fallback;
    }

    gaps.sort_by(f32::total_cmp);

    let median = gaps[gaps.len() / 2];
    let threshold = (median * 1.3).max(base_size * 1.5);

    debug!(
        "paragraph_threshold: base_size={:.1} median_gap={:.1} threshold={:.1} ({} gaps sampled)",
        base_size,
        median,
        threshold,
        gaps.len()
    );

    threshold
}

/// Discover distinct heading font-size tiers in the document.
///
/// Returns tiers sorted largest-first (tier 0 = H1, tier 1 = H2, …).
/// Sizes within 0.5pt are clustered into the same tier. Capped at 4 tiers.
pub(super) fn compute_heading_tiers(lines: &[TextLine], base_size: f32) -> Vec<f32> {
    let mut heading_sizes: Vec<f32> = Vec::new();

    for line in lines {
        if let Some(first) = line.items.first()
            && first.font_size / base_size >= 1.2
        {
            heading_sizes.push(first.font_size);
        }
    }

    heading_sizes.sort_by(|a, b| b.total_cmp(a));

    let mut tiers: Vec<f32> = Vec::new();
    for size in heading_sizes {
        let already_in_tier = tiers.iter().any(|&t| (t - size).abs() < 0.5);
        if !already_in_tier {
            tiers.push(size);
        }
    }

    tiers.truncate(4);
    tiers
}

/// Detect header level from font size using document-specific heading tiers.
///
/// When tiers are available, maps tier 0→H1, tier 1→H2, etc.
/// Falls back to ratio-based thresholds when no tiers exist.
pub(super) fn detect_header_level(
    font_size: f32,
    base_size: f32,
    heading_tiers: &[f32],
) -> Option<usize> {
    let ratio = font_size / base_size;

    if ratio < 1.2 {
        return None;
    }

    if !heading_tiers.is_empty() {
        for (i, &tier_size) in heading_tiers.iter().enumerate() {
            if (font_size - tier_size).abs() < 0.5 {
                return Some(i + 1);
            }
        }
        if ratio >= 1.5 {
            let level = (heading_tiers.len() + 1).min(4);
            return Some(level);
        }
        return None;
    }

    if ratio >= 2.0 {
        Some(1)
    } else if ratio >= 1.5 {
        Some(2)
    } else if ratio >= 1.25 {
        Some(3)
    } else {
        Some(4)
    }
}
