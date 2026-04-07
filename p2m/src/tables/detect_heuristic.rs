//! Heuristic table detection and validation.
//!
//! Detects tables without PDF graphics (rects/lines) by analysing X-position
//! clusters (columns), Y-position clusters (rows), and content patterns.
//! Two passes: small-font items first, then body-font items.

use std::collections::{HashMap, HashSet};

use log::debug;

use crate::text::bidi::is_rtl_text;
use crate::types::{PageNum, TextItem};

use super::financial::try_split_financial_item;
use super::Table;

/// Detection mode controls validation thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    SmallFont,
    BodyFont,
}

/// Merge adjacent single-char items into words (handles per-glyph PDFs).
fn merge_adjacent_items(items: &[TextItem]) -> (Vec<TextItem>, Vec<Vec<usize>>) {
    if items.is_empty() {
        return (vec![], vec![]);
    }

    let y_tolerance = 5.0;
    let mut line_groups: Vec<(f32, Vec<(usize, &TextItem)>)> = Vec::new();

    for (idx, item) in items.iter().enumerate() {
        let found = line_groups
            .iter_mut()
            .find(|(y, _)| (item.y - *y).abs() < y_tolerance);
        if let Some((_, group)) = found {
            group.push((idx, item));
        } else {
            line_groups.push((item.y, vec![(idx, item)]));
        }
    }

    for (_, group) in &mut line_groups {
        group.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
    }
    line_groups.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut merged_items = Vec::new();
    let mut index_map: Vec<Vec<usize>> = Vec::new();

    for (_, group) in &line_groups {
        let mut i = 0;
        while i < group.len() {
            let (first_idx, first_item) = group[i];
            let mut text = first_item.text.clone();
            let mut end_x = first_item.x + first_item.width;
            let mut indices = vec![first_idx];
            let x_gap_max = first_item.font_size * 0.5;

            let mut j = i + 1;
            while j < group.len() {
                let (next_idx, next_item) = group[j];
                if (next_item.font_size - first_item.font_size).abs()
                    > first_item.font_size * 0.20
                {
                    break;
                }
                let gap = next_item.x - end_x;
                if gap > x_gap_max {
                    break;
                }
                if gap < -first_item.font_size * 0.5 {
                    break;
                }
                if gap > first_item.font_size * 0.08 {
                    text.push(' ');
                }
                text.push_str(&next_item.text);
                end_x = next_item.x + next_item.width;
                indices.push(next_idx);
                j += 1;
            }

            merged_items.push(TextItem {
                text,
                x: first_item.x,
                y: first_item.y,
                width: end_x - first_item.x,
                height: first_item.height,
                font: first_item.font.clone(),
                font_size: first_item.font_size,
                page: first_item.page,
                bold: first_item.bold,
                italic: first_item.italic,
                kind: first_item.kind.clone(),
                mcid: first_item.mcid,
            });
            index_map.push(indices);
            i = j;
        }
    }

    (merged_items, index_map)
}

/// Expand consolidated financial items (e.g. `"$ 1,234 $ 5,678"` → sub-items).
fn expand_consolidated_items(items: &[TextItem]) -> (Vec<TextItem>, Vec<usize>) {
    let mut expanded = Vec::with_capacity(items.len());
    let mut index_map = Vec::with_capacity(items.len());
    for (orig_idx, item) in items.iter().enumerate() {
        if let Some(sub_items) = try_split_financial_item(item) {
            for sub in sub_items {
                expanded.push(sub);
                index_map.push(orig_idx);
            }
        } else {
            expanded.push(item.clone());
            index_map.push(orig_idx);
        }
    }
    (expanded, index_map)
}

