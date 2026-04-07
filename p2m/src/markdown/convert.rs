//! Core line-to-Markdown conversion loop with table/image interleaving.

use std::collections::{HashMap, HashSet};

use super::analysis::{
    bold_heading_level, calculate_font_stats, compute_heading_tiers, compute_paragraph_threshold,
    detect_header_level, font_size_rarity, has_dot_leaders,
};
use super::classify::{format_list_item, is_caption_line, is_list_item, is_monospace_font};
use super::postprocess::clean_markdown;
use super::preprocess::{merge_drop_caps, merge_heading_lines};
use crate::options::MarkdownOptions;
use crate::pdf::structure_tree::StructRole;
use crate::types::{PageNum, TextLine};

/// Pre-scan lines to find "isolated" ones: short lines with paragraph breaks
/// both before and after.  These are heading candidates even at body font size.
fn find_isolated_lines(lines: &[TextLine], base_size: f32, para_threshold: f32) -> HashSet<usize> {
    let mut set = HashSet::new();
    for i in 0..lines.len() {
        let line = &lines[i];
        let plain = line.text();
        let trimmed = plain.trim();
        let word_count = trimmed.split_whitespace().count();
        if !(1..=6).contains(&word_count) || trimmed.len() <= 3 {
            continue;
        }
        let font_size = line.items.first().map_or(0.0, |it| it.font_size);
        if font_size < base_size * 0.95 {
            continue;
        }
        if is_list_item(trimmed) || is_caption_line(trimmed) {
            continue;
        }

        let last_char = trimmed.chars().last().unwrap_or(' ');
        if last_char == '-' || last_char == ',' || last_char == ';' {
            continue;
        }
        let last_word = trimmed.split_whitespace().last().unwrap_or("");
        let continuation_words = [
            "the", "a", "an", "and", "or", "of", "in", "to", "for", "with", "by", "on", "at",
            "from", "as", "is", "are", "was", "were", "be", "that", "this", "their", "its", "our",
            "your", "has", "have", "had", "not",
        ];
        if continuation_words.contains(&last_word.to_lowercase().as_str()) {
            continue;
        }

        let break_before = if i == 0 {
            true
        } else {
            let prev = &lines[i - 1];
            prev.page != line.page || (prev.y - line.y).abs() > para_threshold
        };

        let break_after = if i + 1 >= lines.len() {
            true
        } else {
            let next = &lines[i + 1];
            next.page != line.page || (line.y - next.y).abs() > para_threshold
        };

        if !break_before || !break_after {
            continue;
        }

        set.insert(i);
    }

    // Density guard: if too many lines on a page are "isolated", they're
    // paragraph lines in a multi-column layout, not headings.
    let mut page_line_counts: HashMap<PageNum, (usize, usize)> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        let entry = page_line_counts.entry(line.page).or_insert((0, 0));
        entry.0 += 1;
        if set.contains(&i) {
            entry.1 += 1;
        }
    }
    for (&page, &(total, isolated)) in &page_line_counts {
        #[allow(clippy::cast_precision_loss)]
        let ratio = isolated as f32 / total as f32;
        if total > 0 && ratio > 0.25 {
            set.retain(|&i| lines[i].page != page);
        }
    }

    set
}

/// Resolve the dominant structure role for a text line by MCID lookup.
fn resolve_line_struct_role(
    line: &TextLine,
    struct_roles: &HashMap<PageNum, HashMap<i64, StructRole>>,
) -> Option<StructRole> {
    let page_roles = struct_roles.get(&line.page)?;
    for item in &line.items {
        if let Some(mcid) = item.mcid
            && let Some(role) = page_roles.get(&mcid)
        {
            match role {
                StructRole::Document
                | StructRole::Part
                | StructRole::Art
                | StructRole::Sect
                | StructRole::Div
                | StructRole::NonStruct
                | StructRole::Span
                | StructRole::Private => {}
                _ => return Some(role.clone()),
            }
        }
    }
    None
}

