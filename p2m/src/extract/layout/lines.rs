//! Line grouping and reading-order determination.

use std::collections::{HashMap, HashSet};

use log::debug;

use super::columns::{ColumnRegion, detect_columns};
use super::spanning::{identify_spanning_lines, is_page_number, spans_multiple_columns};
use crate::text::bidi::sort_line_items;
use crate::text::unicode::effective_width;
use crate::types::{TextItem, TextLine};

/// Group text items into lines, with multi-column support
/// Detect newspaper-style columns: independent text flows that should be read
/// sequentially (all of col1, then col2) rather than Y-interleaved.
pub(crate) fn is_newspaper_layout(
    per_column_lines: &[Vec<TextLine>],
    columns: &[ColumnRegion],
) -> bool {
    if per_column_lines.len() < 2 {
        return false;
    }

    // Each column must independently have substantial content
    let min_lines = per_column_lines.iter().map(|c| c.len()).min().unwrap_or(0);
    let max_lines = per_column_lines.iter().map(|c| c.len()).max().unwrap_or(0);

    if min_lines < 5 {
        return false;
    }

    if min_lines < 15 {
        // Sidebar detection: a narrow annotation column beside a wide body column.
        // Guards:
        //   - Only 2 columns (sidebars are body+sidebar, not 3+ columns)
        //   - width_ratio < 0.50: sidebar is much narrower than body
        //   - line_balance < 0.35: sidebar has significantly fewer lines
        //   - max_lines >= 20: body column has substantial prose content
        //   - narrower column has fewer lines (not a dense reference column)
        if columns.len() == 2 && per_column_lines.len() == 2 {
            let w0 = columns[0].x_max - columns[0].x_min;
            let w1 = columns[1].x_max - columns[1].x_min;
            let width_ratio = w0.min(w1) / w0.max(w1);
            let line_balance = if max_lines > 0 {
                min_lines as f32 / max_lines as f32
            } else {
                1.0
            };
            let narrow_width = w0.min(w1);
            if width_ratio < 0.50 && line_balance < 0.35 && max_lines >= 20 && narrow_width >= 160.0
            {
                let narrower_idx = if w0 < w1 { 0 } else { 1 };
                let fewest_idx = if per_column_lines[0].len() <= per_column_lines[1].len() {
                    0
                } else {
                    1
                };
                if narrower_idx == fewest_idx {
                    // Sparse density check: sidebar annotations are spread thinly
                    // across the page height while regular two-column text is dense.
                    // Compare average Y-gap between successive lines in each column.
                    let narrow = &per_column_lines[narrower_idx];
                    let wide = &per_column_lines[1 - narrower_idx];
                    let avg_gap = |lines: &[TextLine]| -> f32 {
                        if lines.len() < 2 {
                            return 0.0;
                        }
                        let mut ys: Vec<f32> = lines.iter().map(|l| l.y).collect();
                        ys.sort_by(|a, b| a.total_cmp(b));
                        let span = ys.last().unwrap() - ys.first().unwrap();
                        span / (lines.len() as f32 - 1.0)
                    };
                    let narrow_gap = avg_gap(narrow);
                    let wide_gap = avg_gap(wide);
                    // Sidebar annotations have >2.5x the average gap of body text
                    if wide_gap > 0.0 && narrow_gap / wide_gap >= 2.5 {
                        return true;
                    }
                }
            }
        }
        return false;
    }

    // Dense balanced columns (similar line counts) are newspaper regardless of Y-alignment.
    // By this point table items are already removed, so two dense balanced columns
    // of remaining text are independent prose flows.
    let balance_ratio = min_lines as f32 / max_lines as f32;
    if balance_ratio > 0.7 {
        return true;
    }

    // For unbalanced columns, fall back to Y-collision check
    let y_tol = 5.0; // was 3.0 — handles government gazette typesetting variance
    let (smallest_idx, _) = per_column_lines
        .iter()
        .enumerate()
        .min_by_key(|(_, c)| c.len())
        .unwrap();

    let smallest = &per_column_lines[smallest_idx];
    let mut collisions = 0u32;
    for line in smallest {
        for (ci, col) in per_column_lines.iter().enumerate() {
            if ci == smallest_idx {
                continue;
            }
            if col.iter().any(|ol| (ol.y - line.y).abs() < y_tol) {
                collisions += 1;
                break;
            }
        }
    }

    let ratio = collisions as f32 / smallest.len() as f32;
    ratio > 0.5
}

