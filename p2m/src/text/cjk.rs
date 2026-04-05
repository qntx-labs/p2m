//! CJK character classification and spacing rules.

/// Check if a character is CJK (Chinese, Japanese, Korean).
///
/// CJK languages don't use spaces between words, so word-boundary
/// heuristics should not apply when CJK characters are involved.
pub const fn is_cjk_char(c: char) -> bool {
    matches!(
        c,
        '\u{1100}'..='\u{11FF}'   // Hangul Jamo
        | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{3130}'..='\u{318F}' // Hangul Compatibility Jamo
        | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '\u{FF00}'..='\u{FFEF}' // Halfwidth and Fullwidth Forms
    )
}
