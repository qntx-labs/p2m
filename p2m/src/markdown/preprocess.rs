//! Drop cap merging, heading line consolidation, and repeated line removal.

use std::collections::{HashMap, HashSet};

use super::analysis::detect_header_level;
use crate::pdf::structure_tree::StructRole;
use crate::types::{PageNum, TextLine};

/// Resolve a heading level for a line, considering both struct-tree roles and
/// font heuristics.  Struct-tree headings take priority.
fn effective_heading_level(
    line: &TextLine,
    base_size: f32,
    heading_tiers: &[f32],
    struct_roles: Option<&HashMap<PageNum, HashMap<i64, StructRole>>>,
) -> Option<usize> {
    if let Some(roles) = struct_roles
        && let Some(page_roles) = roles.get(&line.page)
    {
        for item in &line.items {
            if let Some(mcid) = item.mcid
                && let Some(role) = page_roles.get(&mcid)
            {
                let level = match role {
                    StructRole::H | StructRole::H1 => Some(1),
                    StructRole::H2 => Some(2),
                    StructRole::H3 => Some(3),
                    StructRole::H4 => Some(4),
                    StructRole::H5 => Some(5),
                    StructRole::H6 => Some(6),
                    _ => None,
                };
                if level.is_some() {
                    return level;
                }
            }
        }
    }

    let font = line.items.first().map_or(base_size, |i| i.font_size);
    detect_header_level(font, base_size, heading_tiers)
}

/// Merge consecutive heading lines at the same level into a single line.
///
/// When a heading wraps across multiple text lines, each fragment becomes a
/// separate `# Header` in the output.  This detects consecutive lines at the
/// same heading tier on the same page with a small Y gap and merges them.
pub(super) fn merge_heading_lines(
    lines: Vec<TextLine>,
    base_size: f32,
    heading_tiers: &[f32],
    struct_roles: Option<&HashMap<PageNum, HashMap<i64, StructRole>>>,
) -> Vec<TextLine> {
    if lines.is_empty() {
        return lines;
    }

    let mut result: Vec<TextLine> = Vec::with_capacity(lines.len());

    for line in lines {
        let line_level = effective_heading_level(&line, base_size, heading_tiers, struct_roles);
        let line_font = line.items.first().map_or(base_size, |i| i.font_size);

        let should_merge = if let (Some(prev), Some(curr_level)) = (result.last(), line_level) {
            let prev_level = effective_heading_level(prev, base_size, heading_tiers, struct_roles);
            let same_page = prev.page == line.page;
            let same_level = prev_level == Some(curr_level);
            let y_gap = prev.y - line.y;
            let close_enough = y_gap > 0.0 && y_gap < line_font * 2.0;
            let prev_words = prev.text().split_whitespace().count();
            let curr_words = line.text().split_whitespace().count();
            let not_too_long = prev_words + curr_words <= 20;
            same_page && same_level && close_enough && not_too_long
        } else {
            false
        };

        if should_merge {
            #[allow(clippy::unwrap_used)] // should_merge is true only when result.last() is Some
            let prev = result.last_mut().unwrap();
            if let Some(first_item) = line.items.first() {
                let mut space_item = first_item.clone();
                space_item.text = format!(" {}", space_item.text.trim_start());
                prev.items.push(space_item);
            }
            for item in line.items.into_iter().skip(1) {
                prev.items.push(item);
            }
        } else {
            result.push(line);
        }
    }

    result
}

/// Merge drop caps with the appropriate line.
///
/// A drop cap is a single large letter at the start of a paragraph.
/// Due to PDF coordinate sorting, the drop cap may appear AFTER the line it
/// belongs to.
pub(super) fn merge_drop_caps(lines: &[TextLine], base_size: f32) -> Vec<TextLine> {
    let mut result: Vec<TextLine> = Vec::with_capacity(lines.len());

    for line in lines {
        let text = line.text();
        let trimmed = text.trim();

        let is_drop_cap = trimmed.len() <= 2
            && line.items.first().map_or(0.0, |i| i.font_size) >= base_size * 2.5
            && trimmed.chars().next().is_some_and(char::is_uppercase);

        if is_drop_cap {
            #[allow(clippy::unwrap_used)] // trimmed.len() <= 2 check above ensures non-empty
            let drop_char = trimmed.chars().next().unwrap();

            let mut target_idx: Option<usize> = None;

            for (idx, prev_line) in result.iter().enumerate() {
                if prev_line.page != line.page {
                    continue;
                }

                let prev_text = prev_line.text();
                let prev_trimmed = prev_text.trim();

                if prev_trimmed.chars().next().is_some_and(char::is_lowercase) {
                    let is_para_start = if idx == 0 {
                        true
                    } else {
                        let before = result[idx - 1].text();
                        let before_trimmed = before.trim();
                        !before_trimmed
                            .chars()
                            .next()
                            .is_some_and(char::is_lowercase)
                    };

                    if is_para_start {
                        target_idx = Some(idx);
                        break;
                    }
                }
            }

            if let Some(idx) = target_idx
                && let Some(first_item) = result[idx].items.first_mut()
            {
                let prev_text = first_item.text.trim().to_string();
                first_item.text = format!("{drop_char}{prev_text}");
            }
            continue;
        }

        result.push(line.clone());
    }

    result
}