/// Split column lines into a core cluster and stragglers.
/// The core is the largest group of consecutive lines separated by normal
/// line spacing. Lines in other groups (header remnants, per-word items from
/// full-width lines) are returned as stragglers.
fn split_column_stragglers(lines: Vec<TextLine>) -> (Vec<TextLine>, Vec<TextLine>) {
    if lines.len() < 3 {
        return (lines, Vec::new());
    }

    // Lines are sorted Y descending (top-first). Compute gaps.
    let mut gaps: Vec<f32> = Vec::new();
    for i in 0..lines.len() - 1 {
        gaps.push(lines[i].y - lines[i + 1].y);
    }

    // Median gap = typical line spacing
    let mut sorted_gaps = gaps.clone();
    sorted_gaps.sort_by(|a, b| a.total_cmp(b));
    let median_gap = sorted_gaps[sorted_gaps.len() / 2];

    // A gap > 3× median (min 30pt) indicates a break between content clusters
    let threshold = (median_gap * 3.0).max(30.0);

    // Find all split points
    let mut split_indices: Vec<usize> = Vec::new();
    for (i, &gap) in gaps.iter().enumerate() {
        if gap > threshold {
            split_indices.push(i);
        }
    }

    if split_indices.is_empty() {
        return (lines, Vec::new());
    }

    // Build segments: (start_line_idx, end_line_idx_exclusive)
    let mut segments: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for &si in &split_indices {
        segments.push((start, si + 1));
        start = si + 1;
    }
    segments.push((start, lines.len()));

    // Find the largest segment (the core cluster)
    let (core_seg, _) = segments
        .iter()
        .enumerate()
        .max_by_key(|(_, (s, e))| e - s)
        .unwrap();

    let (cs, ce) = segments[core_seg];
    let mut core = Vec::with_capacity(ce - cs);
    let mut stragglers = Vec::new();
    for (i, line) in lines.into_iter().enumerate() {
        if i >= cs && i < ce {
            core.push(line);
        } else {
            stragglers.push(line);
        }
    }

    (core, stragglers)
}

pub fn group_into_lines(items: Vec<TextItem>) -> Vec<TextLine> {
    group_into_lines_with_thresholds(items, &HashMap::new(), &HashSet::new())
}

