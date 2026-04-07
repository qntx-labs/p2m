//! Position-based word boundary detection and Otsu threshold for
//! letter-spacing anomaly correction (Canva-style PDFs).

use crate::text::cjk::is_cjk_char;
use crate::text::unicode::is_cid_font;
use crate::types::TextItem;

/// Determine if two adjacent text items should be joined without a space
/// based on their physical positions on the page and character case.
///
/// Uses a hybrid approach: position-based with case-aware thresholds.
/// The `threshold` parameter is the adaptive letter-spacing threshold
/// (default 0.10, higher for Canva-style PDFs).
#[allow(clippy::too_many_lines)]
pub fn should_join_items(
    prev_item: &TextItem,
    curr_item: &TextItem,
    single_char_threshold: f32,
) -> bool {
    // If either text explicitly has leading/trailing spaces, respect them.
    if prev_item.text.ends_with(' ') || curr_item.text.starts_with(' ') {
        return false;
    }

    let prev_last = prev_item.text.trim_end().chars().last();
    let curr_first = curr_item.text.trim_start().chars().next();

    // Always join if current starts with punctuation that follows without space.
    if let Some(c) = curr_first
        && matches!(c, '.' | ',' | ';' | '!' | '?' | ')' | ']' | '}' | '\'')
    {
        return true;
    }

    // After colons, add space if followed by alphanumeric.
    if let (Some(p), Some(c)) = (prev_last, curr_first)
        && p == ':'
        && c.is_alphanumeric()
    {
        return false;
    }

    // When we have accurate width from font metrics, use position-based logic.
    if prev_item.width > 0.0 {
        let gap = if prev_item.x <= curr_item.x {
            curr_item.x - (prev_item.x + prev_item.width)
        } else {
            prev_item.x - (curr_item.x + curr_item.width)
        };
        let font_size = prev_item.font_size;

        // Never join across column-scale gaps or large overlaps.
        if gap > font_size * 3.0 || gap < -font_size {
            return false;
        }

        // CID fonts emit one word per text operator with gaps ≈ 0.
        let prev_chars = prev_item.text.trim().chars().count();
        let curr_chars = curr_item.text.trim().chars().count();
        let prev_last_char = prev_item.text.trim().chars().last();
        let curr_first_char = curr_item.text.trim().chars().next();
        let is_cjk =
            prev_last_char.is_some_and(is_cjk_char) || curr_first_char.is_some_and(is_cjk_char);

        if !is_cjk && gap >= 0.0 && gap < font_size * 0.01 && is_cid_font(&prev_item.font) {
            let prev_word_count = prev_item.text.split_whitespace().count();
            if prev_word_count >= 3 {
                return gap < font_size * 0.15;
            }
            return false;
        }

        // Numeric continuity.
        if let (Some(p), Some(c)) = (prev_last, curr_first) {
            let prev_is_numeric = p.is_ascii_digit() || p == ',' || p == '.';
            let curr_is_numeric = c.is_ascii_digit() || c == '%' || c == '.';
            if prev_is_numeric && curr_is_numeric {
                return gap > -font_size && gap < font_size * 0.3;
            }
            if (p == '+' || p == '-') && c.is_ascii_digit() {
                return gap > -font_size && gap < font_size * 0.3;
            }
        }

        // Canva-style letter-spacing: use character-width–based joining.
        if single_char_threshold > 0.20 {
            if prev_chars == 1 {
                return gap < prev_item.width * 1.25;
            }
            if curr_chars == 1 {
                #[allow(clippy::cast_precision_loss)]
                let avg_char_width = prev_item.width / prev_chars as f32;
                return gap < avg_char_width * 1.25;
            }
            return gap < font_size * single_char_threshold;
        }

        // Single-char fragment joined to multi-char item.
        if (prev_chars == 1) != (curr_chars == 1) {
            return gap < font_size * 0.20;
        }

        // Both single-char: per-glyph positioning.
        if prev_chars == 1 && curr_chars == 1 {
            if let (Some(p), Some(c)) = (prev_last, curr_first) {
                let p_numeric = p.is_ascii_digit() || matches!(p, ',' | '.' | '%' | '+' | '-');
                let c_numeric = c.is_ascii_digit() || matches!(c, ',' | '.' | '%');
                if p_numeric && c_numeric {
                    return gap < font_size * 0.25;
                }
            }
            return gap < font_size * single_char_threshold;
        }

        // Multi-char items: case-aware thresholds.
        if prev_item.text.trim().chars().count() >= 2 && curr_item.text.trim().chars().count() >= 2
        {
            let prev_ends_lower = prev_item
                .text
                .trim()
                .chars()
                .last()
                .is_some_and(char::is_lowercase);
            let curr_starts_lower = curr_item
                .text
                .trim()
                .chars()
                .next()
                .is_some_and(char::is_lowercase);
            if prev_ends_lower && curr_starts_lower {
                return gap < font_size * 0.18;
            }
        }
        return gap < font_size * 0.15;
    }

    // Fallback: estimate width from font size heuristics.
    let char_width = prev_item.font_size * 0.45;
    #[allow(clippy::cast_precision_loss)]
    let prev_text_len = prev_item.text.chars().count() as f32;
    let estimated_prev_width = prev_text_len * char_width;
    let prev_end_x = prev_item.x + estimated_prev_width;
    let gap = curr_item.x - prev_end_x;

    if gap > char_width * 6.0 {
        return false;
    }

    let is_cjk = prev_last.is_some_and(is_cjk_char) || curr_first.is_some_and(is_cjk_char);
    if is_cjk {
        return gap < char_width * 0.8;
    }

    match (prev_last, curr_first) {
        (Some(p), Some(c)) if p.is_alphabetic() && c.is_alphabetic() => {
            let same_case =
                (p.is_uppercase() && c.is_uppercase()) || (p.is_lowercase() && c.is_lowercase());
            if same_case {
                gap < char_width * 0.8
            } else if p.is_lowercase() && c.is_uppercase() {
                false
            } else {
                gap < char_width * 0.3
            }
        }
        _ => gap < char_width * 0.5,
    }
}

