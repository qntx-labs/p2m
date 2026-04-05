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
//! let doc = p2m::convert_with("document.pdf", opts).unwrap();
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

#![doc = include_str!("../../README.md")]
// TODO: remove once the pipeline is fully wired up.
#![allow(dead_code)]

pub mod error;
pub mod options;
pub mod types;

mod extract;
mod markdown;
mod pdf;
mod table;
pub(crate) mod text;

pub use error::{Error, Result};
pub use options::{MarkdownOptions, Options};
pub use types::{Document, Extraction, ItemKind, Line, PageNum, Rect, TextItem, TextLine};

use std::path::Path;

/// Convert a PDF file to Markdown with default options.
///
/// This is the simplest entry point. For customisation, see [`convert_with`].
///
/// # Errors
///
/// Returns [`Error`] if the file cannot be read, is not a valid PDF, or
/// extraction fails.
pub fn convert(path: impl AsRef<Path>) -> Result<Document> {
    convert_with(path, Options::default())
}

/// Convert a PDF file to Markdown with custom options.
///
/// # Errors
///
/// Returns [`Error`] if the file cannot be read, is not a valid PDF, or
/// extraction fails.
pub fn convert_with(path: impl AsRef<Path>, _options: Options) -> Result<Document> {
    let (_doc, page_count) = pdf::load_from_path(path)?;

    // TODO: implement full pipeline
    Ok(Document {
        markdown: String::new(),
        page_count,
        title: None,
    })
}

/// Convert a PDF from a byte buffer to Markdown with default options.
///
/// # Errors
///
/// Returns [`Error`] if the buffer is not a valid PDF or extraction fails.
pub fn convert_bytes(buffer: &[u8]) -> Result<Document> {
    convert_bytes_with(buffer, Options::default())
}

/// Convert a PDF from a byte buffer to Markdown with custom options.
///
/// # Errors
///
/// Returns [`Error`] if the buffer is not a valid PDF or extraction fails.
pub fn convert_bytes_with(buffer: &[u8], _options: Options) -> Result<Document> {
    let (_doc, page_count) = pdf::load_from_bytes(buffer)?;

    // TODO: implement full pipeline
    Ok(Document {
        markdown: String::new(),
        page_count,
        title: None,
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
    let (_doc, _page_count) = pdf::load_from_path(path)?;

    // TODO: implement extraction pipeline
    Ok(Extraction {
        items: Vec::new(),
        rects: Vec::new(),
        lines: Vec::new(),
    })
}

/// Extract positioned text items from a PDF byte buffer.
///
/// # Errors
///
/// Returns [`Error`] if the buffer is not a valid PDF or extraction fails.
pub fn extract_bytes(buffer: &[u8]) -> Result<Extraction> {
    let (_doc, _page_count) = pdf::load_from_bytes(buffer)?;

    // TODO: implement extraction pipeline
    Ok(Extraction {
        items: Vec::new(),
        rects: Vec::new(),
        lines: Vec::new(),
    })
}
