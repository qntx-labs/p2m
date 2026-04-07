//! NFKC normalization, ligature expansion, and invisible character stripping.

use unicode_normalization::UnicodeNormalization;

/// Expand Unicode ligature characters to their component characters,
/// strip invisible characters, and normalize Arabic presentation forms.
///
/// This makes extracted text more searchable and semantically correct.
#[allow(clippy::shadow_reuse)]
pub fn expand_ligatures(text: &str) -> String {
    // Strip null bytes and other control characters (except newline/tab).
    let text = if text
        .bytes()
        .any(|b| b < 0x20 && b != b'\n' && b != b'\r' && b != b'\t')
    {
        text.chars()
            .filter(|&c| c >= ' ' || c == '\n' || c == '\r' || c == '\t')
            .collect::<String>()
    } else {
        text.to_string()
    };

    // Detect Arabic presentation forms before normalization — their presence
    // signals visual-order storage that needs reversal after NFKC.
    let had_presentation_forms = text.chars().any(is_arabic_presentation_form);

    // Apply NFKC only when Arabic presentation forms are present.
    // Avoids converting NBSP (U+00A0) to regular space in normal text.
    let text = if had_presentation_forms {
        text.nfkc().collect::<String>()
    } else {
        text
    };

    let mut result = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            // Latin ligatures (fallback for fonts bypassing NFKC).
            '\u{FB00}' => result.push_str("ff"),
            '\u{FB01}' => result.push_str("fi"),
            '\u{FB02}' => result.push_str("fl"),
            '\u{FB03}' => result.push_str("ffi"),
            '\u{FB04}' => result.push_str("ffl"),
            '\u{FB05}' | '\u{FB06}' => result.push_str("st"),
            // Strip invisible Unicode characters.
            '\u{00AD}' | '\u{200B}' | '\u{FEFF}' | '\u{200C}' | '\u{200D}' | '\u{2060}' => {}
            // Normalize typographic spaces to ASCII space.
            // Excludes NBSP (U+00A0) which is handled by coordinate-based spacing.
            '\u{2000}'..='\u{200A}' => result.push(' '),
            _ => result.push(ch),
        }
    }

    // If the original text had Arabic presentation forms, the characters are
    // in visual (LTR screen) order. Reverse to restore logical reading order.
    if had_presentation_forms {
        result = reverse_visual_arabic(&result);
    }

    result
}

/// Decode a PDF text string that may be UTF-16BE (BOM `\xFE\xFF`) or
/// `PDFDocEncoding` (Latin-1 superset).
#[allow(clippy::cast_lossless)]
pub fn decode_text_string(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        // UTF-16BE with BOM.
        let utf16: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16_lossy(&utf16)
    } else {
        // PDFDocEncoding — identical to Latin-1 for the byte range we care about.
        bytes.iter().map(|&b| b as char).collect()
    }
}

/// Compute effective font size from base size and text matrix.
///
/// Text matrix is `[a, b, c, d, tx, ty]` where `a`,`d` are scale factors.
pub fn effective_font_size(base_size: f32, text_matrix: &[f32; 6]) -> f32 {
    let scale_x = text_matrix[0].hypot(text_matrix[1]);
    let scale_y = text_matrix[2].hypot(text_matrix[3]);
    let scale = scale_x.max(scale_y);
    base_size * scale
}

/// Estimate the width of a text item, falling back to a character-count
/// heuristic when width is zero.
#[allow(clippy::cast_precision_loss)]
pub fn effective_width(item: &crate::types::TextItem) -> f32 {
    if item.width > 0.0 {
        item.width
    } else {
        item.text.chars().count() as f32 * item.font_size * 0.5
    }
}

/// Check if a font name indicates bold style.
pub fn is_bold_font(font_name: &str) -> bool {
    let lower = font_name.to_lowercase();
    lower.contains("bold")
        || lower.contains("-bd")
        || lower.contains("_bd")
        || lower.contains("black")
        || lower.contains("heavy")
        || lower.contains("demibold")
        || lower.contains("semibold")
        || lower.contains("demi-bold")
        || lower.contains("semi-bold")
        || lower.contains("extrabold")
        || lower.contains("ultrabold")
        || (lower.contains("medium") && !lower.contains("mediumitalic"))
}

/// Check if a font name indicates italic/oblique style.
pub fn is_italic_font(font_name: &str) -> bool {
    let lower = font_name.to_lowercase();
    lower.contains("italic")
        || lower.contains("oblique")
        || lower.contains("-it")
        || lower.contains("_it")
        || lower.contains("slant")
        || lower.contains("inclined")
        || lower.contains("kursiv")
}

/// Check if a font name indicates a CID font.
pub fn is_cid_font(font: &str) -> bool {
    font.starts_with("C2_") || font.starts_with("C0_")
}

/// Check if a character is an Arabic presentation form (not BOM).
const fn is_arabic_presentation_form(c: char) -> bool {
    matches!(c, '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFE}')
}