/// Detect and fix Canva-style letter-spacing within text items.
///
/// Returns the adaptive join threshold for this page: `0.10` for normal
/// pages, or a higher Otsu-derived threshold for Canva-style pages.
pub fn fix_letterspaced_items(items: &mut [TextItem]) -> f32 {
    const DEFAULT: f32 = 0.10;

    fn is_letterspaced(text: &str) -> bool {
        let trimmed = text.trim();
        let chars: Vec<char> = trimmed.chars().collect();
        if chars.len() < 3 {
            return false;
        }
        chars
            .iter()
            .enumerate()
            .all(|(i, &c)| if i % 2 == 0 { c != ' ' } else { c == ' ' })
    }

    if items.is_empty() {
        return DEFAULT;
    }

    let mut letterspaced_count = 0u32;
    let mut total_text_items = 0u32;
    for item in items.iter() {
        let trimmed = item.text.trim();
        if trimmed.is_empty() || trimmed.len() < 3 {
            continue;
        }
        total_text_items += 1;
        if is_letterspaced(&item.text) {
            letterspaced_count += 1;
        }
    }

    if total_text_items < 4 || letterspaced_count * 2 < total_text_items {
        // Second path: per-character rendering without embedded spaces.
        let single_char_count = items
            .iter()
            .filter(|i| i.text.trim().chars().count() == 1)
            .count();
        if items.len() >= 10 && single_char_count * 2 >= items.len() {
            let threshold = compute_canva_join_threshold(items);
            if threshold > 0.40 {
                return threshold;
            }
        }
        return DEFAULT;
    }

    let threshold = compute_canva_join_threshold(items);

    for item in items.iter_mut() {
        if is_letterspaced(&item.text) {
            item.text = item.text.chars().filter(|&c| c != ' ').collect();
        }
    }

    threshold
}

/// Compute join threshold for a confirmed Canva-style page.
///
/// Uses `median × 1.55` on the `gap/font_size` ratio distribution.
fn compute_canva_join_threshold(items: &[TextItem]) -> f32 {
    const DEFAULT: f32 = 0.10;
    const MIN_SAMPLES: usize = 8;

    let ratios = collect_gap_ratios(items);
    if ratios.len() < MIN_SAMPLES {
        return DEFAULT;
    }

    let mut sorted = ratios;
    sorted.sort_by(f32::total_cmp);

    if sorted[sorted.len() - 1] < 0.40 || sorted[0] < 0.40 {
        return DEFAULT;
    }

    let median = sorted[sorted.len() / 2];
    (median * 1.55).clamp(0.50, 2.0)
}

