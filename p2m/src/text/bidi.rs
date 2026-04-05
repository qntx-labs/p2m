//! RTL detection and bidirectional item sorting.

use crate::text::cjk::is_cjk_char;
use crate::types::TextItem;

/// Check if a character belongs to an RTL script.
pub const fn is_rtl_char(c: char) -> bool {
    matches!(
        c,
        '\u{0590}'..='\u{05FF}'   // Hebrew
        | '\u{0600}'..='\u{06FF}' // Arabic
        | '\u{0700}'..='\u{074F}' // Syriac
        | '\u{0750}'..='\u{077F}' // Arabic Supplement
        | '\u{0780}'..='\u{07BF}' // Thaana
        | '\u{07C0}'..='\u{07FF}' // NKo
        | '\u{0800}'..='\u{083F}' // Samaritan
        | '\u{0840}'..='\u{085F}' // Mandaic
        | '\u{08A0}'..='\u{08FF}' // Arabic Extended-A
        | '\u{FB1D}'..='\u{FB4F}' // Hebrew Presentation Forms
        | '\u{FB50}'..='\u{FDFF}' // Arabic Presentation Forms-A
        | '\u{FE70}'..='\u{FEFF}' // Arabic Presentation Forms-B
    )
}

/// Determine whether a sequence of text fragments is predominantly RTL.
pub fn is_rtl_text<I, S>(texts: I) -> bool
where
    I: Iterator<Item = S>,
    S: AsRef<str>,
{
    let (mut rtl, mut ltr) = (0u32, 0u32);
    for t in texts {
        for c in t.as_ref().chars() {
            if is_rtl_char(c) {
                rtl += 1;
            } else if c.is_alphabetic() && !is_cjk_char(c) {
                ltr += 1;
            }
        }
    }
    rtl > 0 && rtl > ltr
}

/// Sort items within a line by X position, respecting text direction.
pub fn sort_line_items(items: &mut [TextItem]) {
    let rtl = is_rtl_text(items.iter().map(|i| &i.text));
    if rtl {
        items.sort_by(|a, b| b.x.total_cmp(&a.x));
    } else {
        items.sort_by(|a, b| a.x.total_cmp(&b.x));
    }
}