/// Main entry: detect heuristic tables for a single page.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn detect_heuristic_tables(
    items: &[TextItem],
    base_font_size: f32,
    page: PageNum,
) -> Vec<Table> {
    let page_items: Vec<&TextItem> = items.iter().filter(|i| i.page == page).collect();
    if page_items.len() < 6 {
        return vec![];
    }

    let owned: Vec<TextItem> = page_items.into_iter().cloned().collect();
    let (merged_items, _merge_map) = merge_adjacent_items(&owned);
    let (expanded_items, _expand_map) = expand_consolidated_items(&merged_items);
    let items_ref = &expanded_items[..];

    let mut tables = Vec::new();
    let mut claimed_indices: HashSet<usize> = HashSet::new();

    // Pass 1: Small-font tables
    let table_font_threshold = base_font_size * 0.90;
    let table_candidates: Vec<(usize, &TextItem)> = items_ref
        .iter()
        .enumerate()
        .filter(|(_, item)| item.font_size <= table_font_threshold && item.font_size >= 6.0)
        .collect();

    if table_candidates.len() >= 6 {
        let regions = find_table_regions(&table_candidates);
        for (y_min, y_max) in regions {
            let region_items: Vec<(usize, &TextItem)> = table_candidates
                .iter()
                .filter(|(_, item)| item.y >= y_min && item.y <= y_max)
                .cloned()
                .collect();
            if region_items.len() < 6 {
                continue;
            }
            if let Some(table) = detect_table_in_region(&region_items, Mode::SmallFont, page) {
                for (idx, _) in &region_items {
                    claimed_indices.insert(*idx);
                }
                tables.push(table);
            }
        }
    }

    // Pass 2: Body-font tables
    let body_font_low = base_font_size * 0.85;
    let body_font_high = base_font_size * 1.05;
    let body_candidates: Vec<(usize, &TextItem)> = items_ref
        .iter()
        .enumerate()
        .filter(|(idx, item)| {
            !claimed_indices.contains(idx)
                && item.font_size >= body_font_low
                && item.font_size <= body_font_high
                && item.font_size >= 6.0
        })
        .collect();

    if body_candidates.len() >= 6 {
        let regions = find_table_regions_strict(&body_candidates);
        for (y_min, y_max, _, _) in &regions {
            let region_items: Vec<(usize, &TextItem)> = body_candidates
                .iter()
                .filter(|(_, item)| item.y >= *y_min && item.y <= *y_max)
                .cloned()
                .collect();
            if region_items.len() < 6 {
                continue;
            }
            if let Some(table) = detect_table_in_region(&region_items, Mode::BodyFont, page) {
                tables.push(table);
            }
        }
    }

    tables
}

fn find_table_regions(items: &[(usize, &TextItem)]) -> Vec<(f32, f32)> {
    if items.is_empty() {
        return vec![];
    }
    let mut y_positions: Vec<f32> = items.iter().map(|(_, i)| i.y).collect();
    y_positions.sort_by(|a, b| a.total_cmp(b));

    let gap_threshold = 30.0;
    let mut regions = Vec::new();
    let mut region_start = y_positions[0];
    let mut region_end = y_positions[0];
    let mut region_count = 1;

    for &y in &y_positions[1..] {
        if y - region_end > gap_threshold {
            if region_count >= 4 {
                regions.push((region_start - 5.0, region_end + 5.0));
            }
            region_start = y;
            region_end = y;
            region_count = 1;
        } else {
            region_end = y;
            region_count += 1;
        }
    }
    if region_count >= 4 {
        regions.push((region_start - 5.0, region_end + 5.0));
    }
    regions
}

