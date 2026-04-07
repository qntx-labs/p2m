//! Core data types used across the extraction and markdown pipelines.

use std::collections::HashMap;
use std::fmt;

/// A 1-indexed page number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageNum(u32);

impl PageNum {
    /// Create a new page number.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if `n` is zero.
    #[must_use]
    pub fn new(n: u32) -> Self {
        debug_assert!(n > 0, "PageNum is 1-indexed; got 0");
        Self(n)
    }

    /// Return the raw 1-indexed value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for PageNum {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The kind of a content item extracted from a PDF.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum ItemKind {
    /// Regular text content.
    #[default]
    Text,
    /// An image placeholder.
    Image,
    /// A hyperlink with its target URL.
    Link(String),
    /// A form field.
    FormField,
}

/// A text fragment with position and font metadata.
///
/// This is the fundamental unit produced by the extraction pipeline.
/// Coordinates use the PDF coordinate system (origin at bottom-left).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TextItem {
    /// The decoded text content.
    pub text: String,
    /// X position on page (points, from left).
    pub x: f32,
    /// Y position on page (points, from bottom).
    pub y: f32,
    /// Approximate width of the text run (points).
    pub width: f32,
    /// Approximate height, derived from font size (points).
    pub height: f32,
    /// Font name as recorded in the PDF.
    pub font: String,
    /// Font size in points.
    pub font_size: f32,
    /// Page this item belongs to.
    pub page: PageNum,
    /// Whether the font is bold.
    pub bold: bool,
    /// Whether the font is italic.
    pub italic: bool,
    /// What kind of content this item represents.
    pub kind: ItemKind,
    /// Marked Content ID from the content stream's `BDC`/`BMC` operator.
    ///
    /// Used to link this item to the PDF structure tree for tagged PDFs.
    pub mcid: Option<i64>,
}

/// A line of text: grouped [`TextItem`]s that share approximately the same
/// baseline.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TextLine {
    /// The items that make up this line, in reading order.
    pub items: Vec<TextItem>,
    /// The dominant Y coordinate (baseline) of the line.
    pub y: f32,
    /// Page this line belongs to.
    pub page: PageNum,
    /// Adaptive join threshold from page-level letter-spacing detection.
    ///
    /// Default `0.10` for normal PDFs; higher for Canva-style PDFs with
    /// excessive letter-spacing.
    pub(crate) adaptive_threshold: f32,
}

impl TextLine {
    /// Get the plain text of this line (items joined with appropriate spacing).
    #[must_use]
    pub fn text(&self) -> String {
        self.text_with_formatting(false, false)
    }

    /// Get text with optional bold/italic markdown formatting.
    #[must_use]
    pub fn text_with_formatting(&self, format_bold: bool, format_italic: bool) -> String {
        if !format_bold && !format_italic {
            return self.text_plain();
        }

        let threshold = self.adaptive_threshold;
        let mut result = String::new();
        let mut current_bold = false;
        let mut current_italic = false;

        for (i, item) in self.items.iter().enumerate() {
            let text = item.text.as_str();
            let text_trimmed = text.trim();

            if text_trimmed.is_empty() {
                continue;
            }

            let needs_space = if i == 0 || result.is_empty() {
                false
            } else {
                Self::needs_space_between(&self.items[i - 1], item, &result, threshold)
            };

            // Preserve leading whitespace that indicates a word boundary.
            let has_leading_space = text.starts_with(' ');

            let item_bold = format_bold && item.bold;
            let item_italic = format_italic && item.italic;

            // Close previous styles if they change.
            if current_italic && !item_italic {
                result.push('*');
                current_italic = false;
            }
            if current_bold && !item_bold {
                result.push_str("**");
                current_bold = false;
            }

            if needs_space || (has_leading_space && !result.is_empty() && !result.ends_with(' ')) {
                result.push(' ');
            }

            // Open new styles.
            if item_bold && !current_bold {
                result.push_str("**");
                current_bold = true;
            }
            if item_italic && !current_italic {
                result.push('*');
                current_italic = true;
            }

            result.push_str(text_trimmed);
        }

        if current_italic {
            result.push('*');
        }
        if current_bold {
            result.push_str("**");
        }

        result
    }

