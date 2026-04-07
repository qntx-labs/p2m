//! Column detection via horizontal projection histograms.

use log::debug;

use crate::text::unicode::effective_width;
use crate::types::TextItem;

/// Represents a column region on a page
#[derive(Debug, Clone)]
pub(crate) struct ColumnRegion {
    pub(crate) x_min: f32,
    pub(crate) x_max: f32,
}

/// Detect column boundaries on a page using a horizontal projection profile.
///
/// Builds an occupancy histogram across the page width and finds empty valleys
/// (gutters) where no text exists. Validates valleys with vertical consistency
/// checks to avoid false positives.
pub(crate) fn detect_columns(
    items: &[TextItem],
    page: u32,
    page_has_table: bool,
) -> Vec<ColumnRegion> {
    const BIN_WIDTH: f32 = 2.0;
    const MIN_GUTTER_WIDTH: f32 = 8.0;
    const MIN_VERTICAL_SPAN_RATIO: f32 = 0.30;
    const MIN_ITEMS_PER_COLUMN: usize = 10;
    const NOISE_FRACTION: f32 = 0.15;

    // Get items for this page
    let page_num = crate::types::PageNum::new(page);
    let page_items: Vec<&TextItem> = items.iter().filter(|i| i.page == page_num).collect();

    if page_items.is_empty() {
        return vec![];
    }
    debug!("page {}: detect_columns: {} items", page, page_items.len());

    // Find page bounds
    let x_min = page_items.iter().map(|i| i.x).fold(f32::INFINITY, f32::min);
    let x_max = page_items
        .iter()
        .map(|i| i.x + effective_width(i))
        .fold(f32::NEG_INFINITY, f32::max);

    let page_width = x_max - x_min;
    if page_width < 200.0 {
        return vec![ColumnRegion { x_min, x_max }];
    }

    if page_items.len() < 20 {
        return vec![ColumnRegion { x_min, x_max }];
    }

    // Build occupancy histogram.
    // Exclude items wider than 60% of page width — these are spanning items
    // (titles, full-width paragraphs) that would fill the gutter and prevent
    // detection of partial-page column layouts (e.g. two-column abstracts on
    // a page that also has single-column introduction text).
    let wide_threshold = page_width * 0.6;
    let num_bins = ((page_width / BIN_WIDTH).ceil() as usize).max(1);
    let mut histogram = vec![0u32; num_bins];

    for item in &page_items {
        let w = effective_width(item);
        if w > wide_threshold {
            continue;
        }
        let left = ((item.x - x_min) / BIN_WIDTH).floor() as usize;
        let right = (((item.x + w) - x_min) / BIN_WIDTH).ceil() as usize;
        let left = left.min(num_bins);
        let right = right.min(num_bins);
        for count in histogram.iter_mut().take(right).skip(left) {
            *count += 1;
        }
    }

    // Find the noise threshold: bins with count <= max_count * NOISE_FRACTION are "empty"
    let max_count = *histogram.iter().max().unwrap_or(&0);
    let noise_threshold = (max_count as f32 * NOISE_FRACTION) as u32;

    // Find empty valleys (consecutive runs of low-count bins)
    // Each valley is stored as (start_bin, end_bin)
    let mut valleys: Vec<(usize, usize)> = Vec::new();
    let mut valley_start: Option<usize> = None;

    for (i, &count) in histogram.iter().enumerate() {
        if count <= noise_threshold {
            if valley_start.is_none() {
                valley_start = Some(i);
            }
        } else if let Some(start) = valley_start {
            valleys.push((start, i));
            valley_start = None;
        }
    }
    // Close any valley that extends to the end
    if let Some(start) = valley_start {
        valleys.push((start, num_bins));
    }

    // Filter valleys: must be wide enough and not at page margins
    let margin_threshold = page_width * 0.05;
    let valleys: Vec<(usize, usize)> = valleys
        .into_iter()
        .filter(|&(start, end)| {
            let width_pts = (end - start) as f32 * BIN_WIDTH;
            if width_pts < MIN_GUTTER_WIDTH {
                return false;
            }
            // Valley center must not be within 5% of page edges
            let center_pts = ((start + end) as f32 / 2.0) * BIN_WIDTH;
            center_pts > margin_threshold && center_pts < (page_width - margin_threshold)
        })
        .collect();

    // Fallback: if no absolute valleys found, try relative valley detection.
    // Justified text can leave gutter bins non-empty because item widths extend
    // to the column edge. Look for local minima that are significantly lower
    // than the peaks on either side.
    // Only attempt this for dense pages (>=100 items) — sparse pages with shallow
    // histogram dips are likely not multi-column.
    // Skip on pages with detected tables — table column gaps look like gutters
    // in the histogram but the table pipeline already handles reading order.
    if valleys.is_empty() && page_items.len() >= 100 && !page_has_table {
        let rel_valleys = find_relative_valleys(
            &histogram,
            num_bins,
            x_min,
            BIN_WIDTH,
            page_width,
            margin_threshold,
        );
        if !rel_valleys.is_empty() {
            let result = validate_and_build_columns(
                &rel_valleys,
                &page_items,
                x_min,
                BIN_WIDTH,
                x_max,
                MIN_ITEMS_PER_COLUMN,
                MIN_VERTICAL_SPAN_RATIO,
                page,
                true, // center-based assignment for relative valleys
            );
            if result.len() > 1 {
                // Validate that both sides contain paragraph-like content.
                // Tables, forms, and checklists have short scattered items
                // that create false gutter signals. Only commit to relative
                // valley columns when both sides look like flowing prose.
                if columns_have_prose(&result, &page_items) {
                    debug!(
                        "page {}: relative valley detection found {} columns",
                        page,
                        result.len()
                    );
                    return result;
                } else {
                    debug!(
                        "page {}: relative valley rejected — columns lack prose density",
                        page,
                    );
                }
            }
        }
        // Try XY-cut fallback before giving up
        if let Some(columns) = try_xy_cut_split(&page_items, x_min, x_max, page) {
            return columns;
        }
        return vec![ColumnRegion { x_min, x_max }];
    }

    // Try center-based assignment first (handles asymmetric layouts / sidebars
    // better than edge-based). Fall back to edge-based if center produces
    // a degenerate split (one side empty).
    let result = validate_and_build_columns(
        &valleys,
        &page_items,
        x_min,
        BIN_WIDTH,
        x_max,
        MIN_ITEMS_PER_COLUMN,
        MIN_VERTICAL_SPAN_RATIO,
        page,
        true, // center-based assignment
    );
    if result.len() > 1 {
        return result;
    }
    let result = validate_and_build_columns(
        &valleys,
        &page_items,
        x_min,
        BIN_WIDTH,
        x_max,
        MIN_ITEMS_PER_COLUMN,
        MIN_VERTICAL_SPAN_RATIO,
        page,
        false, // edge-based fallback
    );
    if result.len() > 1 {
        return result;
    }

    // Fallback: XY-cut style gap detection.  When the histogram finds no
    // clear valleys (common with asymmetric/sidebar layouts), look for the
    // largest horizontal gap between item edges.  This is a simplified
    // single-level XY-cut inspired by opendataloader's XY-Cut++ algorithm.
    if page_items.len() >= 20 && !page_has_table {
        if let Some(columns) = try_xy_cut_split(&page_items, x_min, x_max, page) {
            return columns;
        }
    }

    vec![ColumnRegion { x_min, x_max }]
}