#[allow(clippy::cast_precision_loss)]
fn find_table_regions_strict(items: &[(usize, &TextItem)]) -> Vec<(f32, f32, f32, f32)> {
    if items.is_empty() {
        return vec![];
    }

    let mut row_groups: Vec<(f32, Vec<f32>)> = Vec::new();
    for (_, item) in items {
        let mut found = false;
        for (center, x_positions) in row_groups.iter_mut() {
            if (item.y - *center).abs() < 8.0 {
                x_positions.push(item.x);
                found = true;
                break;
            }
        }
        if !found {
            row_groups.push((item.y, vec![item.x]));
        }
    }

    let mut qualifying_rows: Vec<(f32, Vec<f32>)> = Vec::new();
    for (y, x_positions) in &row_groups {
        let mut sorted_xs = x_positions.clone();
        sorted_xs.sort_by(|a, b| a.total_cmp(b));
        if sorted_xs.is_empty() {
            continue;
        }
        let mut cluster_starts: Vec<f32> = vec![sorted_xs[0]];
        let mut last_x = sorted_xs[0];
        for &x in &sorted_xs[1..] {
            if x - last_x > 20.0 {
                cluster_starts.push(x);
                last_x = x;
            }
        }
        if cluster_starts.len() >= 2 {
            qualifying_rows.push((*y, cluster_starts));
        }
    }

    if qualifying_rows.len() < 3 {
        return vec![];
    }

    qualifying_rows.sort_by(|a, b| a.0.total_cmp(&b.0));

    let max_gap = if qualifying_rows.len() >= 3 {
        let mut gaps: Vec<f32> = qualifying_rows
            .windows(2)
            .map(|w| (w[1].0 - w[0].0).abs())
            .collect();
        gaps.sort_by(|a, b| a.total_cmp(b));
        (gaps[gaps.len() / 2] * 3.0).max(25.0)
    } else {
        25.0
    };

    let mut candidate_regions: Vec<Vec<&(f32, Vec<f32>)>> = Vec::new();
    let mut current_region: Vec<&(f32, Vec<f32>)> = vec![&qualifying_rows[0]];

    for row in qualifying_rows.iter().skip(1) {
        let prev_y = current_region.last().unwrap().0;
        if row.0 - prev_y > max_gap {
            if current_region.len() >= 3 {
                candidate_regions.push(current_region);
            }
            current_region = vec![row];
        } else {
            current_region.push(row);
        }
    }
    if current_region.len() >= 3 {
        candidate_regions.push(current_region);
    }

    let mut regions = Vec::new();
    for region_rows in &candidate_regions {
        let num_rows = region_rows.len();
        let mut total_score = 0.0f32;
        let mut pair_count = 0u32;
        let tolerance = 10.0f32;

        for i in 0..num_rows {
            for j in (i + 1)..num_rows {
                let ca = &region_rows[i].1;
                let cb = &region_rows[j].1;
                let ma = ca.iter().filter(|&&a| cb.iter().any(|&b| (a - b).abs() < tolerance)).count();
                let mb = cb.iter().filter(|&&b| ca.iter().any(|&a| (a - b).abs() < tolerance)).count();
                let max_len = ca.len().max(cb.len());
                if max_len > 0 {
                    total_score += (ma + mb) as f32 / (2 * max_len) as f32;
                    pair_count += 1;
                }
            }
        }

        let avg_score = if pair_count > 0 { total_score / pair_count as f32 } else { 0.0 };
        if avg_score >= 0.5 {
            let y_min = region_rows.first().unwrap().0;
            let y_max = region_rows.last().unwrap().0;
            let x_min = region_rows.iter().flat_map(|(_, c)| c.iter()).copied().fold(f32::INFINITY, f32::min);
            let x_max = region_rows.iter().flat_map(|(_, c)| c.iter()).copied().fold(f32::NEG_INFINITY, f32::max);
            regions.push((y_min - 5.0, y_max + 5.0, x_min - 15.0, x_max + 50.0));
        }
    }
    regions
}

#[allow(clippy::cast_precision_loss)]
fn find_column_boundaries(items: &[(usize, &TextItem)], mode: Mode) -> Vec<f32> {
    let mut x_positions: Vec<f32> = items.iter().map(|(_, i)| i.x).collect();
    x_positions.sort_by(|a, b| a.total_cmp(b));
    if x_positions.is_empty() {
        return vec![];
    }

    let x_range = x_positions.last().unwrap() - x_positions.first().unwrap();
    let avg_gap = if x_positions.len() > 1 {
        x_range / (x_positions.len() - 1) as f32
    } else {
        60.0
    };

    let mut cluster_threshold = avg_gap.clamp(25.0, 50.0);
    let mut use_edge_clustering = false;

    let mut consec_gaps: Vec<f32> = x_positions
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|&g| g > 0.1)
        .collect();

    if consec_gaps.len() > 2 {
        consec_gaps.sort_by(|a, b| a.total_cmp(b));
        let mut best_split = consec_gaps.len() / 2;
        let mut best_jump = 0.0f32;
        let min_side = 3.min(consec_gaps.len() / 2);
        for i in 0..consec_gaps.len().saturating_sub(1) {
            let left = i + 1;
            let right = consec_gaps.len() - i - 1;
            if left < min_side || right < min_side {
                continue;
            }
            let jump = consec_gaps[i + 1] - consec_gaps[i];
            if jump > best_jump {
                best_jump = jump;
                best_split = i;
            }
        }
        let threshold =
            (consec_gaps[best_split] + consec_gaps[(best_split + 1).min(consec_gaps.len() - 1)]) / 2.0;
        if threshold < 15.0 && best_jump > 2.0 && x_positions.len() > 500 {
            cluster_threshold = threshold.clamp(8.0, 25.0);
            use_edge_clustering = true;
        } else if best_jump > 10.0 && threshold < cluster_threshold {
            cluster_threshold = threshold.max(8.0);
        }
    }

    let mut columns = Vec::new();
    let mut cluster_items: Vec<f32> = vec![x_positions[0]];

    for &x in &x_positions[1..] {
        let reference = if use_edge_clustering {
            *cluster_items.last().unwrap()
        } else {
            cluster_items.iter().sum::<f32>() / cluster_items.len() as f32
        };
        if x - reference > cluster_threshold {
            columns.push(cluster_items.iter().sum::<f32>() / cluster_items.len() as f32);
            cluster_items = vec![x];
        } else {
            cluster_items.push(x);
        }
    }
    if !cluster_items.is_empty() {
        columns.push(cluster_items.iter().sum::<f32>() / cluster_items.len() as f32);
    }

    let min_items_per_col = (items.len() / columns.len().max(1) / 4).max(2);
    let columns: Vec<f32> = columns
        .into_iter()
        .filter(|&col_x| {
            items.iter().filter(|(_, i)| (i.x - col_x).abs() < cluster_threshold).count() >= min_items_per_col
        })
        .collect();

    if mode == Mode::BodyFont {
        let total = items.len();
        for &col_x in &columns {
            let count = items.iter().filter(|(_, i)| (i.x - col_x).abs() < cluster_threshold).count();
            if count as f32 / total as f32 > 0.60 {
                return vec![];
            }
        }
    }

    columns
}