/// Normalize whitespace in a string for comparison.
#[allow(dead_code)]
fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalize text for frequency comparison: collapse whitespace and strip
/// leading/trailing digit sequences (page numbers).
#[allow(dead_code)]
fn normalize_for_comparison(s: &str) -> String {
    let ws = normalize_whitespace(s);
    let result = ws
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start()
        .trim_end_matches(|c: char| c.is_ascii_digit())
        .trim_end();
    result.to_string()
}

/// Returns true if the line looks like a list item or heading (should not be stripped).
#[allow(dead_code)]
fn is_structural_line(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('#')
        || t.starts_with("- ")
        || t.starts_with("* ")
        || t.starts_with("• ")
        || t.chars().next().is_some_and(|c| c.is_ascii_digit())
            && (t.contains(". ") || t.contains(") "))
}

/// Returns true if a line consists entirely of a single repeated character.
#[allow(dead_code)]
fn is_decorative_separator(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    chars.all(|c| c == first)
}

/// Returns true if the given Y position is among the first or last N distinct
/// Y positions on the specified page.
#[allow(dead_code)]
fn is_y_at_edge(
    y: f32,
    page: PageNum,
    page_sorted_ys: &HashMap<PageNum, Vec<f32>>,
    n: usize,
) -> bool {
    let Some(ys) = page_sorted_ys.get(&page) else {
        return false;
    };
    if ys.len() <= n * 2 {
        return true;
    }
    let Some(pos) = ys.iter().position(|&py| (py - y).abs() < 0.1) else {
        return false;
    };
    pos < n || pos >= ys.len() - n
}