/// Group text items into lines, using pre-computed per-page adaptive thresholds
/// from Canva-style letter-spacing detection. Falls back to computing the
/// threshold from item gaps when no pre-computed value is available.
pub(crate) fn group_into_lines_with_thresholds(
    items: Vec<TextItem>,
    page_thresholds: &HashMap<u32, f32>,
    table_pages: &HashSet<u32>,
) -> Vec<TextLine> {
    if items.is_empty() {
        return Vec::new();
    }

    // Filter out page numbers (standalone numbers at top/bottom of page)
    let items: Vec<TextItem> = items
        .into_iter()
        .filter(|item| !is_page_number(item))
        .collect();

    // Get unique pages
    let mut pages: Vec<u32> = items.iter().map(|i| i.page.get()).collect();
    pages.sort_unstable();
    pages.dedup();

    let mut all_lines = Vec::new();

    for page in pages {
        let page_num = crate::types::PageNum::new(page);
        let page_items: Vec<TextItem> = items
            .iter()
            .filter(|i| i.page == page_num)
            .cloned()
            .collect();

        // Use pre-computed threshold from fix_letterspaced_items if available
        // (computed before embedded-space removal, with full signal).
        // Non-Canva pages use the default 0.10 threshold.
        let adaptive_threshold = page_thresholds.get(&page).copied().unwrap_or(0.10);

        // Detect columns for this page
        let columns = detect_columns(&page_items, page, table_pages.contains(&page));

        if columns.len() <= 1 {
            // Single column - use simple sorting
            let lines = group_single_column(page_items, adaptive_threshold);
            all_lines.extend(lines);
        } else {
            // Multi-column detected. Pre-mask lines that span the full page
            // width (titles, section headers, footers). These multi-item lines
            // would otherwise be split across column buckets, corrupting
            // newspaper detection and reading order.
            let spanning_mask = identify_spanning_lines(&page_items, &columns);
            let premasked_count = spanning_mask.iter().filter(|&&m| m).count();
            if premasked_count > 0 {
                debug!(
                    "page {}: pre-masked {} spanning-line items",
                    page, premasked_count
                );
            }

            // Partition items preserving original order
            let mut spanning_items: Vec<TextItem> = Vec::new();
            let mut column_items: Vec<TextItem> = Vec::new();

            for (i, item) in page_items.into_iter().enumerate() {
                if spanning_mask[i] || spans_multiple_columns(&item, &columns) {
                    spanning_items.push(item);
                } else {
                    column_items.push(item);
                }
            }

            // Process each column's items independently, preserving column identity.
            // Assign each item to the column with greatest horizontal overlap
            // (instead of center-point) to avoid gutter mis-assignment.
            let mut col_buckets: Vec<Vec<TextItem>> = vec![Vec::new(); columns.len()];
            for item in &column_items {
                let item_left = item.x;
                let item_right = item.x + effective_width(item);
                let mut best_col = 0;
                let mut best_overlap = f32::NEG_INFINITY;
                for (ci, col) in columns.iter().enumerate() {
                    let overlap = (item_right.min(col.x_max) - item_left.max(col.x_min)).max(0.0);
                    if overlap > best_overlap {
                        best_overlap = overlap;
                        best_col = ci;
                    }
                }
                col_buckets[best_col].push(item.clone());
            }

            debug!(
                "page {}: {} columns, {} spanning items",
                page,
                columns.len(),
                spanning_items.len()
            );
            for (ci, col) in columns.iter().enumerate() {
                debug!(
                    "  col {}: x=[{:.0}..{:.0}] {} items",
                    ci,
                    col.x_min,
                    col.x_max,
                    col_buckets[ci].len()
                );
            }
            if log::log_enabled!(log::Level::Trace) {
                for (ci, bucket) in col_buckets.iter().enumerate() {
                    for item in bucket {
                        log::trace!(
                            "  col {} <- x={:7.1} y={:7.1} {:?}",
                            ci,
                            item.x,
                            item.y,
                            if item.text.len() > 60 {
                                &item.text[..60]
                            } else {
                                &item.text
                            }
                        );
                    }
                }
            }

            let mut per_column_lines: Vec<Vec<TextLine>> = Vec::new();
            for col_items in col_buckets {
                let lines = group_single_column(col_items, adaptive_threshold);
                per_column_lines.push(lines);
            }

            // Process spanning items as their own group
            let spanning_lines = group_single_column(spanning_items, adaptive_threshold);

            let is_newspaper = is_newspaper_layout(&per_column_lines, &columns);
            debug!(
                "page {}: layout={}",
                page,
                if is_newspaper { "newspaper" } else { "tabular" }
            );

            if is_newspaper {
                // Newspaper: columns are independent text flows.
                // 1. Split each column into its densest cluster (core) and stragglers
                // 2. Use core columns to determine the above/below threshold
                // 3. Emit: above items → core columns sequentially → below items
                let mut core_columns: Vec<Vec<TextLine>> = Vec::new();
                let mut col_stragglers: Vec<Vec<TextLine>> = Vec::new();
                for col in per_column_lines {
                    let (core, stragglers) = split_column_stragglers(col);
                    core_columns.push(core);
                    col_stragglers.push(stragglers);
                }

                // col_top = min of max Y across core columns
                let col_top = core_columns
                    .iter()
                    .filter(|c| !c.is_empty())
                    .map(|c| c.iter().map(|l| l.y).fold(f32::NEG_INFINITY, f32::max))
                    .fold(f32::INFINITY, f32::min);
                let margin = 5.0;

                let mut above: Vec<TextLine> = Vec::new();
                let mut below_spanning: Vec<TextLine> = Vec::new();

                // Spanning items: above or below the column region
                for line in spanning_lines {
                    if line.y > col_top + margin {
                        above.push(line);
                    } else {
                        below_spanning.push(line);
                    }
                }

                // Column stragglers above col_top go to "above";
                // below col_top they stay with their column to avoid
                // re-interleaving when sorted by Y.
                let mut col_below: Vec<Vec<TextLine>> = vec![Vec::new(); core_columns.len()];
                for (ci, stragglers) in col_stragglers.into_iter().enumerate() {
                    for line in stragglers {
                        if line.y > col_top + margin {
                            above.push(line);
                        } else {
                            col_below[ci].push(line);
                        }
                    }
                }

                above.sort_by(|a, b| b.y.total_cmp(&a.y));
                below_spanning.sort_by(|a, b| b.y.total_cmp(&a.y));

                all_lines.extend(above);
                for col in core_columns {
                    all_lines.extend(col);
                }
                for cb in col_below {
                    all_lines.extend(cb);
                }
                all_lines.extend(below_spanning);
            } else {
                // Tabular: Y-interleaved merge — rows at the same Y from
                // different columns form a single logical line.
                let mut all_page_lines: Vec<TextLine> = Vec::new();
                all_page_lines.extend(spanning_lines);
                for col_lines in per_column_lines {
                    all_page_lines.extend(col_lines);
                }

                // Sort by Y descending (top-first), then by X for same-Y lines
                all_page_lines.sort_by(|a, b| {
                    b.y.total_cmp(&a.y).then(
                        a.items
                            .first()
                            .map(|i| i.x)
                            .unwrap_or(0.0)
                            .total_cmp(&b.items.first().map(|i| i.x).unwrap_or(0.0)),
                    )
                });

                // Merge lines at the same Y (within tolerance) into single lines
                let y_tol = 3.0;
                let mut merged: Vec<TextLine> = Vec::new();
                for line in all_page_lines {
                    if let Some(last) = merged.last_mut() {
                        if last.page == line.page && (last.y - line.y).abs() < y_tol {
                            last.items.extend(line.items);
                            sort_line_items(&mut last.items);
                            continue;
                        }
                    }
                    merged.push(line);
                }

                all_lines.extend(merged);
            }
        }
    }

    all_lines
}