    /// Get plain text without formatting.
    fn text_plain(&self) -> String {
        let threshold = self.adaptive_threshold;
        let mut result = String::new();
        for (i, item) in self.items.iter().enumerate() {
            if i > 0 && Self::needs_space_between(&self.items[i - 1], item, &result, threshold) {
                result.push(' ');
            }
            result.push_str(&item.text);
        }
        result
    }

    /// Determine if a space is needed between two adjacent items.
    fn needs_space_between(prev: &TextItem, curr: &TextItem, result: &str, threshold: f32) -> bool {
        let text = curr.text.as_str();

        let prev_ends_with_hyphen = result.ends_with('-');
        let curr_is_hyphen = text.trim() == "-";
        let curr_starts_with_hyphen = text.starts_with('-');

        // Detect subscript/superscript by font-size ratio and Y offset.
        let font_ratio = curr.font_size / prev.font_size;
        let reverse_ratio = prev.font_size / curr.font_size;
        let y_diff = (curr.y - prev.y).abs();
        let is_sub_super = font_ratio < 0.85 && y_diff > 1.0;
        let was_sub_super = reverse_ratio < 0.85 && y_diff > 1.0;

        let should_join = crate::text::should_join_items(prev, curr, threshold);

        let prev_ends_with_space = result.ends_with(' ');
        let curr_starts_with_space = text.starts_with(' ');
        let space_exists = prev_ends_with_space || curr_starts_with_space;

        !(prev_ends_with_hyphen
            || curr_is_hyphen
            || curr_starts_with_hyphen
            || is_sub_super
            || was_sub_super
            || should_join
            || space_exists)
    }
}

/// A rectangle from a PDF `re` operator (cell boundary, border, etc.).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Rect {
    /// Left edge X coordinate.
    pub x: f32,
    /// Bottom edge Y coordinate.
    pub y: f32,
    /// Width in points.
    pub width: f32,
    /// Height in points.
    pub height: f32,
    /// Page this rectangle belongs to.
    pub page: PageNum,
}

/// A line segment from PDF path operators (`m`/`l`/`S`).
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Line {
    /// Start X coordinate.
    pub x1: f32,
    /// Start Y coordinate.
    pub y1: f32,
    /// End X coordinate.
    pub x2: f32,
    /// End Y coordinate.
    pub y2: f32,
    /// Page this line segment belongs to.
    pub page: PageNum,
}

/// Raw extraction result from a PDF document (before markdown conversion).
#[derive(Debug)]
#[non_exhaustive]
pub struct Extraction {
    /// All text items extracted, in reading order.
    pub items: Vec<TextItem>,
    /// Rectangles from PDF drawing operators (used for table detection).
    pub rects: Vec<Rect>,
    /// Line segments from PDF path operators (used for table detection).
    pub lines: Vec<Line>,
}

/// The final result of converting a PDF to Markdown.
#[derive(Debug)]
#[non_exhaustive]
pub struct Document {
    /// The generated Markdown text.
    pub markdown: String,
    /// Total number of pages in the PDF.
    pub page_count: u32,
    /// Title from PDF metadata, if available.
    pub title: Option<String>,
}

/// Font encoding map: byte codes to Unicode characters.
pub(crate) type FontEncodingMap = HashMap<u8, char>;

/// All font encodings for a page, keyed by font resource name.
pub(crate) type PageFontEncodings = HashMap<String, FontEncodingMap>;

/// Font width information extracted from PDF font dictionaries.
#[derive(Debug, Clone)]
pub(crate) struct FontWidthInfo {
    /// Glyph widths: character code -> width in font units.
    pub(crate) widths: HashMap<u16, u16>,
    /// Default width for glyphs not in the widths table.
    pub(crate) default_width: u16,
    /// Width of the space character (code 32) if known.
    pub(crate) space_width: u16,
    /// Whether this is a CID font (2-byte character codes).
    pub(crate) is_cid: bool,
    /// Scale factor to convert font units to text space units.
    pub(crate) units_scale: f32,
    /// Writing mode: 0 = horizontal (default), 1 = vertical.
    pub(crate) _wmode: u8,
}

/// All font width info for a page, keyed by font resource name.
pub(crate) type PageFontWidths = HashMap<String, FontWidthInfo>;

/// Result tuple from page-level text extraction.
pub(crate) type PageExtraction = (Vec<TextItem>, Vec<Rect>, Vec<Line>);