/// Strip lines that repeat on many distinct pages (running headers/footers).
///
/// A line is considered a repeated header/footer if:
/// 1. Its normalized text appears on `>= max(3, page_count * 30%)` distinct pages
/// 2. It is at least 10 characters long
/// 3. It doesn't look like a structural element
/// 4. It consistently appears in the top or bottom N distinct Y positions
/// 5. Its Y positions across pages have low variance
/// 6. It is not a decorative separator
#[allow(
    dead_code,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::excessive_nesting,
    clippy::items_after_statements,
    clippy::too_many_lines
)]
pub(super) fn strip_repeated_lines(lines: Vec<TextLine>, page_count: u32) -> Vec<TextLine> {
    if lines.is_empty() || page_count < 3 {
        return lines;
    }

    const EDGE_LINE_COUNT: usize = 5;

    let mut page_y_range: HashMap<PageNum, (f32, f32)> = HashMap::new();
    for line in &lines {
        let entry = page_y_range.entry(line.page).or_insert((line.y, line.y));
        if line.y < entry.0 {
            entry.0 = line.y;
        }
        if line.y > entry.1 {
            entry.1 = line.y;
        }
    }

    let mut page_sorted_ys: HashMap<PageNum, Vec<f32>> = HashMap::new();
    for line in &lines {
        page_sorted_ys.entry(line.page).or_default().push(line.y);
    }
    for ys in page_sorted_ys.values_mut() {
        ys.sort_by(f32::total_cmp);
        ys.dedup();
    }

    #[allow(clippy::cast_precision_loss)]
    let avg_span = {
        let total: f32 = page_y_range.values().map(|(lo, hi)| hi - lo).sum();
        if page_y_range.is_empty() {
            1.0
        } else {
            (total / page_y_range.len() as f32).max(1.0)
        }
    };

    #[allow(clippy::cast_possible_truncation)]
    let mut y_bands: HashMap<(PageNum, i32), Vec<usize>> = HashMap::new();
    for (idx, line) in lines.iter().enumerate() {
        let y_bucket = (line.y * 10.0).round() as i32;
        y_bands.entry((line.page, y_bucket)).or_default().push(idx);
    }

    let mut freq: HashMap<String, HashSet<PageNum>> = HashMap::new();
    let mut y_positions: HashMap<String, Vec<f32>> = HashMap::new();
    for line in &lines {
        if !is_y_at_edge(line.y, line.page, &page_sorted_ys, EDGE_LINE_COUNT) {
            continue;
        }
        let text = line.text();
        let normalized = normalize_for_comparison(&text);
        if normalized.len() < 10 || is_decorative_separator(&normalized) {
            continue;
        }
        freq.entry(normalized.clone())
            .or_default()
            .insert(line.page);
        y_positions.entry(normalized).or_default().push(line.y);
    }

    let mut band_freq: HashMap<String, HashSet<PageNum>> = HashMap::new();
    let mut band_y_positions: HashMap<String, Vec<f32>> = HashMap::new();
    for (&(page, _), indices) in &y_bands {
        if indices.len() < 2 {
            continue;
        }
        let band_y = lines[indices[0]].y;
        if !is_y_at_edge(band_y, page, &page_sorted_ys, EDGE_LINE_COUNT) {
            continue;
        }
        let mut sorted_indices = indices.clone();
        sorted_indices.sort_unstable();
        let coalesced: String = sorted_indices
            .iter()
            .map(|&i| lines[i].text())
            .collect::<Vec<_>>()
            .join(" ");
        let normalized = normalize_for_comparison(&coalesced);
        if normalized.len() < 10 || is_decorative_separator(&normalized) {
            continue;
        }
        band_freq
            .entry(normalized.clone())
            .or_default()
            .insert(page);
        band_y_positions.entry(normalized).or_default().push(band_y);
    }

    let threshold = 3u32.max(page_count * 30 / 100);

    let has_consistent_y = |text: &str, positions: &HashMap<String, Vec<f32>>| -> bool {
        let pos = match positions.get(text) {
            Some(p) if p.len() >= 2 => p,
            _ => return true,
        };
        let n = pos.len() as f32;
        let mean = pos.iter().sum::<f32>() / n;
        let variance = pos.iter().map(|y| (y - mean).powi(2)).sum::<f32>() / n;
        let stddev = variance.sqrt();
        stddev / avg_span < 0.05
    };

    let candidates: HashSet<String> = freq
        .into_iter()
        .filter(|(text, pages)| {
            pages.len() as u32 >= threshold
                && !is_structural_line(text)
                && has_consistent_y(text, &y_positions)
        })
        .map(|(text, _)| text)
        .collect();

    let band_candidates: HashSet<String> = band_freq
        .into_iter()
        .filter(|(text, pages)| {
            pages.len() as u32 >= threshold
                && !is_structural_line(text)
                && has_consistent_y(text, &band_y_positions)
        })
        .map(|(text, _)| text)
        .collect();

    if candidates.is_empty() && band_candidates.is_empty() {
        return lines;
    }

    let mut removal_set: HashSet<usize> = HashSet::new();

    let mut first_page_individual: HashMap<String, PageNum> = HashMap::new();
    for (idx, line) in lines.iter().enumerate() {
        if !is_y_at_edge(line.y, line.page, &page_sorted_ys, EDGE_LINE_COUNT) {
            continue;
        }
        let text = line.text();
        let normalized = normalize_for_comparison(&text);
        if candidates.contains(&normalized) {
            let first = first_page_individual.entry(normalized).or_insert(line.page);
            if line.page > *first {
                removal_set.insert(idx);
            }
        }
    }

    let mut first_page_band: HashMap<String, PageNum> = HashMap::new();
    for (&(page, _), indices) in &y_bands {
        if indices.len() < 2 {
            continue;
        }
        let band_y = lines[indices[0]].y;
        if !is_y_at_edge(band_y, page, &page_sorted_ys, EDGE_LINE_COUNT) {
            continue;
        }
        let mut sorted_indices = indices.clone();
        sorted_indices.sort_unstable();
        let coalesced: String = sorted_indices
            .iter()
            .map(|&i| lines[i].text())
            .collect::<Vec<_>>()
            .join(" ");
        let normalized = normalize_for_comparison(&coalesced);
        if band_candidates.contains(&normalized) {
            let first = first_page_band.entry(normalized).or_insert(page);
            if page < *first {
                *first = page;
            }
        }
    }
    for (&(page, _), indices) in &y_bands {
        if indices.len() < 2 {
            continue;
        }
        let band_y = lines[indices[0]].y;
        if !is_y_at_edge(band_y, page, &page_sorted_ys, EDGE_LINE_COUNT) {
            continue;
        }
        let mut sorted_indices = indices.clone();
        sorted_indices.sort_unstable();
        let coalesced: String = sorted_indices
            .iter()
            .map(|&i| lines[i].text())
            .collect::<Vec<_>>()
            .join(" ");
        let normalized = normalize_for_comparison(&coalesced);
        if band_candidates.contains(&normalized) {
            let first = first_page_band
                .get(&normalized)
                .copied()
                .unwrap_or_else(|| PageNum::new(0));
            if page > first {
                for &idx in &sorted_indices {
                    removal_set.insert(idx);
                }
            }
        }
    }

    // Y-band sibling propagation
    for (&(page, _), indices) in &y_bands {
        let band_y = lines[indices[0]].y;
        if !is_y_at_edge(band_y, page, &page_sorted_ys, EDGE_LINE_COUNT) {
            continue;
        }
        if indices.iter().any(|idx| removal_set.contains(idx)) {
            for &idx in indices {
                removal_set.insert(idx);
            }
        }
    }

    if removal_set.is_empty() {
        return lines;
    }

    lines
        .into_iter()
        .enumerate()
        .filter(|(idx, _)| !removal_set.contains(idx))
        .map(|(_, line)| line)
        .collect()
}