/// Map a `StructRole` heading variant to a markdown heading level (1–6).
const fn struct_role_heading_level(role: &StructRole) -> Option<usize> {
    match role {
        StructRole::H | StructRole::H1 => Some(1),
        StructRole::H2 => Some(2),
        StructRole::H3 => Some(3),
        StructRole::H4 => Some(4),
        StructRole::H5 => Some(5),
        StructRole::H6 => Some(6),
        _ => None,
    }
}

/// Merge continuation tables that span across page breaks.
#[allow(dead_code)]
pub(super) fn merge_continuation_tables(
    page_tables: &mut HashMap<PageNum, Vec<(f32, String)>>,
    table_only_pages: &HashSet<PageNum>,
) {
    let mut sorted_pages: Vec<PageNum> = page_tables.keys().copied().collect();
    sorted_pages.sort();

    if sorted_pages.len() < 2 {
        return;
    }

    let mut i = 0;
    while i < sorted_pages.len() {
        let first_page = sorted_pages[i];
        let first_tables = match page_tables.get(&first_page) {
            Some(t) if t.len() == 1 => t,
            _ => {
                i += 1;
                continue;
            }
        };

        if !table_only_pages.contains(&first_page) {
            i += 1;
            continue;
        }

        let first_col_count = count_table_columns(&first_tables[0].1);
        if first_col_count == 0 {
            i += 1;
            continue;
        }

        let mut continuation_pages = Vec::new();
        let mut j = i + 1;
        while j < sorted_pages.len() {
            let next_page = sorted_pages[j];
            let prev_page = continuation_pages.last().copied().unwrap_or(first_page);
            if next_page.get() != prev_page.get() + 1 {
                break;
            }

            if !table_only_pages.contains(&next_page) {
                break;
            }

            let next_tables = match page_tables.get(&next_page) {
                Some(t) if t.len() == 1 => t,
                _ => break,
            };

            let next_col_count = count_table_columns(&next_tables[0].1);
            if next_col_count != first_col_count {
                break;
            }

            continuation_pages.push(next_page);
            j += 1;
        }

        if continuation_pages.is_empty() {
            i += 1;
        } else {
            let mut extra_rows = String::new();
            for &cont_page in &continuation_pages {
                if let Some(tables) = page_tables.get(&cont_page) {
                    for line in tables[0].1.lines().skip(2) {
                        extra_rows.push_str(line);
                        extra_rows.push('\n');
                    }
                }
            }

            if let Some(tables) = page_tables.get_mut(&first_page) {
                tables[0].1.push_str(&extra_rows);
            }

            for &cont_page in &continuation_pages {
                page_tables.remove(&cont_page);
            }

            i = j;
        }
    }
}

/// Count the number of columns in a markdown table by counting `|` in the
/// separator row.
#[allow(dead_code)]
fn count_table_columns(table_md: &str) -> usize {
    if let Some(sep_line) = table_md.lines().nth(1)
        && sep_line.contains("---")
    {
        let pipes = sep_line.chars().filter(|&c| c == '|').count();
        return if pipes >= 2 { pipes - 1 } else { 0 };
    }
    0
}

/// Flush any remaining tables and images for a given page.
fn flush_page_tables_and_images(
    page: PageNum,
    page_tables: &HashMap<PageNum, Vec<(f32, String)>>,
    page_images: &HashMap<PageNum, Vec<(f32, String)>>,
    inserted_tables: &mut HashSet<(PageNum, usize)>,
    inserted_images: &mut HashSet<(PageNum, usize)>,
    output: &mut String,
    in_paragraph: &mut bool,
) {
    if let Some(tables) = page_tables.get(&page) {
        for (idx, (_, table_md)) in tables.iter().enumerate() {
            if inserted_tables.insert((page, idx)) {
                if *in_paragraph {
                    output.push_str("\n\n");
                    *in_paragraph = false;
                }
                output.push('\n');
                output.push_str(table_md);
                output.push('\n');
            }
        }
    }
    if let Some(images) = page_images.get(&page) {
        for (idx, (_, image_md)) in images.iter().enumerate() {
            if inserted_images.insert((page, idx)) {
                if *in_paragraph {
                    output.push_str("\n\n");
                    *in_paragraph = false;
                }
                output.push('\n');
                output.push_str(image_md);
                output.push('\n');
            }
        }
    }
}