#[allow(clippy::cast_precision_loss)]
fn find_row_boundaries(items: &[(usize, &TextItem)]) -> Vec<f32> {
    let mut y_positions: Vec<f32> = items.iter().map(|(_, i)| i.y).collect();
    y_positions.sort_by(|a, b| b.total_cmp(a));
    if y_positions.is_empty() {
        return vec![];
    }

    let cluster_threshold = {
        let mut font_sizes: Vec<f32> = items.iter().map(|(_, i)| i.font_size).collect();
        font_sizes.sort_by(|a, b| a.total_cmp(b));
        (font_sizes[font_sizes.len() / 2] * 0.8).max(4.0)
    };

    let mut rows = Vec::new();
    let mut cluster_items: Vec<f32> = vec![y_positions[0]];

    for &y in &y_positions[1..] {
        let center = cluster_items.iter().sum::<f32>() / cluster_items.len() as f32;
        if center - y >= cluster_threshold {
            rows.push(center);
            cluster_items = vec![y];
        } else {
            cluster_items.push(y);
        }
    }
    if !cluster_items.is_empty() {
        rows.push(cluster_items.iter().sum::<f32>() / cluster_items.len() as f32);
    }
    rows
}

fn find_column_index(columns: &[f32], x: f32) -> Option<usize> {
    let threshold = if columns.len() >= 2 {
        let min_gap = columns.windows(2).map(|w| (w[1] - w[0]).abs()).fold(f32::INFINITY, f32::min);
        (min_gap / 2.0).clamp(25.0, 50.0)
    } else {
        50.0
    };
    columns
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (x - *a).abs().total_cmp(&(x - *b).abs()))
        .filter(|(_, col_x)| (x - *col_x).abs() < threshold)
        .map(|(idx, _)| idx)
}

fn find_row_index(rows: &[f32], y: f32) -> Option<usize> {
    let threshold = 15.0;
    rows.iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (y - *a).abs().total_cmp(&(y - *b).abs()))
        .filter(|(_, row_y)| (y - *row_y).abs() < threshold)
        .map(|(idx, _)| idx)
}

fn join_cell_items(items: &[&TextItem]) -> String {
    let mut result = String::new();
    for (i, item) in items.iter().enumerate() {
        let text = item.text.trim();
        if text.is_empty() {
            continue;
        }
        if result.is_empty() {
            result.push_str(text);
        } else {
            let prev = items[i - 1];
            let prev_ends_hyphen = result.ends_with('-');
            let curr_is_hyphen = text == "-";
            let curr_starts_hyphen = text.starts_with('-');
            let font_ratio = item.font_size / prev.font_size;
            let rev_ratio = prev.font_size / item.font_size;
            let y_diff = (item.y - prev.y).abs();
            let is_sub_super = font_ratio < 0.85 && y_diff > 1.0;
            let was_sub_super = rev_ratio < 0.85 && y_diff > 1.0;

            if prev_ends_hyphen || curr_is_hyphen || curr_starts_hyphen || is_sub_super || was_sub_super {
                result.push_str(text);
            } else {
                result.push(' ');
                result.push_str(text);
            }
        }
    }
    result
}