/// Simplified single-level XY-cut: find the largest horizontal gap between
/// item right-edges and left-edges.  If the gap is wide enough and both sides
/// have sufficient items with vertical overlap, split into two columns.
///
/// Inspired by opendataloader's XY-Cut++ algorithm but without full recursion.
/// Handles asymmetric layouts (sidebars) that the histogram misses because
/// the narrow column has too few items to register in the occupancy profile.
fn try_xy_cut_split(
    page_items: &[&TextItem],
    page_x_min: f32,
    page_x_max: f32,
    page: u32,
) -> Option<Vec<ColumnRegion>> {
    const MIN_GAP: f32 = 15.0; // minimum gap to consider a split
    const MIN_ITEMS_MAJOR: usize = 10; // major column must have ≥10 items
    const MIN_ITEMS_MINOR: usize = 3; // minor column (sidebar) must have ≥3

    let page_width = page_x_max - page_x_min;
    if page_width < 200.0 {
        return None;
    }

    // Collect all item edges: (right_edge, left_edge) pairs sorted by right_edge
    // The gap between one item's right edge and the next item's left edge
    // reveals column gutters.
    let mut edges: Vec<(f32, f32)> = page_items
        .iter()
        .map(|i| (i.x, i.x + effective_width(i)))
        .collect();
    edges.sort_by(|a, b| a.0.total_cmp(&b.0));

    // Find the largest gap between consecutive items (by left edge).
    // Use a sweep: sort left edges, find max gap between sorted right edges
    // of items to the left and left edges of items to the right.
    let mut left_edges: Vec<f32> = page_items.iter().map(|i| i.x).collect();
    left_edges.sort_by(|a, b| a.total_cmp(b));

    // Build prefix max of right edges (for items sorted by left edge)
    let mut sorted_by_left: Vec<(f32, f32)> = page_items
        .iter()
        .map(|i| (i.x, i.x + effective_width(i)))
        .collect();
    sorted_by_left.sort_by(|a, b| a.0.total_cmp(&b.0));

    let mut best_gap = 0.0f32;
    let mut best_split = 0.0f32;
    let mut max_right_so_far = f32::NEG_INFINITY;

    for i in 0..sorted_by_left.len() - 1 {
        let (_, right) = sorted_by_left[i];
        max_right_so_far = max_right_so_far.max(right);

        let (next_left, _) = sorted_by_left[i + 1];
        let gap = next_left - max_right_so_far;
        if gap > best_gap {
            best_gap = gap;
            best_split = (max_right_so_far + next_left) / 2.0;
        }
    }

    if best_gap < MIN_GAP {
        return None;
    }

    // Don't split at page margins (within 10% of edges)
    let margin = page_width * 0.10;
    if best_split - page_x_min < margin || page_x_max - best_split < margin {
        return None;
    }

    // Count items on each side
    let left_count = page_items
        .iter()
        .filter(|i| i.x + effective_width(i) / 2.0 <= best_split)
        .count();
    let right_count = page_items
        .iter()
        .filter(|i| i.x + effective_width(i) / 2.0 > best_split)
        .count();

    let (minor, major) = if left_count <= right_count {
        (left_count, right_count)
    } else {
        (right_count, left_count)
    };

    if major < MIN_ITEMS_MAJOR || minor < MIN_ITEMS_MINOR {
        return None;
    }

    // Check vertical overlap — both sides should span a meaningful Y range
    let left_items: Vec<&&TextItem> = page_items
        .iter()
        .filter(|i| i.x + effective_width(i) / 2.0 <= best_split)
        .collect();
    let right_items: Vec<&&TextItem> = page_items
        .iter()
        .filter(|i| i.x + effective_width(i) / 2.0 > best_split)
        .collect();

    let l_y_min = left_items.iter().map(|i| i.y).fold(f32::INFINITY, f32::min);
    let l_y_max = left_items
        .iter()
        .map(|i| i.y)
        .fold(f32::NEG_INFINITY, f32::max);
    let r_y_min = right_items
        .iter()
        .map(|i| i.y)
        .fold(f32::INFINITY, f32::min);
    let r_y_max = right_items
        .iter()
        .map(|i| i.y)
        .fold(f32::NEG_INFINITY, f32::max);

    let overlap_min = l_y_min.max(r_y_min);
    let overlap_max = l_y_max.min(r_y_max);
    let overlap = (overlap_max - overlap_min).max(0.0);
    let y_range = (l_y_max.max(r_y_max) - l_y_min.min(r_y_min)).max(1.0);

    if overlap / y_range < 0.20 {
        return None;
    }

    debug!(
        "page {}: XY-cut split at x={:.1} (gap={:.1}pt, left={}, right={})",
        page, best_split, best_gap, left_count, right_count
    );

    Some(vec![
        ColumnRegion {
            x_min: page_x_min,
            x_max: best_split,
        },
        ColumnRegion {
            x_min: best_split,
            x_max: page_x_max,
        },
    ])
}

