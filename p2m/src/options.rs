//! Configuration for PDF-to-Markdown conversion.

use std::collections::HashSet;

/// Top-level configuration for [`crate::convert_with`] and related functions.
///
/// # Examples
///
/// ```
/// let opts = p2m::Options::new().pages([1, 3, 5]);
/// ```
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Options {
    /// Markdown formatting options.
    pub markdown: MarkdownOptions,
    /// Optional set of 1-indexed pages to process. `None` means all pages.
    pub page_filter: Option<HashSet<u32>>,
}

impl Options {
    /// Create options with all defaults.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set markdown formatting options.
    #[must_use]
    pub const fn markdown(mut self, options: MarkdownOptions) -> Self {
        self.markdown = options;
        self
    }

    /// Limit processing to specific 1-indexed pages.
    #[must_use]
    pub fn pages(mut self, pages: impl IntoIterator<Item = u32>) -> Self {
        self.page_filter = Some(pages.into_iter().collect());
        self
    }
}

/// Controls which markdown features are detected and emitted.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools, clippy::module_name_repetitions)]
pub struct MarkdownOptions {
    /// Detect headers by font size.
    pub detect_headers: bool,
    /// Detect list items (bullet, numbered, lettered).
    pub detect_lists: bool,
    /// Detect code blocks via monospace font detection.
    pub detect_code: bool,
    /// Base font size override for heading detection. `None` means auto-detect.
    pub base_font_size: Option<f32>,
    /// Remove standalone page numbers from output.
    pub remove_page_numbers: bool,
    /// Convert bare URLs to markdown links.
    pub format_urls: bool,
    /// Fix hyphenation (rejoin words broken across lines).
    pub fix_hyphenation: bool,
    /// Detect and format bold text from font names.
    pub detect_bold: bool,
    /// Detect and format italic text from font names.
    pub detect_italic: bool,
    /// Include image placeholders in output.
    pub include_images: bool,
    /// Include extracted hyperlinks.
    pub include_links: bool,
    /// Insert page break markers (`<!-- Page N -->`) between pages.
    pub include_page_breaks: bool,
    /// Strip repeated headers/footers that appear on many pages.
    pub strip_headers_footers: bool,
}

impl Default for MarkdownOptions {
    fn default() -> Self {
        Self {
            detect_headers: true,
            detect_lists: true,
            detect_code: true,
            base_font_size: None,
            remove_page_numbers: true,
            format_urls: true,
            fix_hyphenation: true,
            detect_bold: true,
            detect_italic: true,
            include_images: true,
            include_links: true,
            include_page_breaks: false,
            strip_headers_footers: true,
        }
    }
}
