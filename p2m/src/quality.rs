//! Quality checks for extracted text.
//!
//! Detects garbage text from broken font encodings, CID-to-Unicode mapping
//! failures, and dollar-as-space encoding issues. These signals indicate that
//! the extracted text is unreliable and OCR fallback may be needed.

/// Check if extracted text is predominantly garbage (non-alphanumeric).
///
/// Broken font encodings produce text like `"----1-.-.-.___  --.-. .._ I_---."`.
/// Real text in any language has >50% alphanumeric characters.
pub fn is_garbage_text(text: &str) -> bool {
    let mut alphanum = 0usize;
    let mut non_alphanum = 0usize;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        if matches!(ch, '#' | '*' | '|' | '-' | '\n') {
            continue;
        }
        if ch.is_alphanumeric() {
            alphanum += 1;
        } else {
            non_alphanum += 1;
        }
    }
    let total = alphanum + non_alphanum;
    total >= 50 && alphanum * 2 < total
}

/// Detect garbage from failed CID-to-Unicode mapping (Identity-H fonts).
///
/// Raw CID bytes often produce C1 control characters (U+0080–U+009F) or
/// high-Latin-1 mojibake when CID values don't map to Unicode codepoints.
pub fn is_cid_garbage(text: &str) -> bool {
    if is_garbage_text(text) {
        return true;
    }
    let mut total = 0usize;
    let mut c1_control = 0usize;
    let mut high_latin = 0usize;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        total += 1;
        if ('\u{0080}'..='\u{009F}').contains(&ch) {
            c1_control += 1;
        }
        if ('\u{00A0}'..='\u{00FF}').contains(&ch) {
            high_latin += 1;
        }
    }
    if total < 5 {
        return false;
    }
    if c1_control * 20 >= total {
        return true;
    }
    let ascii_letters = text.chars().filter(|c| c.is_ascii_alphabetic()).count();
    high_latin * 5 >= total * 2 && ascii_letters * 3 < total
}

/// Detect encoding issues like U+FFFD replacements or dollar-as-space patterns.
///
/// Returns `true` when:
/// - Text contains U+FFFD (replacement character)
/// - `$` appears frequently between letters (broken CMap substitution)
pub fn detect_encoding_issues(text: &str) -> bool {
    if text.contains('\u{FFFD}') {
        return true;
    }

    let total_dollars = text.matches('$').count();
    if total_dollars > 10 {
        let bytes = text.as_bytes();
        let mut letter_dollar_letter = 0usize;
        for i in 1..bytes.len().saturating_sub(1) {
            if bytes[i] == b'$'
                && bytes[i - 1].is_ascii_alphabetic()
                && bytes[i + 1].is_ascii_alphabetic()
            {
                letter_dollar_letter += 1;
            }
        }
        if letter_dollar_letter > 20 || letter_dollar_letter * 2 > total_dollars {
            return true;
        }
    }

    false
}

/// Combined quality assessment result.
#[derive(Debug, Clone, Copy)]
pub struct QualityReport {
    /// Text is predominantly non-alphanumeric (broken font encoding).
    pub is_garbage: bool,
    /// CID-to-Unicode mapping failure detected (C1 controls, mojibake).
    pub is_cid_garbage: bool,
    /// Encoding issues like U+FFFD or dollar-as-space patterns.
    pub has_encoding_issues: bool,
}

impl QualityReport {
    /// Returns `true` if any quality issue was detected.
    pub fn needs_ocr_fallback(&self) -> bool {
        self.is_garbage || self.is_cid_garbage || self.has_encoding_issues
    }
}

/// Run all quality checks on the extracted markdown text.
pub fn check_quality(markdown: &str) -> QualityReport {
    QualityReport {
        is_garbage: is_garbage_text(markdown),
        is_cid_garbage: is_cid_garbage(markdown),
        has_encoding_issues: detect_encoding_issues(markdown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_text_detected() {
        let garbage = ",&<X ~%5&8-!A ~*(!,-!U (/#!U X ~#/=U 9/%*(!U !(  X \
                       (%U-(-/ V %&((8-#&&< *,(6--< %5&8-!( (,(/! #/<5U X \
                       º&( >/5 /5&(#(8-!5 *,(6--( *,%@/-A W";
        assert!(is_garbage_text(garbage));
    }

    #[test]
    fn normal_text_not_garbage() {
        let normal = "This is a normal paragraph with words and sentences that \
                      contains enough characters to pass the threshold.";
        assert!(!is_garbage_text(normal));
    }

    #[test]
    fn cyrillic_not_garbage() {
        let cyrillic = "Роботизированные технологии комплексы для производства \
                        металлургических предприятий";
        assert!(!is_garbage_text(cyrillic));
    }

    #[test]
    fn cid_garbage_detected() {
        let cid = "Ë>íÓ\tý\r\u{0088}æ&Ït\u{0094}äí;\ný;wAL¢©èåD\rü£\
                   qq\u{0096}¶Í Æ\réá; Ô 7G\u{008B}ý;èÕç¢ £ ý;C";
        assert!(is_cid_garbage(cid));
    }

    #[test]
    fn korean_text_not_cid_garbage() {
        let korean = "본 가격표는 국내 거주 중인 외국인을 위한 한국어 가격표의 비공식 번역본입니다";
        assert!(!is_cid_garbage(korean));
    }

    #[test]
    fn fffd_encoding_issue() {
        assert!(detect_encoding_issues("Some text with \u{FFFD} replacement"));
    }

    #[test]
    fn dollar_as_space_encoding_issue() {
        let garbled = "Last$advanced$Book$Programm$3th$Workshop$on$Chest$Wall$Deformities$and$More";
        assert!(detect_encoding_issues(garbled));
    }

    #[test]
    fn financial_text_no_encoding_issue() {
        let financial = "Revenue was $100M in Q1, up from $90M. Costs: $50M, $30M, $20M, $15M, $12M, $8M, $5M, $3M, $2M, $1M, $500K.";
        assert!(!detect_encoding_issues(financial));
    }

    #[test]
    fn clean_text_no_encoding_issue() {
        assert!(!detect_encoding_issues("Normal markdown text with no issues."));
    }

    #[test]
    fn quality_report_combined() {
        let report = check_quality("Hello, this is perfectly fine text with enough words for the check.");
        assert!(!report.needs_ocr_fallback());
    }
}