/// Reverse visual-order Arabic text to logical order.
///
/// Pure RTL text gets a simple character reversal. Mixed content
/// (embedded numbers or Latin words) splits into LTR and non-LTR runs:
/// run order is reversed, and only non-LTR runs are reversed internally.
fn reverse_visual_arabic(text: &str) -> String {
    let has_ltr = text.chars().any(|c| c.is_ascii_alphanumeric());

    if !has_ltr {
        return text.chars().rev().collect();
    }

    let chars: Vec<char> = text.chars().collect();
    let mut runs: Vec<(bool, String)> = Vec::new();

    let mut i = 0;
    while i < chars.len() {
        let is_ltr = chars[i].is_ascii_alphanumeric()
            || (chars[i].is_ascii_punctuation() && is_adjacent_to_ascii_alnum(&chars, i));

        let mut run = String::new();
        while i < chars.len() {
            let c = chars[i];
            let c_is_ltr = c.is_ascii_alphanumeric()
                || (c.is_ascii_punctuation() && is_adjacent_to_ascii_alnum(&chars, i));
            if c_is_ltr != is_ltr {
                break;
            }
            run.push(c);
            i += 1;
        }
        runs.push((is_ltr, run));
    }

    runs.reverse();
    let mut result = String::with_capacity(text.len());
    for (is_ltr, content) in &runs {
        if *is_ltr {
            result.push_str(content);
        } else {
            result.extend(content.chars().rev());
        }
    }
    result
}

/// Check if the character at `idx` is adjacent to an ASCII alphanumeric.
fn is_adjacent_to_ascii_alnum(chars: &[char], idx: usize) -> bool {
    (idx > 0 && chars[idx - 1].is_ascii_alphanumeric())
        || (idx + 1 < chars.len() && chars[idx + 1].is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_soft_hyphen() {
        assert_eq!(expand_ligatures("con\u{00AD}tent"), "content");
    }

    #[test]
    fn strip_zero_width_space() {
        assert_eq!(expand_ligatures("hello\u{200B}world"), "helloworld");
    }

    #[test]
    fn strip_bom() {
        assert_eq!(expand_ligatures("\u{FEFF}text"), "text");
    }

    #[test]
    fn strip_zwnj_zwj_word_joiner() {
        assert_eq!(expand_ligatures("a\u{200C}b\u{200D}c\u{2060}d"), "abcd");
    }

    #[test]
    fn ligature_plus_invisible_chars() {
        assert_eq!(expand_ligatures("\u{FB01}rst\u{00AD}ly"), "firstly");
    }

    #[test]
    fn ligatures_still_expand() {
        assert_eq!(expand_ligatures("\u{FB00}\u{FB01}\u{FB02}"), "fffifl");
    }

    #[test]
    fn normalize_typographic_spaces() {
        assert_eq!(expand_ligatures("•\u{2003}text"), "• text");
        assert_eq!(expand_ligatures("a\u{2002}b"), "a b");
        assert_eq!(expand_ligatures("x\u{2009}y"), "x y");
    }

    #[test]
    fn nbsp_preserved() {
        assert_eq!(expand_ligatures("a\u{00A0}b"), "a\u{00A0}b");
    }

    #[test]
    fn nfkc_arabic_presentation_forms() {
        let input = "\u{FEE1}\u{FEF3}";
        let result = expand_ligatures(input);
        assert!(
            !result.chars().any(is_arabic_presentation_form),
            "presentation forms should be normalized: {result:?}"
        );
        assert!(
            result.chars().any(|c| matches!(c, '\u{0600}'..='\u{06FF}')),
            "should contain base Arabic characters: {result:?}"
        );
    }

    #[test]
    fn no_reversal_for_base_arabic() {
        let input = "\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}"; // مرحبا
        let result = expand_ligatures(input);
        assert_eq!(result, input, "base Arabic should pass through unchanged");
    }

    #[test]
    fn latin_text_unaffected() {
        assert_eq!(expand_ligatures("Hello World"), "Hello World");
    }

    #[test]
    fn reverse_visual_arabic_pure_rtl() {
        let input = "\u{0628}\u{0627}";
        let result = reverse_visual_arabic(input);
        assert_eq!(result, "\u{0627}\u{0628}");
    }

    #[test]
    fn reverse_visual_arabic_with_ltr_run() {
        let input = "\u{0623}123\u{0628}";
        let result = reverse_visual_arabic(input);
        assert_eq!(result, "\u{0628}123\u{0623}");
    }

    #[test]
    fn arabic_presentation_form_detection() {
        assert!(is_arabic_presentation_form('\u{FB50}'));
        assert!(is_arabic_presentation_form('\u{FDFF}'));
        assert!(is_arabic_presentation_form('\u{FE70}'));
        assert!(is_arabic_presentation_form('\u{FEFE}'));
        assert!(!is_arabic_presentation_form('\u{FEFF}'));
        assert!(!is_arabic_presentation_form('\u{0645}'));
        assert!(!is_arabic_presentation_form('A'));
    }
}
