//! PDF to Markdown converter.
//!
//! `p2m` extracts text from PDF documents and converts it to clean Markdown,
//! handling headings, lists, tables, bold/italic formatting, and more.
//!
//! # Quick start
//!
//! ```no_run
//! // Convert a PDF file to Markdown with default options.
//! let doc = p2m::convert("document.pdf").unwrap();
//! println!("{}", doc.markdown);
//!
//! // With custom options:
//! let opts = p2m::Options::new().pages([1, 3, 5]);
//! let doc = p2m::convert_with("document.pdf", &opts).unwrap();
//!
//! // From bytes:
//! let pdf_bytes = std::fs::read("document.pdf").unwrap();
//! let doc = p2m::convert_bytes(&pdf_bytes).unwrap();
//!
//! // Low-level: extract positioned text items (no markdown).
//! let extraction = p2m::extract("document.pdf").unwrap();
//! for item in &extraction.items {
//!     println!("{}: ({}, {}) {}", item.page, item.x, item.y, item.text);
//! }
//! ```

pub mod error;
pub mod options;
pub mod types;

mod extract;
mod markdown;
mod pdf;
pub mod quality;
mod tables;
pub(crate) mod text;

use std::collections::{HashMap, HashSet};
use std::path::Path;

pub use error::{Error, Result};
use extract::layout::group_into_lines;
use extract::text::{merge_subscript_items, merge_text_items};
use markdown::to_markdown_from_lines;
pub use options::{MarkdownOptions, Options};
use pdf::structure_tree::StructTree;
use pdf::tounicode::FontCMaps;
pub use types::{Document, Extraction, ItemKind, Line, PageNum, Rect, TextItem, TextLine};

/// Convert a PDF file to Markdown with default options.
///
/// This is the simplest entry point. For customisation, see [`convert_with`].
///
/// # Errors
///
/// Returns [`Error`] if the file cannot be read, is not a valid PDF, or
/// extraction fails.
pub fn convert(path: impl AsRef<Path>) -> Result<Document> {
    convert_with(path, &Options::default())
}

/// Convert a PDF file to Markdown with custom options.
///
/// # Errors
///
/// Returns [`Error`] if the file cannot be read, is not a valid PDF, or
/// extraction fails.
pub fn convert_with(path: impl AsRef<Path>, options: &Options) -> Result<Document> {
    let buffer = std::fs::read(path)?;
    convert_bytes_with(&buffer, options)
}

/// Convert a PDF from a byte buffer to Markdown with default options.
///
/// # Errors
///
/// Returns [`Error`] if the buffer is not a valid PDF or extraction fails.
pub fn convert_bytes(buffer: &[u8]) -> Result<Document> {
    convert_bytes_with(buffer, &Options::default())
}

/// Convert a PDF from a byte buffer to Markdown with custom options.
///
/// # Errors
///
/// Returns [`Error`] if the buffer is not a valid PDF or extraction fails.
pub fn convert_bytes_with(buffer: &[u8], options: &Options) -> Result<Document> {
    let (doc, page_count) = pdf::load_from_bytes(buffer)?;

    let page_filter: Option<HashSet<u32>> = options.page_filter.clone();
    let font_cmaps = FontCMaps::from_doc_pages(&doc, page_filter.as_ref());

    let ((raw_items, rects, lines), _page_thresholds) =
        extract::extract_positioned_text(&doc, &font_cmaps, page_filter.as_ref())?;

    // Invisible text fallback: if initial extraction produces garbage, retry
    // including invisible (Tr=3) text which may be an OCR layer.
    let (raw_items, rects, lines) = {
        let sample: String = raw_items.iter().take(200).map(|i| i.text.as_str()).collect();
        if quality::is_garbage_text(&sample) || sample.trim().is_empty() {
            log::debug!("initial extraction is garbage — retrying with invisible text");
            let ((retry_items, retry_rects, retry_lines), _) =
                extract::extract_positioned_text_include_invisible(
                    &doc,
                    &font_cmaps,
                    page_filter.as_ref(),
                )?;
            (retry_items, retry_rects, retry_lines)
        } else {
            (raw_items, rects, lines)
        }
    };

    let merged = merge_text_items(raw_items);
    let items = merge_subscript_items(merged);

    let struct_tree = StructTree::from_doc(&doc);
    let page_ids = doc.get_pages();
    let struct_roles = struct_tree.as_ref().map(|tree| {
        let raw = tree.mcid_to_roles(&page_ids);
        raw.into_iter()
            .map(|(page_num, roles)| (PageNum::new(page_num), roles))
            .collect::<HashMap<_, _>>()
    });
    let struct_tables = struct_tree
        .as_ref()
        .map(|tree| tree.extract_tables(&page_ids))
        .unwrap_or_default();

    let base_font_size = {
        let mut counts: HashMap<i32, usize> = HashMap::new();
        for item in &items {
            #[allow(clippy::cast_possible_truncation)]
            let key = (item.font_size * 10.0) as i32;
            *counts.entry(key).or_insert(0) += 1;
        }
        counts
            .iter()
            .max_by(|(sa, ca), (sb, cb)| ca.cmp(cb).then_with(|| sb.cmp(sa)))
            .map_or(12.0, |(s, _)| *s as f32 / 10.0)
    };

    let page_tables =
        tables::detect_tables(&items, &rects, &lines, &struct_tables, base_font_size);

    let text_lines = group_into_lines(items);

    let markdown = to_markdown_from_lines(
        text_lines,
        &options.markdown,
        page_tables,
        struct_roles.as_ref(),
    );

    let title = doc
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|info| match info {
            lopdf::Object::Reference(r) => doc.get_dictionary(*r).ok(),
            _ => None,
        })
        .and_then(|info_dict| info_dict.get(b"Title").ok())
        .and_then(|t| match t {
            lopdf::Object::String(s, _) => String::from_utf8(s.clone()).ok(),
            _ => None,
        })
        .filter(|t| !t.trim().is_empty());

    let quality = quality::check_quality(&markdown);

    Ok(Document {
        markdown,
        page_count,
        title,
        quality,
    })
}

/// Extract positioned text items from a PDF file without markdown conversion.
///
/// This is useful for building custom rendering pipelines or debugging
/// extraction quality.
///
/// # Errors
///
/// Returns [`Error`] if the file cannot be read, is not a valid PDF, or
/// extraction fails.
pub fn extract(path: impl AsRef<Path>) -> Result<Extraction> {
    let buffer = std::fs::read(path)?;
    extract_bytes(&buffer)
}

/// Extract positioned text items from a PDF byte buffer.
///
/// # Errors
///
/// Returns [`Error`] if the buffer is not a valid PDF or extraction fails.
pub fn extract_bytes(buffer: &[u8]) -> Result<Extraction> {
    let (doc, _page_count) = pdf::load_from_bytes(buffer)?;

    let font_cmaps = FontCMaps::from_doc(&doc);

    let ((raw_items, rects, lines), _page_thresholds) =
        extract::extract_positioned_text(&doc, &font_cmaps, None)?;

    let merged = merge_text_items(raw_items);
    let items = merge_subscript_items(merged);

    Ok(Extraction {
        items,
        rects,
        lines,
    })
}
