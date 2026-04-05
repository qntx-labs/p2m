//! Text item merging and subscript absorption.
//!
//! Post-processes extracted [`TextItem`]s by merging adjacent items on the
//! same line and absorbing numeric subscripts into their parent items.

use crate::text::bidi::is_rtl_text;
use crate::text::cjk::is_cjk_char;
use crate::types::{PageNum, TextItem};

/// Cap item width for merge-gap computation to guard against Tw inflation.
///
/// When PDF word-spacing (Tw) is large, the advance width of strings
/// containing spaces extends far past the visible glyph extent. This
/// capping prevents incorrectly merging items from different columns.
fn effective_merge_width(item: &TextItem) -> f32 {
    if item.width <= 0.0 || item.font_size <= 0.0 {
        return item.width;
    }
    if !item.text.contains(' ') {
        return item.width;
    }
    if item.text.chars().any(is_cjk_char) {
        return item.width;
    }
    let char_count = item.text.chars().count();
    if char_count == 0 {
        return item.width;
    }
    let avg = item.width / char_count as f32;
    if avg > item.font_size * 0.85 {
        let capped = char_count as f32 * item.font_size * 0.6;
        capped.min(item.width)
    } else {
        item.width
    }
}

/// Merge adjacent text items on the same line into single items.
///
/// Groups items by (page, Y-position) with a 5pt tolerance, sorts within
/// each group by X, then merges consecutive items that share a similar
/// font size and are close horizontally.
pub(crate) fn merge_text_items(items: Vec<TextItem>) -> Vec<TextItem> {
    if items.is_empty() {
        return items;
    }

    let y_tolerance = 5.0;
    let mut line_groups: Vec<(PageNum, f32, Vec<&TextItem>)> = Vec::new();

    for item in &items {
        let found = line_groups
            .iter_mut()
            .find(|(pg, y, _)| *pg == item.page && (item.y - *y).abs() < y_tolerance);
        if let Some((_, _, group)) = found {
            group.push(item);
        } else {
            line_groups.push((item.page, item.y, vec![item]));
        }
    }

    for (_, _, group) in &mut line_groups {
        let rtl = is_rtl_text(group.iter().map(|i| &i.text));
        if rtl {
            group.sort_by(|a, b| b.x.total_cmp(&a.x));
        } else {
            group.sort_by(|a, b| a.x.total_cmp(&b.x));
        }
    }

    line_groups.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.total_cmp(&a.1)));

    let mut merged = Vec::new();

    for (_, _, group) in &line_groups {
        let mut i = 0;
        while i < group.len() {
            let first = group[i];
            let mut text = first.text.clone();
            let mut end_x = first.x + effective_merge_width(first);
            let x_gap_max = first.font_size * 0.5;

            let mut j = i + 1;
            while j < group.len() {
                let next = group[j];
                if (next.font_size - first.font_size).abs() > first.font_size * 0.20 {
                    break;
                }
                let gap = next.x - end_x;
                if gap > x_gap_max {
                    break;
                }
                if gap < -first.font_size * 0.5 {
                    break;
                }
                let threshold = {
                    let prev_last = text.trim_end().chars().last();
                    let next_first = next.text.trim_start().chars().next();
                    if next_first.is_some_and(|c| matches!(c, '.' | ',' | ';' | ')' | ']' | '}')) {
                        first.font_size * 0.25
                    } else if prev_last.is_some_and(|c| c.is_lowercase())
                        && next_first.is_some_and(|c| c.is_lowercase())
                    {
                        first.font_size * 0.13
                    } else {
                        first.font_size * 0.08
                    }
                };
                if gap > threshold {
                    text.push(' ');
                }
                text.push_str(&next.text);
                end_x = next.x + effective_merge_width(next);
                j += 1;
            }

            merged.push(TextItem {
                text,
                x: first.x,
                y: first.y,
                width: end_x - first.x,
                height: first.height,
                font: first.font.clone(),
                font_size: first.font_size,
                page: first.page,
                bold: first.bold,
                italic: first.italic,
                kind: first.kind.clone(),
                mcid: first.mcid,
            });

            i = j;
        }
    }

    merged
}

/// Merge subscript/superscript items into their adjacent parent items.
///
/// Subscripts (e.g. "2" in H₂O) are rendered as separate text items with a
/// much smaller font size. This pass absorbs them into the preceding
/// normal-sized item.
pub(crate) fn merge_subscript_items(items: Vec<TextItem>) -> Vec<TextItem> {
    if items.len() < 2 {
        return items;
    }

    let y_tolerance = 5.0;
    let mut line_groups: Vec<(PageNum, f32, Vec<TextItem>)> = Vec::new();

    for item in items {
        let found = line_groups
            .iter_mut()
            .find(|(pg, y, _)| *pg == item.page && (item.y - *y).abs() < y_tolerance);
        if let Some((_, _, group)) = found {
            group.push(item);
        } else {
            let page = item.page;
            let y = item.y;
            line_groups.push((page, y, vec![item]));
        }
    }

    let mut result = Vec::new();

    for (_, _, mut group) in line_groups {
        group.sort_by(|a, b| a.x.total_cmp(&b.x));

        let max_fs = group.iter().map(|i| i.font_size).fold(0.0_f32, f32::max);
        if max_fs < 1.0 {
            result.extend(group);
            continue;
        }

        let sub_threshold = max_fs * 0.75;
        let mut merged: Vec<TextItem> = Vec::new();

        for item in group {
            if item.font_size < sub_threshold
                && item.font_size > 0.0
                && item.text.len() <= 4
                && item.text.chars().all(|c| c.is_ascii_digit())
            {
                if let Some(parent) = merged.last_mut() {
                    let ends_with_letter = parent
                        .text
                        .chars()
                        .last()
                        .is_some_and(|c| c.is_alphabetic());
                    if parent.font_size >= sub_threshold && ends_with_letter {
                        let parent_right = parent.x + parent.width;
                        let gap = item.x - parent_right;
                        if gap < parent.font_size * 0.2 && gap > -parent.font_size * 0.3 {
                            parent.text.push_str(&item.text);
                            parent.width = (item.x + item.width) - parent.x;
                            continue;
                        }
                    }
                }
            }
            merged.push(item);
        }
        result.extend(merged);
    }

    result
}