#[allow(clippy::cast_precision_loss)]
fn detect_table_in_region(
    items: &[(usize, &TextItem)],
    mode: Mode,
    page: PageNum,
) -> Option<Table> {
    let columns = find_column_boundaries(items, mode);
    if columns.len() < 2 || columns.len() > 25 {
        return None;
    }

    let rows = find_row_boundaries(items);
    if rows.len() < 2 {
        return None;
    }

    let col_alignment = check_column_alignment(items, &columns, mode);
    let min_alignment = match mode {
        Mode::SmallFont => 0.5,
        Mode::BodyFont => 0.7,
    };
    if col_alignment < min_alignment {
        return None;
    }

    let mut cell_items: Vec<Vec<Vec<&TextItem>>> = vec![vec![Vec::new(); columns.len()]; rows.len()];
    for (_, item) in items {
        if let (Some(col), Some(row)) = (find_column_index(&columns, item.x), find_row_index(&rows, item.y)) {
            cell_items[row][col].push(item);
        }
    }

    // Detect and skip form header rows
    let string_cells: Vec<Vec<String>> = cell_items
        .iter()
        .map(|row| row.iter().map(|col| join_cell_items(col)).collect())
        .collect();
    let first_table_row = find_first_table_row(&string_cells, &rows);

    let (rows, cell_items) = if first_table_row > 0 {
        (rows[first_table_row..].to_vec(), cell_items[first_table_row..].to_vec())
    } else {
        (rows, cell_items)
    };

    let mut cells: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    for row_items in &cell_items {
        let mut row_cells = Vec::with_capacity(columns.len());
        for col_items in row_items {
            let mut sorted: Vec<&TextItem> = col_items.clone();
            let rtl = is_rtl_text(sorted.iter().map(|i| &i.text));
            if rtl {
                sorted.sort_by(|a, b| b.x.total_cmp(&a.x));
            } else {
                sorted.sort_by(|a, b| a.x.total_cmp(&b.x));
            }
            row_cells.push(join_cell_items(&sorted));
        }
        cells.push(row_cells);
    }

    // Validations
    let rows_with_first_col = cells.iter().filter(|row| !row[0].is_empty()).count();
    if rows_with_first_col < rows.len() / 4 {
        return None;
    }

    let rows_with_multi_cols = cells.iter().filter(|row| row.iter().filter(|c| !c.is_empty()).count() >= 2).count();
    let multi_threshold = match mode {
        Mode::SmallFont => (rows.len() / 3).max(1),
        Mode::BodyFont => (rows.len() / 2).max(1),
    };
    if rows_with_multi_cols < multi_threshold {
        return None;
    }

    if rows.len() > 200 {
        return None;
    }

    let total_filled: usize = cells.iter().map(|row| row.iter().filter(|c| !c.is_empty()).count()).sum();
    if (total_filled as f32 / rows.len() as f32) < 1.5 {
        return None;
    }

    if is_key_value_layout(&cells) || !has_consistent_columns(&cells) || !has_table_like_content(&cells, mode) || is_table_of_contents(&cells) || is_paragraph_content(&cells) {
        return None;
    }

    let y_top = rows.first().copied().unwrap_or(0.0);

    debug!(
        "heuristic table detected: {} rows x {} cols",
        rows.len(),
        columns.len()
    );

    Some(Table { cells, y_top, page })
}

#[allow(clippy::cast_precision_loss)]
fn find_first_table_row(cells: &[Vec<String>], _rows: &[f32]) -> usize {
    if cells.is_empty() {
        return 0;
    }
    let total_cols = cells[0].len();

    for (row_idx, row) in cells.iter().enumerate() {
        let filled: Vec<&String> = row.iter().filter(|c| !c.trim().is_empty()).collect();
        let fill_ratio = filled.len() as f32 / total_cols as f32;

        let form_count = filled.iter().filter(|c| {
            let t = c.trim();
            (t.ends_with(':') && t.len() > 1) || (t.contains(": ") && !looks_like_number(t))
        }).count();
        let has_form = form_count > 0 && (form_count * 2 >= filled.len() || fill_ratio < 0.3);

        if has_form { continue; }

        let numeric = filled.iter().filter(|c| looks_like_number(c.trim())).count();
        if numeric >= 2 || fill_ratio >= 0.4 {
            return row_idx;
        }
        if fill_ratio < 0.3 { continue; }

        if row_idx + 1 < cells.len() {
            let next = &cells[row_idx + 1];
            let nf = next.iter().filter(|c| !c.trim().is_empty()).count();
            let nfr = nf as f32 / total_cols as f32;
            let next_form = next.iter().any(|c| {
                let t = c.trim();
                (t.ends_with(':') && t.len() > 1) || (t.contains(": ") && !looks_like_number(t))
            });
            if (nfr >= 0.4 || next.iter().filter(|c| looks_like_number(c.trim())).count() >= 2) && !next_form {
                return row_idx;
            }
        }
    }
    0
}

