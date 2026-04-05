//! Post-processing: dot leaders, hyphenation, page numbers, URL formatting.

use std::fmt::Write;
use std::sync::LazyLock;

use regex::Regex;

use crate::options::MarkdownOptions;

/// Clean up markdown output with post-processing.
pub(super) fn clean_markdown(mut text: String, options: &MarkdownOptions) -> String {
    text = collapse_dot_leaders(&text);

    if options.fix_hyphenation {
        text = fix_hyphenation(&text);
    }

    if options.remove_page_numbers {
        text = remove_page_numbers(&text);
    }

    if options.format_urls {
        text = format_urls(&text);
    }

    collapse_consecutive_spaces(&mut text);

    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }

    text = text.trim().to_string();
    text.push('\n');

    text
}

/// Collapse runs of 2+ spaces to a single space within each line.
fn collapse_consecutive_spaces(text: &mut String) {
    let mut result = String::with_capacity(text.len());
    for line in text.split('\n') {
        if !result.is_empty() {
            result.push('\n');
        }
        let trimmed = line.trim_start();
        let leading = &line[..line.len() - trimmed.len()];
        result.push_str(leading);
        let mut prev_space = false;
        for ch in trimmed.chars() {
            if ch == ' ' {
                if !prev_space {
                    result.push(' ');
                }
                prev_space = true;
            } else {
                prev_space = false;
                result.push(ch);
            }
        }
    }
    *text = result;
}

/// Collapse dot leaders (runs of 4+ dots) into " ... ".
#[allow(clippy::expect_used)]
fn collapse_dot_leaders(text: &str) -> String {
    static DOT_LEADER_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\.{4,}").expect("valid regex"));
    DOT_LEADER_RE.replace_all(text, " ... ").to_string()
}

/// Fix words broken across lines with spaces before the continuation.
#[allow(clippy::expect_used)]
fn fix_hyphenation(text: &str) -> String {
    static SPACED_HYPHEN_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"([a-zA-ZáàâãéèêíïóôõöúçñÁÀÂÃÉÈÊÍÏÓÔÕÖÚÇÑ]) - ([a-zA-ZáàâãéèêíïóôõöúçñÁÀÂÃÉÈÊÍÏÓÔÕÖÚÇÑ])").expect("valid regex")
    });

    SPACED_HYPHEN_RE
        .replace_all(text, |caps: &regex::Captures<'_>| {
            format!("{}-{}", &caps[1], &caps[2])
        })
        .to_string()
}

/// Remove standalone page numbers.
fn remove_page_numbers(text: &str) -> String {
    let mut result = Vec::new();
    let lines: Vec<&str> = text.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if is_page_number_line(trimmed) {
            let prev_is_break = i > 0 && lines[i - 1].trim() == "---";
            let next_is_break = i + 1 < lines.len() && lines[i + 1].trim() == "---";
            let prev_is_empty = i > 0 && lines[i - 1].trim().is_empty();
            let next_is_empty = i + 1 < lines.len() && lines[i + 1].trim().is_empty();

            let is_isolated = (prev_is_break || prev_is_empty || i == 0)
                && (next_is_break || next_is_empty || i + 1 == lines.len());

            let before_break = i + 1 < lines.len()
                && (lines[i + 1].trim() == "---"
                    || (i + 2 < lines.len()
                        && lines[i + 1].trim().is_empty()
                        && lines[i + 2].trim() == "---"));

            if is_isolated || before_break {
                continue;
            }
        }

        result.push(*line);
    }

    result.join("\n")
}

/// Check if a line looks like a page number.
fn is_page_number_line(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }

    // Pattern 1: Just a number (1-4 digits)
    if trimmed.len() <= 4 && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }

    // Pattern 2: "Page X of Y" or "Page X"
    let lower = trimmed.to_lowercase();
    if let Some(after_page) = lower.strip_prefix("page") {
        let rest = after_page.trim();
        if rest == "of" || rest.starts_with("of ") {
            return true;
        }
        if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
        if rest.is_empty()
            || rest
                .split_whitespace()
                .all(|w| w == "of" || w.chars().all(|c| c.is_ascii_digit()))
        {
            return true;
        }
    }

    // Pattern 3: "X of Y" where X and Y are numbers
    if let Some(of_idx) = trimmed.find(" of ") {
        let before = trimmed[..of_idx].trim();
        let after = trimmed[of_idx + 4..].trim();
        if before.chars().all(|c| c.is_ascii_digit())
            && after.chars().all(|c| c.is_ascii_digit())
            && !before.is_empty()
            && !after.is_empty()
        {
            return true;
        }
    }

    // Pattern 4: "- X -" centered page number
    if trimmed.len() >= 3 && trimmed.starts_with('-') && trimmed.ends_with('-') {
        let inner = trimmed[1..trimmed.len() - 1].trim();
        if inner.chars().all(|c| c.is_ascii_digit()) && !inner.is_empty() {
            return true;
        }
    }

    false
}

/// Convert URLs to markdown links.
#[allow(clippy::expect_used)]
fn format_urls(text: &str) -> String {
    static URL_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"https?://[^\s<>\)\]]+[^\s<>\)\]\.\,;]").expect("valid regex"));

    let mut result = String::with_capacity(text.len());
    let mut last_end = 0;

    for mat in URL_RE.find_iter(text) {
        let start = mat.start();
        let url = mat.as_str();

        let before = {
            let mut check_start = start.saturating_sub(2);
            while check_start > 0 && !text.is_char_boundary(check_start) {
                check_start -= 1;
            }
            if check_start < start && text.is_char_boundary(start) {
                &text[check_start..start]
            } else {
                ""
            }
        };
        let already_linked = before.ends_with("](");

        let prefix = if text.is_char_boundary(start) {
            &text[..start]
        } else {
            let mut safe_start = start;
            while safe_start > 0 && !text.is_char_boundary(safe_start) {
                safe_start -= 1;
            }
            &text[..safe_start]
        };
        let open_brackets = prefix.matches('[').count();
        let close_brackets = prefix.matches(']').count();
        let inside_link_text = open_brackets > close_brackets;

        let safe_last_end = safe_char_boundary(text, last_end, true);
        let safe_start = safe_char_boundary(text, start, true);
        let safe_end = safe_char_boundary(text, mat.end(), true);

        if already_linked || inside_link_text {
            if safe_last_end <= safe_end {
                result.push_str(&text[safe_last_end..safe_end]);
            }
        } else {
            if safe_last_end <= safe_start {
                result.push_str(&text[safe_last_end..safe_start]);
            }
            let _ = write!(result, "[{url}]({url})");
        }
        last_end = safe_end;
    }

    let safe_last_end = safe_char_boundary(text, last_end, true);
    if safe_last_end < text.len() {
        result.push_str(&text[safe_last_end..]);
    }
    result
}

/// Find the nearest valid character boundary at or after `pos`.
const fn safe_char_boundary(text: &str, pos: usize, forward: bool) -> usize {
    if text.is_char_boundary(pos) {
        return pos;
    }
    let mut p = pos;
    if forward {
        while p < text.len() && !text.is_char_boundary(p) {
            p += 1;
        }
    } else {
        while p > 0 && !text.is_char_boundary(p) {
            p -= 1;
        }
    }
    p
}