/// Check whether each proposed column contains paragraph-like content.
///
/// Groups items per column into rough lines by Y-proximity, then measures
/// what fraction of those lines span a significant portion of the column
/// width. Two-column prose (justified or ragged-right) produces lines that
/// fill most of the column width. Tables, forms, and checklists produce
/// short scattered items that don't.
///
/// Returns true only when *every* column passes a minimum prose density.
fn columns_have_prose(columns: &[ColumnRegion], items: &[&TextItem]) -> bool {
    const Y_TOL: f32 = 3.0; // y-proximity to group items into the same line
    const LINE_FILL_THRESHOLD: f32 = 0.45; // line must span ≥45% of column width
    const MIN_PROSE_RATIO: f32 = 0.40; // ≥40% of lines must be "full"
    const MIN_LINES: usize = 8; // need enough lines to judge
    const MIN_COL_WIDTH: f32 = 120.0; // columns must be ≥120pt (not narrow sidebars/fragments)
    const MAX_AVG_ITEMS_PER_LINE: f32 = 3.5; // prose has 1-3 items/line; tables/forms have 4+

    for col in columns {
        let col_width = col.x_max - col.x_min;
        if col_width < MIN_COL_WIDTH {
            return false;
        }

        // Collect items whose center falls within this column
        let col_items: Vec<&TextItem> = items
            .iter()
            .filter(|i| {
                let center = i.x + effective_width(i) / 2.0;
                center >= col.x_min && center <= col.x_max
            })
            .copied()
            .collect();

        if col_items.len() < MIN_LINES {
            return false;
        }

        // Sort by Y descending (top of page = higher Y in PDF coords)
        let mut sorted: Vec<&TextItem> = col_items;
        sorted.sort_by(|a, b| b.y.total_cmp(&a.y));

        // Group into lines by Y-proximity and measure fill + item count
        let mut full_lines = 0usize;
        let mut total_lines = 0usize;
        let mut total_items_in_lines = 0usize;
        let mut line_items: Vec<&TextItem> = Vec::new();
        let mut line_y = f32::NAN;

        let flush_line = |line_items: &[&TextItem],
                          full: &mut usize,
                          total: &mut usize,
                          total_items: &mut usize| {
            if line_items.is_empty() {
                return;
            }
            *total += 1;
            *total_items += line_items.len();
            // Compute the span of text on this line within the column
            let left = line_items
                .iter()
                .map(|i| i.x.max(col.x_min))
                .fold(f32::INFINITY, f32::min);
            let right = line_items
                .iter()
                .map(|i| (i.x + effective_width(i)).min(col.x_max))
                .fold(f32::NEG_INFINITY, f32::max);
            let span = (right - left).max(0.0);
            if span >= col_width * LINE_FILL_THRESHOLD {
                *full += 1;
            }
        };

        for item in &sorted {
            if line_items.is_empty() || (line_y - item.y).abs() < Y_TOL {
                if line_items.is_empty() {
                    line_y = item.y;
                }
                line_items.push(item);
            } else {
                flush_line(
                    &line_items,
                    &mut full_lines,
                    &mut total_lines,
                    &mut total_items_in_lines,
                );
                line_items.clear();
                line_y = item.y;
                line_items.push(item);
            }
        }
        flush_line(
            &line_items,
            &mut full_lines,
            &mut total_lines,
            &mut total_items_in_lines,
        );

        if total_lines < MIN_LINES {
            return false;
        }

        let ratio = full_lines as f32 / total_lines as f32;
        let avg_items = total_items_in_lines as f32 / total_lines as f32;
        debug!(
            "columns_have_prose: col [{:.0}..{:.0}] lines={} full={} ratio={:.2} avg_items={:.1}",
            col.x_min, col.x_max, total_lines, full_lines, ratio, avg_items
        );
        if ratio < MIN_PROSE_RATIO {
            return false;
        }
        // Tables and forms tend to have many small items per line (one per cell),
        // while prose has few items per line (one per word-run or phrase).
        if avg_items > MAX_AVG_ITEMS_PER_LINE {
            return false;
        }
    }

    true
}