fn check_column_alignment(items: &[(usize, &TextItem)], columns: &[f32], mode: Mode) -> f32 {
    let tolerance = match mode {
        Mode::SmallFont => 40.0,
        Mode::BodyFont => 30.0,
    };
    let aligned = items.iter().filter(|(_, item)| columns.iter().any(|&col| (item.x - col).abs() < tolerance)).count();
    aligned as f32 / items.len() as f32
}

fn is_key_value_layout(cells: &[Vec<String>]) -> bool {
    if cells.is_empty() { return false; }
    let num_cols = cells[0].len();
    let mut label_like = 0;
    let mut rows_two_or_less = 0;
    for row in cells {
        let filled = row.iter().filter(|c| !c.is_empty()).count();
        if filled <= 2 { rows_two_or_less += 1; }
        let first = row.first().map(|s| s.trim()).unwrap_or("");
        if first.ends_with(':') || (first.len() > 3 && first.chars().all(|c| c.is_uppercase() || c.is_whitespace() || c == '(' || c == ')')) {
            label_like += 1;
        }
    }
    let pct_two = rows_two_or_less as f32 / cells.len() as f32;
    let pct_label = label_like as f32 / cells.len() as f32;
    pct_two > 0.7 && pct_label > 0.5 && num_cols <= 6
}

#[allow(clippy::cast_precision_loss)]
fn has_consistent_columns(cells: &[Vec<String>]) -> bool {
    if cells.len() < 3 { return true; }
    let filled_counts: Vec<usize> = cells.iter().map(|r| r.iter().filter(|c| !c.is_empty()).count()).collect();
    let mut freq: HashMap<usize, usize> = HashMap::new();
    for &c in &filled_counts { *freq.entry(c).or_insert(0) += 1; }
    let most_common = freq.iter().max_by(|(ca, fa), (cb, fb)| fa.cmp(fb).then_with(|| ca.cmp(cb))).map(|(c, _)| *c).unwrap_or(0);

    let num_cols = cells[0].len();
    let tolerance = if num_cols > 15 { num_cols / 4 } else { 2 };
    let consistent = filled_counts.iter().filter(|&&c| c >= most_common.saturating_sub(tolerance) && c <= most_common + tolerance).count();
    let min_ratio = if num_cols > 15 { 0.25 } else { 0.40 };
    consistent as f32 / cells.len() as f32 > min_ratio
}

fn has_table_like_content(cells: &[Vec<String>], mode: Mode) -> bool {
    let mut data_like = 0;
    let mut total = 0;
    for row in cells.iter().skip(1) {
        for cell in row {
            let t = cell.trim();
            if !t.is_empty() {
                total += 1;
                if looks_like_table_data(t) { data_like += 1; }
            }
        }
    }
    if total == 0 { return false; }
    let pct = data_like as f32 / total as f32;
    let num_cols = cells.first().map(|r| r.len()).unwrap_or(0);
    let min_pct = match mode { Mode::SmallFont => 0.2, Mode::BodyFont => 0.3 };
    if pct > min_pct || num_cols >= 3 { return true; }
    if num_cols == 2 && mode == Mode::BodyFont {
        let non_empty: Vec<usize> = cells.iter().skip(1).flat_map(|r| r.iter()).filter(|c| !c.trim().is_empty()).map(|c| c.trim().len()).collect();
        if !non_empty.is_empty() {
            return non_empty.iter().sum::<usize>() / non_empty.len() <= 25;
        }
    }
    false
}

