//! Low-level PDF parsing: document loading, validation, and content extraction.

pub mod adobe_korea1;
pub mod content_stream;
pub mod fonts;
pub mod glyph_names;
pub mod links;
pub mod structure_tree;
pub mod tounicode;
pub mod xobjects;

use std::path::Path;

use lopdf::Document;

use crate::error::{Error, Result};

/// Load a PDF from a file path.
///
/// The document is parsed once and returned along with the page count.
pub fn load_from_path(path: impl AsRef<Path>) -> Result<(Document, u32)> {
    let buffer = std::fs::read(path)?;
    load_from_bytes(&buffer)
}

/// Load a PDF from a byte buffer.
pub fn load_from_bytes(buffer: &[u8]) -> Result<(Document, u32)> {
    validate_bytes(buffer)?;

    let doc = Document::load_mem(buffer)?;
    #[allow(clippy::cast_possible_truncation)]
    let page_count = doc.get_pages().len() as u32;
    Ok((doc, page_count))
}

/// Validate that a byte buffer looks like a PDF.
fn validate_bytes(buffer: &[u8]) -> Result<()> {
    if buffer.is_empty() {
        return Err(Error::NotPdf {
            hint: detect_type_hint(buffer),
        });
    }

    let header = &buffer[..buffer.len().min(1024)];
    let trimmed = strip_bom_and_whitespace(header);

    if trimmed.starts_with(b"%PDF-") {
        Ok(())
    } else {
        Err(Error::NotPdf {
            hint: detect_type_hint(buffer),
        })
    }
}

/// Strip UTF-8 BOM and leading whitespace from a byte slice.
fn strip_bom_and_whitespace(bytes: &[u8]) -> &[u8] {
    let mut b = bytes;
    // Strip UTF-8 BOM.
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        b = &b[3..];
    }
    // Strip leading whitespace.
    while let Some((&first, rest)) = b.split_first() {
        if first.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    b
}

/// Provide a hint about what a non-PDF file actually is.
fn detect_type_hint(buffer: &[u8]) -> String {
    if buffer.is_empty() {
        return "empty file".into();
    }
    let head = &buffer[..buffer.len().min(256)];
    if head.starts_with(b"PK") {
        "ZIP archive (possibly DOCX/XLSX/PPTX)".into()
    } else if head.starts_with(b"\x89PNG") {
        "PNG image".into()
    } else if head.starts_with(b"\xFF\xD8\xFF") {
        "JPEG image".into()
    } else if head.starts_with(b"GIF8") {
        "GIF image".into()
    } else if head.starts_with(b"<!DOCTYPE") || head.starts_with(b"<html") {
        "HTML document".into()
    } else if head.starts_with(b"{") || head.starts_with(b"[") {
        "JSON data".into()
    } else {
        "unknown file type".into()
    }
}

/// Get page height in points from the `MediaBox`.
pub fn page_height(doc: &Document, page_id: lopdf::ObjectId) -> Option<f32> {
    let page_dict = doc.get_dictionary(page_id).ok()?;
    let media_box = page_dict.get(b"MediaBox").ok()?;
    let arr = match media_box {
        lopdf::Object::Array(a) => a,
        lopdf::Object::Reference(r) => match doc.get_object(*r) {
            Ok(lopdf::Object::Array(a)) => a,
            _ => return None,
        },
        _ => return None,
    };
    if arr.len() >= 4 {
        let y1 = obj_to_f32(&arr[1])?;
        let y2 = obj_to_f32(&arr[3])?;
        Some((y2 - y1).abs())
    } else {
        None
    }
}

/// Convert a PDF object to f32.
#[allow(clippy::cast_precision_loss)]
pub const fn obj_to_f32(obj: &lopdf::Object) -> Option<f32> {
    match obj {
        lopdf::Object::Integer(i) => Some(*i as f32),
        lopdf::Object::Real(f) => Some(*f),
        _ => None,
    }
}