/// Find relative valleys (local minima) in the histogram.
///
/// When justified text fills gutters, the absolute noise threshold fails.
/// This finds local minima where the count drops significantly below
/// the peaks on either side — indicating a gutter even when not empty.
fn find_relative_valleys(
    histogram: &[u32],
    num_bins: usize,
    _x_min: f32,
    bin_width: f32,
    page_width: f32,
    margin_threshold: f32,
) -> Vec<(usize, usize)> {
    const MIN_GUTTER_BINS: usize = 2; // minimum 4pt gutter
    const CONTRAST_THRESHOLD: f32 = 0.60; // valley must be < 60% of surrounding peaks
    const PEAK_WINDOW: usize = 25; // look 50pt on each side for peaks
    const MIN_PEAK_HEIGHT: f32 = 20.0; // peaks must be ≥20 (dense text columns)

    if num_bins < 10 {
        return vec![];
    }

    // Smooth histogram with a 5-bin moving average to reduce noise
    let mut smoothed = vec![0.0f32; num_bins];
    let half_win = 2usize;
    for (i, s) in smoothed.iter_mut().enumerate().take(num_bins) {
        let lo = i.saturating_sub(half_win);
        let hi = (i + half_win + 1).min(num_bins);
        let sum: u32 = histogram[lo..hi].iter().sum();
        *s = sum as f32 / (hi - lo) as f32;
    }

    // Find local minima: positions where smoothed value is lower than
    // both sides within a search window
    let mut candidates: Vec<(usize, f32, f32)> = Vec::new(); // (bin, valley_val, contrast)

    for i in PEAK_WINDOW..num_bins.saturating_sub(PEAK_WINDOW) {
        let val = smoothed[i];
        if val < 1.0 {
            continue; // skip empty margins
        }

        // Check this is a local minimum within a small window
        let local_lo = i.saturating_sub(3);
        let local_hi = (i + 4).min(num_bins);
        let is_local_min = (local_lo..local_hi).all(|j| smoothed[j] >= val - 0.5);
        if !is_local_min {
            continue;
        }

        // Find peak values on each side
        let left_peak = smoothed[i.saturating_sub(PEAK_WINDOW)..i]
            .iter()
            .cloned()
            .fold(0.0f32, f32::max);
        let right_peak = smoothed[(i + 1)..(i + 1 + PEAK_WINDOW).min(num_bins)]
            .iter()
            .cloned()
            .fold(0.0f32, f32::max);

        if left_peak < MIN_PEAK_HEIGHT || right_peak < MIN_PEAK_HEIGHT {
            continue;
        }

        // Both peaks must be substantial — prevents detecting margin drop-offs
        // as gutters in single-column layouts with ragged text.
        let peak_balance = left_peak.min(right_peak) / left_peak.max(right_peak);
        if peak_balance < 0.40 {
            continue;
        }

        // Contrast: ratio of valley to the smaller of the two peaks
        let ref_peak = left_peak.min(right_peak);
        let contrast = val / ref_peak;

        if contrast < CONTRAST_THRESHOLD {
            // Check margin constraint
            let center_pts = i as f32 * bin_width;
            if center_pts > margin_threshold && center_pts < (page_width - margin_threshold) {
                candidates.push((i, val, contrast));
            }
        }
    }

    if candidates.is_empty() {
        return vec![];
    }

    // Group adjacent candidates into valley ranges and pick the deepest point
    let mut valleys: Vec<(usize, usize)> = Vec::new();
    let mut best_bin = candidates[0].0;
    let mut best_contrast = candidates[0].2;

    for window in candidates.windows(2) {
        let (prev_bin, _, _) = window[0];
        let (next_bin, _, next_contrast) = window[1];

        if next_bin - prev_bin <= 5 {
            // Same group
            if next_contrast < best_contrast {
                best_bin = next_bin;
                best_contrast = next_contrast;
            }
        } else {
            // End current group
            let half = MIN_GUTTER_BINS;
            valleys.push((
                best_bin.saturating_sub(half),
                (best_bin + half + 1).min(num_bins),
            ));
            best_bin = next_bin;
            best_contrast = next_contrast;
        }
    }
    // Close last group
    let half = MIN_GUTTER_BINS;
    valleys.push((
        best_bin.saturating_sub(half),
        (best_bin + half + 1).min(num_bins),
    ));

    // Limit to the single best valley (deepest contrast).
    // Multi-column layouts with 3+ columns typically have clear gutters that
    // the absolute valley detection handles. The relative fallback is designed
    // for 2-column layouts where justified text fills the gutter.
    if valleys.len() > 1 {
        // Keep only the valley with the best (lowest) contrast in the candidates
        let mut best_idx = 0;
        let mut best_c = f32::MAX;
        for (vi, v) in valleys.iter().enumerate() {
            let mid = (v.0 + v.1) / 2;
            // Find the candidate closest to this valley's midpoint
            if let Some(c) = candidates
                .iter()
                .filter(|(b, _, _)| (*b as isize - mid as isize).unsigned_abs() <= 5)
                .map(|(_, _, c)| *c)
                .reduce(f32::min)
            {
                if c < best_c {
                    best_c = c;
                    best_idx = vi;
                }
            }
        }
        return vec![valleys[best_idx]];
    }

    valleys
}