fn looks_like_table_data(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() { return false; }
    if looks_like_number(s) { return true; }
    if s.len() <= 10 && s.chars().filter(|c| c.is_ascii_digit()).count() >= 4 && (s.contains('/') || s.contains('-')) && s.chars().all(|c| c.is_ascii_digit() || c == '/' || c == '-') { return true; }
    if s.len() <= 10 && s.chars().all(|c| c.is_alphanumeric()) && s.chars().any(|c| c.is_ascii_digit()) { return true; }
    let has_number = s.chars().any(|c| c.is_ascii_digit());
    let has_unit = s.contains('°') || s.contains('V') || s.contains('A') || s.contains("Hz") || s.contains("mA") || s.contains("µ") || s.contains("pin") || s.contains("MHz") || s.contains("kHz");
    if has_number && has_unit { return true; }
    if s.contains('(') && s.contains(')') && has_number { return true; }
    if (s.contains("°C") || s.contains("°F")) && s.contains("to") { return true; }
    false
}

fn looks_like_number(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',' || c == '-' || c == '+') && s.chars().any(|c| c.is_ascii_digit())
}

fn is_table_of_contents(cells: &[Vec<String>]) -> bool {
    if cells.is_empty() { return false; }
    let num_cols = cells[0].len();
    let mut dot_cells = 0;
    let mut page_number_cells = 0;
    let mut total_cells = 0;
    let mut dot_cols = vec![0u32; num_cols];
    let mut numeric_cols = vec![0u32; num_cols];

    for row in cells {
        for (ci, cell) in row.iter().enumerate() {
            let t = cell.trim();
            if t.is_empty() { continue; }
            total_cells += 1;
            let dot_count = t.chars().filter(|&c| c == '.').count();
            if dot_count > t.len() / 2 && dot_count >= 3 {
                dot_cells += 1;
                if ci < num_cols { dot_cols[ci] += 1; }
            }
            let digits: String = t.chars().filter(|c| !c.is_whitespace()).collect();
            if digits.len() <= 4 && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                page_number_cells += 1;
                if ci < num_cols { numeric_cols[ci] += 1; }
            }
        }
    }
    if total_cells == 0 { return false; }
    let cols_with_dots = dot_cols.iter().filter(|&&c| c >= 2).count();
    let cols_with_numbers = numeric_cols.iter().filter(|&&c| c >= 2).count();
    if cols_with_dots <= 1 && cols_with_numbers >= 3 { return false; }
    let dot_ratio = dot_cells as f32 / total_cells as f32;
    let page_ratio = page_number_cells as f32 / total_cells as f32;
    dot_ratio > 0.15 || (dot_ratio > 0.05 && page_ratio > 0.15)
}

#[allow(clippy::cast_precision_loss)]
fn is_paragraph_content(cells: &[Vec<String>]) -> bool {
    if cells.is_empty() { return false; }
    let num_cols = cells[0].len();
    let total_cells = cells.len() * num_cols;
    if total_cells == 0 { return false; }

    let filled: Vec<&str> = cells.iter().flat_map(|r| r.iter()).map(|c| c.trim()).filter(|c| !c.is_empty()).collect();
    let total_filled = filled.len();
    if total_filled < 4 { return false; }

    let empty_ratio = 1.0 - (total_filled as f32 / total_cells as f32);

    let hyphen_breaks = filled.iter().filter(|c| {
        c.ends_with('-') && c.len() > 1 && {
            let mut chars = c.chars().rev();
            chars.next();
            chars.next().is_some_and(|ch| ch.is_alphabetic())
        }
    }).count();
    if hyphen_breaks as f32 / total_filled as f32 > 0.03 { return true; }
    if empty_ratio > 0.55 && cells.len() > 10 { return true; }

    let letter_spaced = filled.iter().filter(|c| {
        let chars: Vec<char> = c.chars().collect();
        chars.len() >= 9 && chars.windows(4).all(|w| (w[0].is_alphabetic() && w[1] == ' ' && w[2].is_alphabetic() && w[3] == ' ') || (w[0] == ' ' && w[1].is_alphabetic() && w[2] == ' ' && w[3].is_alphabetic()))
    }).count();
    if letter_spaced > 0 && letter_spaced as f32 / total_filled as f32 > 0.08 { return true; }

    let long_cells = filled.iter().filter(|c| c.len() > 60).count();
    let long_ratio = long_cells as f32 / total_filled as f32;
    let avg_len = filled.iter().map(|c| c.len()).sum::<usize>() as f32 / total_filled as f32;
    if avg_len > 40.0 && long_ratio > 0.2 { return true; }
    if long_ratio > 0.3 { return true; }
    false
}