/// Determine if Y-sorting should be used instead of stream order.
/// Returns true if the stream order appears chaotic (items jump around in Y position).
fn should_use_y_sorting(items: &[TextItem]) -> bool {
    if items.len() < 5 {
        return false; // Not enough items to judge
    }

    // Sample Y positions from stream order
    let y_positions: Vec<f32> = items.iter().map(|i| i.y).collect();

    // Count "order violations" - cases where Y increases (going up) when it should decrease
    // In proper reading order, Y should generally decrease (top to bottom)
    let mut large_jumps_up = 0;
    let mut large_jumps_down = 0;
    let jump_threshold = 50.0; // Significant Y jump

    for window in y_positions.windows(2) {
        let delta = window[1] - window[0];
        if delta > jump_threshold {
            large_jumps_up += 1; // Y increased significantly (jumped up on page)
        } else if delta < -jump_threshold {
            large_jumps_down += 1; // Y decreased significantly (normal reading direction)
        }
    }

    // If there are many upward jumps relative to downward jumps, order is chaotic
    // A well-ordered document should have mostly downward progression
    let total_jumps = large_jumps_up + large_jumps_down;
    if total_jumps < 3 {
        return false; // Not enough jumps to judge
    }

    // If more than 40% of large jumps are upward, use Y-sorting
    let chaos_ratio = large_jumps_up as f32 / total_jumps as f32;
    chaos_ratio > 0.4
}

/// Group items from a single column into lines
/// Uses heuristics to decide between PDF stream order and Y-position sorting.
fn group_single_column(items: Vec<TextItem>, adaptive_threshold: f32) -> Vec<TextLine> {
    if items.is_empty() {
        return Vec::new();
    }

    // Decide whether to use stream order or Y-sorting
    let use_y_sorting = should_use_y_sorting(&items);

    let items = if use_y_sorting {
        // Sort by Y descending (top to bottom in PDF coords)
        let mut sorted = items;
        sorted.sort_by(|a, b| b.y.total_cmp(&a.y).then(a.x.total_cmp(&b.x)));
        sorted
    } else {
        items
    };

    // Group items into lines
    let mut lines: Vec<TextLine> = Vec::new();
    let y_tolerance = 3.0;

    for item in items {
        // Only check the most recent line for merging
        let should_merge = lines.last().is_some_and(|last_line| {
            if last_line.page != item.page {
                return false;
            }
            let y_diff = (last_line.y - item.y).abs();
            if y_diff >= y_tolerance {
                return false;
            }
            // Check if this looks like a new line despite similar Y:
            // If items are at the same X position (left margin) but different Y,
            // they're vertically stacked lines, not the same line
            let has_y_change = y_diff > 0.5;
            if has_y_change {
                if let Some(first_item) = last_line.items.first() {
                    let at_same_x = (item.x - first_item.x).abs() < 5.0;
                    // If at same X (left margin) with Y change, it's likely a new line
                    if at_same_x {
                        return false;
                    }
                    // If new item starts significantly to the left with Y change,
                    // it's a new line (not just out-of-order items on same line)
                    if let Some(last_item) = last_line.items.last() {
                        if item.x < last_item.x - 10.0 {
                            return false;
                        }
                    }
                }
            }
            true
        });

        if should_merge {
            // Add to the most recent line
            lines.last_mut().unwrap().items.push(item);
        } else {
            // Create new line
            let y = item.y;
            let page = item.page;
            lines.push(TextLine {
                items: vec![item],
                y,
                page,
                adaptive_threshold,
            });
        }
    }

    // Sort items within each line by X position (direction-aware)
    for line in &mut lines {
        sort_line_items(&mut line.items);
    }

    debug!("group_single_column: {} lines", lines.len());

    lines
}