/// Convert text lines to markdown, inserting tables and images at appropriate
/// Y positions.
#[allow(
    clippy::cast_precision_loss,
    clippy::needless_pass_by_value,
    clippy::shadow_unrelated,
    clippy::too_many_lines
)]
pub(super) fn to_markdown_with_tables_and_images(
    lines: Vec<TextLine>,
    options: &MarkdownOptions,
    page_tables: HashMap<PageNum, Vec<(f32, String)>>,
    page_images: HashMap<PageNum, Vec<(f32, String)>>,
    band_split_pages: &HashSet<PageNum>,
    struct_roles: Option<&HashMap<PageNum, HashMap<i64, StructRole>>>,
) -> String {
    if lines.is_empty() && page_tables.is_empty() && page_images.is_empty() {
        return String::new();
    }

    let font_stats = calculate_font_stats(&lines);
    let base_size = options
        .base_font_size
        .unwrap_or(font_stats.most_common_size);

    let merged = merge_drop_caps(&lines, base_size);
    let heading_tiers = compute_heading_tiers(&merged, base_size);
    let processed = merge_heading_lines(merged, base_size, &heading_tiers, struct_roles);
    drop(lines);
    let lines = processed;
    let para_threshold = compute_paragraph_threshold(&lines, base_size);
    let isolated_lines = find_isolated_lines(&lines, base_size, para_threshold);

    let mut output = String::new();
    let mut current_page = PageNum::new(1);
    let mut first_page = true;
    let mut prev_y = f32::MAX;
    let mut prev_x = 0.0f32;
    let mut in_list = false;
    let mut in_paragraph = false;
    let mut last_list_x: Option<f32> = None;
    let mut in_code_block = false;
    let mut prev_had_dot_leaders = false;
    let mut inserted_tables: HashSet<(PageNum, usize)> = HashSet::new();
    let mut inserted_images: HashSet<(PageNum, usize)> = HashSet::new();

    let mut all_content_pages: Vec<PageNum> = page_tables
        .keys()
        .chain(page_images.keys())
        .copied()
        .collect();
    all_content_pages.sort();
    all_content_pages.dedup();

    for (line_idx, line) in lines.iter().enumerate() {
        if first_page || line.page != current_page {
            if !first_page {
                if in_code_block {
                    output.push_str("```\n");
                    in_code_block = false;
                }
                flush_page_tables_and_images(
                    current_page,
                    &page_tables,
                    &page_images,
                    &mut inserted_tables,
                    &mut inserted_images,
                    &mut output,
                    &mut in_paragraph,
                );
                if in_paragraph {
                    output.push_str("\n\n");
                    in_paragraph = false;
                }
                output.push_str("\n\n");

                for &p in &all_content_pages {
                    if p <= current_page || p >= line.page {
                        continue;
                    }
                    flush_page_tables_and_images(
                        p,
                        &page_tables,
                        &page_images,
                        &mut inserted_tables,
                        &mut inserted_images,
                        &mut output,
                        &mut in_paragraph,
                    );
                    if in_paragraph {
                        output.push_str("\n\n");
                        in_paragraph = false;
                    }
                    output.push_str("\n\n");
                }
            }

            current_page = line.page;
            first_page = false;
            prev_y = f32::MAX;
            prev_x = 0.0;

            if options.include_page_breaks {
                output.push_str("<!-- Page ");
                output.push_str(&current_page.to_string());
                output.push_str(" -->\n\n");
            }
        }

        // Insert tables before this line based on Y position
        if let Some(tables) = page_tables.get(&current_page) {
            for (idx, (table_y, table_md)) in tables.iter().enumerate() {
                if *table_y > line.y && inserted_tables.insert((current_page, idx)) {
                    if in_paragraph {
                        output.push_str("\n\n");
                        in_paragraph = false;
                    }
                    output.push('\n');
                    output.push_str(table_md);
                    output.push('\n');
                }
            }
        }

        // Insert images before this line based on Y position
        if let Some(images) = page_images.get(&current_page) {
            for (idx, (image_y, image_md)) in images.iter().enumerate() {
                if *image_y > line.y && inserted_images.insert((current_page, idx)) {
                    if in_paragraph {
                        output.push_str("\n\n");
                        in_paragraph = false;
                    }
                    output.push('\n');
                    output.push_str(image_md);
                    output.push('\n');
                }
            }
        }

        let y_gap = prev_y - line.y;
        let line_x = line.items.first().map_or(0.0, |i| i.x);
        let is_para_break = y_gap.abs() > para_threshold;
        let is_band_switch = band_split_pages.contains(&line.page)
            && y_gap.abs() <= para_threshold
            && (prev_x - line_x).abs() > 50.0
            && prev_y < f32::MAX;
        if (is_para_break || is_band_switch) && in_paragraph {
            output.push_str("\n\n");
            in_paragraph = false;
        }
        prev_y = line.y;
        prev_x = line_x;

        let text = line.text_with_formatting(options.detect_bold, options.detect_italic);
        let trimmed = text.trim();

        let plain_text = line.text();
        let plain_trimmed = plain_text.trim();

        if trimmed.is_empty() {
            continue;
        }

        let struct_role = struct_roles.and_then(|roles| resolve_line_struct_role(line, roles));

        let is_code_line = struct_role
            .as_ref()
            .is_some_and(|r| matches!(r, StructRole::Code))
            || (options.detect_code && line.items.iter().any(|i| is_monospace_font(&i.font)));

        if in_code_block && !is_code_line {
            output.push_str("```\n");
            in_code_block = false;
        }

        // Captions
        if struct_role
            .as_ref()
            .is_some_and(|r| matches!(r, StructRole::Caption))
            || is_caption_line(plain_trimmed)
        {
            if in_paragraph {
                output.push_str("\n\n");
                in_paragraph = false;
            }
            output.push_str(trimmed);
            output.push_str("\n\n");
            continue;
        }

        // Headings
        let struct_heading = struct_role.as_ref().and_then(struct_role_heading_level);
        let heuristic_heading = if options.detect_headers
            && plain_trimmed.len() > 3
            && plain_trimmed.split_whitespace().count() <= 15
        {
            let line_font_size = line.items.first().map_or(base_size, |i| i.font_size);
            detect_header_level(line_font_size, base_size, &heading_tiers).or_else(|| {
                if line_font_size < base_size * 0.95 {
                    return None;
                }
                let word_count = plain_trimmed.split_whitespace().count();
                if !(1..=15).contains(&word_count) {
                    return None;
                }
                let rarity = font_size_rarity(line_font_size, &font_stats);
                let all_bold = !line.items.is_empty() && line.items.iter().all(|i| i.bold);
                let standalone = !in_paragraph;
                let isolated = isolated_lines.contains(&line_idx);

                let score = rarity.mul_add(
                    0.5,
                    if all_bold { 0.3 } else { 0.0 }
                        + if standalone { 0.2 } else { 0.0 }
                        + if isolated { 0.3 } else { 0.0 },
                );

                if score >= 0.5 && standalone && word_count >= 2 {
                    Some(bold_heading_level(&heading_tiers))
                } else {
                    None
                }
            })
        } else {
            None
        };

        if let Some(level) = struct_heading.or(heuristic_heading) {
            if in_paragraph {
                output.push_str("\n\n");
                in_paragraph = false;
            }
            for _ in 0..level {
                output.push('#');
            }
            output.push(' ');
            output.push_str(plain_trimmed);
            output.push_str("\n\n");
            in_list = false;
            continue;
        }

        // Structure-tree list item
        if struct_role
            .as_ref()
            .is_some_and(|r| matches!(r, StructRole::LI))
            && !is_list_item(plain_trimmed)
        {
            if in_paragraph {
                output.push_str("\n\n");
                in_paragraph = false;
            }
            output.push_str("- ");
            output.push_str(trimmed);
            output.push('\n');
            in_list = true;
            last_list_x = line.items.first().map(|i| i.x);
            continue;
        }

        // List items
        if options.detect_lists && is_list_item(plain_trimmed) {
            if in_paragraph {
                output.push_str("\n\n");
                in_paragraph = false;
            }
            let formatted = format_list_item(trimmed);
            output.push_str(&formatted);
            output.push('\n');
            in_list = true;
            last_list_x = line.items.first().map(|i| i.x);
            continue;
        }
        if in_list {
            let curr_x = line.items.first().map(|i| i.x);
            let is_continuation = if let (Some(list_x), Some(cx)) = (last_list_x, curr_x) {
                let x_ok = cx >= list_x - 5.0 && cx <= list_x + 50.0;
                let y_ok = y_gap < base_size * 7.0;
                x_ok && y_ok && !is_list_item(plain_trimmed) && !has_dot_leaders(plain_trimmed)
            } else {
                false
            };

            if is_continuation {
                if output.ends_with('\n') {
                    output.pop();
                    output.push(' ');
                }
                output.push_str(trimmed);
                output.push('\n');
                continue;
            }
            in_list = false;
            last_list_x = None;
        }

        // Block quote
        if struct_role
            .as_ref()
            .is_some_and(|r| matches!(r, StructRole::BlockQuote))
        {
            if in_paragraph {
                output.push_str("\n\n");
                in_paragraph = false;
            }
            output.push_str("> ");
            output.push_str(trimmed);
            output.push('\n');
            continue;
        }

        // Code blocks
        if is_code_line {
            if in_paragraph {
                output.push_str("\n\n");
                in_paragraph = false;
            }
            if !in_code_block {
                output.push_str("```\n");
                in_code_block = true;
            }
            output.push_str(plain_trimmed);
            output.push('\n');
            continue;
        }

        // Regular text
        let cur_dot_leaders = has_dot_leaders(plain_trimmed);
        if in_paragraph {
            if cur_dot_leaders || prev_had_dot_leaders {
                output.push('\n');
            } else {
                output.push(' ');
            }
        }
        output.push_str(trimmed);
        in_paragraph = true;
        prev_had_dot_leaders = cur_dot_leaders;
    }

    if in_code_block {
        output.push_str("```\n");
    }

    flush_page_tables_and_images(
        current_page,
        &page_tables,
        &page_images,
        &mut inserted_tables,
        &mut inserted_images,
        &mut output,
        &mut in_paragraph,
    );
    for &p in &all_content_pages {
        if p <= current_page {
            continue;
        }
        flush_page_tables_and_images(
            p,
            &page_tables,
            &page_images,
            &mut inserted_tables,
            &mut inserted_images,
            &mut output,
            &mut in_paragraph,
        );
    }

    if in_paragraph {
        output.push('\n');
    }

    clean_markdown(output, options)
}

/// Convert text lines to markdown, interleaving detected tables at their
/// original Y positions.
pub fn to_markdown_from_lines(
    lines: Vec<TextLine>,
    options: &MarkdownOptions,
    page_tables: HashMap<PageNum, Vec<(f32, String)>>,
    struct_roles: Option<&HashMap<PageNum, HashMap<i64, StructRole>>>,
) -> String {
    to_markdown_with_tables_and_images(
        lines,
        options,
        page_tables,
        HashMap::new(),
        &HashSet::new(),
        struct_roles,
    )
}