/// Validate valley candidates with vertical consistency checks and build column regions.
///
/// When `center_assign` is true, items are assigned to columns based on their
/// center point rather than their right edge. This helps when justified text
/// items extend past the gutter.
#[allow(clippy::too_many_arguments)]
fn validate_and_build_columns(
    valleys: &[(usize, usize)],
    page_items: &[&TextItem],
    x_min: f32,
    bin_width: f32,
    x_max: f32,
    min_items: usize,
    min_vertical_span: f32,
    page: u32,
    center_assign: bool,
) -> Vec<ColumnRegion> {
    // Compute Y range of the page
    let y_min = page_items.iter().map(|i| i.y).fold(f32::INFINITY, f32::min);
    let y_max = page_items
        .iter()
        .map(|i| i.y)
        .fold(f32::NEG_INFINITY, f32::max);
    let y_range = y_max - y_min;

    // Validate each valley with vertical consistency
    let mut valid_valleys: Vec<(usize, usize, usize, usize)> = Vec::new();
    for &(start, end) in valleys {
        let gutter_left = x_min + start as f32 * bin_width;
        let gutter_right = x_min + end as f32 * bin_width;
        let gutter_center = (gutter_left + gutter_right) / 2.0;

        // Collect items on each side of the gutter.
        // Center-based: use item midpoint (better for justified text).
        // Edge-based: use item right edge (original behavior).
        let left_items: Vec<&&TextItem> = page_items
            .iter()
            .filter(|i| {
                if center_assign {
                    i.x + effective_width(i) / 2.0 <= gutter_center
                } else {
                    i.x + effective_width(i) <= gutter_center
                }
            })
            .collect();
        let right_items: Vec<&&TextItem> = page_items
            .iter()
            .filter(|i| {
                if center_assign {
                    i.x + effective_width(i) / 2.0 > gutter_center
                } else {
                    i.x >= gutter_center
                }
            })
            .collect();

        // Require both sides to have items. Symmetric layout needs min_items
        // on each side. Asymmetric layouts (sidebars) are accepted when the
        // dominant side has ≥ min_items and the smaller side has ≥ 3 items.
        let (smaller, larger) = if left_items.len() <= right_items.len() {
            (left_items.len(), right_items.len())
        } else {
            (right_items.len(), left_items.len())
        };
        if larger < min_items || smaller < 3 {
            continue;
        }

        // Check vertical overlap
        if y_range > 0.0 {
            let left_y_min = left_items.iter().map(|i| i.y).fold(f32::INFINITY, f32::min);
            let left_y_max = left_items
                .iter()
                .map(|i| i.y)
                .fold(f32::NEG_INFINITY, f32::max);
            let right_y_min = right_items
                .iter()
                .map(|i| i.y)
                .fold(f32::INFINITY, f32::min);
            let right_y_max = right_items
                .iter()
                .map(|i| i.y)
                .fold(f32::NEG_INFINITY, f32::max);

            let overlap_min = left_y_min.max(right_y_min);
            let overlap_max = left_y_max.min(right_y_max);
            let overlap = (overlap_max - overlap_min).max(0.0);

            if overlap / y_range < min_vertical_span {
                continue;
            }
        }

        valid_valleys.push((start, end, left_items.len(), right_items.len()));
    }

    if valid_valleys.is_empty() {
        debug!(
            "page {}: {} valleys found but none passed validation",
            page,
            valleys.len()
        );
        return vec![ColumnRegion { x_min, x_max }];
    }

    debug!(
        "page {}: {} columns detected (boundaries: {:?})",
        page,
        valid_valleys.len() + 1,
        valid_valleys
            .iter()
            .map(|(s, e, _, _)| x_min + ((*s + *e) as f32 / 2.0) * bin_width)
            .collect::<Vec<_>>()
    );

    // Limit to at most 3 gutters (4 columns).
    // Score = width_in_bins * min(left_count, right_count)
    if valid_valleys.len() > 3 {
        valid_valleys.sort_by(|a, b| {
            let score_a = (a.1 - a.0) as f32 * (a.2.min(a.3) as f32);
            let score_b = (b.1 - b.0) as f32 * (b.2.min(b.3) as f32);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        valid_valleys.truncate(3);
        valid_valleys.sort_by_key(|v| v.0);
    }

    // Build column regions from gutter boundaries
    let mut columns = Vec::new();
    let mut col_start = x_min;
    for &(start, end, _, _) in &valid_valleys {
        let gutter_center = x_min + ((start + end) as f32 / 2.0) * bin_width;
        columns.push(ColumnRegion {
            x_min: col_start,
            x_max: gutter_center,
        });
        col_start = gutter_center;
    }
    columns.push(ColumnRegion {
        x_min: col_start,
        x_max,
    });

    columns
}