/// Collect positive `gap/font_size` ratios from adjacent item pairs.
fn collect_gap_ratios(items: &[TextItem]) -> Vec<f32> {
    let mut ratios = Vec::new();
    for pair in items.windows(2) {
        let prev = &pair[0];
        let curr = &pair[1];

        let prev_c = prev.text.trim().chars().last();
        let curr_c = curr.text.trim().chars().next();
        if prev_c.is_some_and(is_cjk_char) || curr_c.is_some_and(is_cjk_char) {
            continue;
        }

        if prev.width <= 0.0 || prev.font_size <= 0.0 {
            continue;
        }

        let gap = if prev.x <= curr.x {
            curr.x - (prev.x + prev.width)
        } else {
            prev.x - (curr.x + curr.width)
        };

        let ratio = gap / prev.font_size;
        if (0.0..=3.0).contains(&ratio) {
            ratios.push(ratio);
        }
    }
    ratios
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ItemKind, PageNum};

    fn make_char_item(ch: char, x: f32, width: f32, font_size: f32) -> TextItem {
        TextItem {
            text: ch.to_string(),
            x,
            y: 100.0,
            width,
            height: font_size,
            font: "TestFont".into(),
            font_size,
            page: PageNum::new(1),
            bold: false,
            italic: false,
            kind: ItemKind::Text,
            mcid: None,
        }
    }

    fn make_text_item(text: &str, x: f32, width: f32, font_size: f32) -> TextItem {
        TextItem {
            text: text.into(),
            x,
            y: 100.0,
            width,
            height: font_size,
            font: "TestFont".into(),
            font_size,
            page: PageNum::new(1),
            bold: false,
            italic: false,
            kind: ItemKind::Text,
            mcid: None,
        }
    }

    #[test]
    fn fix_letterspaced_items_returns_adaptive_threshold() {
        let fs = 12.0;
        let char_w = fs * 0.5;
        let letter_gap = fs * 0.6;
        let word_gap = fs * 1.2;

        let words = [
            "H e l l o",
            "W o r l d",
            "F o o",
            "B a r",
            "B a z",
            "Q u x",
            "T e s t",
            "D a t a",
            "M o r e",
            "T e x t",
        ];

        let mut items = Vec::new();
        let mut x = 100.0;
        #[allow(clippy::cast_precision_loss)]
        for (wi, word) in words.iter().enumerate() {
            let char_count = word.chars().filter(|c| !c.is_whitespace()).count();
            let w = ((char_count - 1) as f32).mul_add(letter_gap, char_count as f32 * char_w);
            items.push(TextItem {
                text: (*word).into(),
                x,
                y: 100.0,
                width: w,
                height: fs,
                font: "TestFont".into(),
                font_size: fs,
                page: PageNum::new(1),
                bold: false,
                italic: false,
                kind: ItemKind::Text,
                mcid: None,
            });
            x += w + if wi % 3 == 2 { word_gap } else { letter_gap };
        }

        let threshold = fix_letterspaced_items(&mut items);
        assert!(
            threshold > 0.50,
            "Canva page should get threshold > 0.50, got {threshold}"
        );
        assert_eq!(items[0].text, "Hello");
        assert_eq!(items[1].text, "World");
        assert_eq!(items[9].text, "Text");
    }

    #[test]
    fn canva_style_items_join_correctly() {
        let fs = 12.0;
        let char_w = fs * 0.5;
        let intra_gap = fs * 0.6;
        let word_gap = fs * 1.2;

        let mut items = Vec::new();
        #[allow(clippy::cast_precision_loss)]
        for (i, &ch) in ['H', 'e', 'l', 'l', 'o'].iter().enumerate() {
            let x = (i as f32).mul_add(char_w + intra_gap, 100.0);
            items.push(make_char_item(ch, x, char_w, fs));
        }
        #[allow(clippy::expect_used)] // items is non-empty after the loop above
        let w_x = items.last().expect("not empty").x + char_w + word_gap;
        items.push(make_char_item('W', w_x, char_w, fs));
        #[allow(clippy::cast_precision_loss)]
        for (i, &ch) in ['o', 'r', 'l', 'd'].iter().enumerate() {
            let x = ((i + 1) as f32).mul_add(char_w + intra_gap, w_x);
            items.push(make_char_item(ch, x, char_w, fs));
        }

        // Compute threshold from gap ratios.
        let threshold = compute_canva_join_threshold(&items);

        assert!(
            should_join_items(&items[0], &items[1], threshold),
            "H+e should join with threshold {threshold}"
        );
        assert!(
            !should_join_items(&items[4], &items[5], threshold),
            "o+W (word boundary) should NOT join with threshold {threshold}"
        );
    }

    #[test]
    fn canva_width_based_single_char_prev_join() {
        let fs = 12.0;
        let threshold = 0.90;

        let k = make_text_item("K", 100.0, 7.9, fs);
        let a = make_text_item("a", 115.9, 6.0, fs);
        assert!(should_join_items(&k, &a, threshold), "K→a should join");

        let f = make_text_item("f", 193.0, 4.0, fs);
        let k2 = make_text_item("K", 207.47, 7.9, fs);
        assert!(!should_join_items(&f, &k2, threshold), "f→K should split");
    }

    #[test]
    fn canva_width_based_multi_to_single_join() {
        let fs = 12.0;
        let threshold = 0.90;

        let ilw = make_text_item("ilw", 320.0, 23.6, fs);
        let a = make_text_item("a", 353.0, 6.0, fs);
        assert!(should_join_items(&ilw, &a, threshold), "ilw→a should join");

        let rich = make_text_item("rich", 229.0, 34.8, fs);
        let m = make_text_item("m", 277.8, 10.7, fs);
        assert!(
            !should_join_items(&rich, &m, threshold),
            "rich→m should split"
        );
    }
}
