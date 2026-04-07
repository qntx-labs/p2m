//! Hyperlink and form field annotation extraction.
//!
//! Extracts link annotations and form fields from PDF pages, converting them
//! into [`TextItem`]s with [`ItemKind::Link`] or [`ItemKind::FormField`].

use lopdf::{Document, Object, ObjectId};

use crate::pdf::obj_to_f32;
use crate::types::{ItemKind, PageNum, TextItem};

/// Extract hyperlink annotations from a page.
///
/// Returns `TextItem`s positioned at the link's bounding box with
/// `ItemKind::Link(url)`.
pub fn extract_page_links(doc: &Document, page_id: ObjectId, page_num: u32) -> Vec<TextItem> {
    let mut items = Vec::new();

    let Ok(page_dict) = doc.get_dictionary(page_id) else {
        return items;
    };

    let annots = match page_dict.get(b"Annots") {
        Ok(Object::Array(arr)) => arr.clone(),
        Ok(Object::Reference(r)) => match doc.get_object(*r) {
            Ok(Object::Array(arr)) => arr.clone(),
            _ => return items,
        },
        _ => return items,
    };

    for annot_obj in &annots {
        let annot_dict = match annot_obj {
            Object::Reference(r) => match doc.get_dictionary(*r) {
                Ok(d) => d,
                _ => continue,
            },
            Object::Dictionary(d) => d,
            _ => continue,
        };

        // Must be a /Link annotation.
        let is_link = annot_dict
            .get(b"Subtype")
            .ok()
            .and_then(|o| o.as_name().ok())
            .is_some_and(|n| n == b"Link");
        if !is_link {
            continue;
        }

        // Extract URI from /A (Action) dict.
        let Some(uri) = extract_link_uri(doc, annot_dict) else {
            continue;
        };

        // Extract bounding box /Rect [x1, y1, x2, y2].
        let rect = match annot_dict.get(b"Rect") {
            Ok(Object::Array(arr)) if arr.len() >= 4 => {
                let x1 = obj_to_f32(&arr[0]).unwrap_or(0.0);
                let y1 = obj_to_f32(&arr[1]).unwrap_or(0.0);
                let x2 = obj_to_f32(&arr[2]).unwrap_or(0.0);
                let y2 = obj_to_f32(&arr[3]).unwrap_or(0.0);
                (x1, y1, x2, y2)
            }
            _ => continue,
        };

        items.push(TextItem {
            text: uri.clone(),
            x: rect.0,
            y: rect.1,
            width: rect.2 - rect.0,
            height: rect.3 - rect.1,
            font: String::new(),
            font_size: 0.0,
            page: PageNum::new(page_num),
            bold: false,
            italic: false,
            kind: ItemKind::Link(uri),
            mcid: None,
        });
    }

    items
}

/// Extract URI from a link annotation's `/A` (Action) dictionary.
fn extract_link_uri(doc: &Document, annot_dict: &lopdf::Dictionary) -> Option<String> {
    let a_obj = annot_dict.get(b"A").ok()?;
    let a_dict = match a_obj {
        Object::Dictionary(d) => d,
        Object::Reference(r) => doc.get_dictionary(*r).ok()?,
        _ => return None,
    };

    let uri_obj = a_dict.get(b"URI").ok()?;
    match uri_obj {
        Object::String(bytes, _) => Some(String::from_utf8_lossy(bytes).to_string()),
        _ => None,
    }
}

/// Extract form fields from the document's `/AcroForm`.
///
/// Returns `TextItem`s with `ItemKind::FormField` for each filled field.
#[allow(clippy::cast_possible_truncation)]
pub fn extract_form_fields(
    doc: &Document,
    page_id_to_num: &std::collections::HashMap<ObjectId, u32>,
) -> Vec<TextItem> {
    let mut items = Vec::new();

    let Ok(catalog) = doc.catalog() else {
        return items;
    };
    let Ok(acro_obj) = catalog.get(b"AcroForm") else {
        return items;
    };
    let acro_dict = match acro_obj {
        Object::Dictionary(d) => d,
        Object::Reference(r) => match doc.get_dictionary(*r) {
            Ok(d) => d,
            _ => return items,
        },
        _ => return items,
    };

    let fields = match acro_dict.get(b"Fields") {
        Ok(Object::Array(arr)) => arr,
        Ok(Object::Reference(r)) => match doc.get_object(*r) {
            Ok(Object::Array(arr)) => arr,
            _ => return items,
        },
        _ => return items,
    };

    walk_form_fields(doc, fields, page_id_to_num, &mut items, &mut String::new());
    items
}

/// Recursively walk form fields.
fn walk_form_fields(
    doc: &Document,
    fields: &[Object],
    page_id_to_num: &std::collections::HashMap<ObjectId, u32>,
    items: &mut Vec<TextItem>,
    parent_name: &mut String,
) {
    for field_obj in fields {
        let field_dict = match field_obj {
            Object::Reference(r) => match doc.get_dictionary(*r) {
                Ok(d) => d,
                _ => continue,
            },
            Object::Dictionary(d) => d,
            _ => continue,
        };

        // Build fully qualified field name.
        let partial_name = field_dict
            .get(b"T")
            .ok()
            .and_then(|o| match o {
                Object::String(bytes, _) => Some(String::from_utf8_lossy(bytes).to_string()),
                _ => None,
            })
            .unwrap_or_default();

        let fq_name = if parent_name.is_empty() {
            partial_name.clone()
        } else if partial_name.is_empty() {
            parent_name.clone()
        } else {
            format!("{parent_name}.{partial_name}")
        };

        // Check for /Kids (field group) -> recurse.
        if let Ok(Object::Array(kids)) = field_dict.get(b"Kids") {
            let has_widget_kids = kids.iter().any(|k| {
                if let Object::Reference(r) = k {
                    doc.get_dictionary(*r)
                        .ok()
                        .and_then(|d| d.get(b"Subtype").ok())
                        .and_then(|o| o.as_name().ok())
                        .is_some_and(|n| n == b"Widget")
                } else {
                    false
                }
            });
            if !has_widget_kids {
                let mut name = fq_name;
                walk_form_fields(doc, kids, page_id_to_num, items, &mut name);
                continue;
            }
        }

        // Extract field value based on /FT (field type).
        let ft = field_dict
            .get(b"FT")
            .ok()
            .and_then(|o| o.as_name().ok())
            .unwrap_or(b"");

        let value = match ft {
            b"Tx" | b"Ch" => field_dict.get(b"V").ok().and_then(|o| match o {
                Object::String(bytes, _) => Some(String::from_utf8_lossy(bytes).to_string()),
                _ => None,
            }),
            b"Btn" => field_dict
                .get(b"V")
                .ok()
                .and_then(|o| o.as_name().ok())
                .map(|n| String::from_utf8_lossy(n).to_string()),
            _ => None,
        };

        let Some(value) = value else {
            continue;
        };

        if value.is_empty() || value == "Off" {
            continue;
        }

        // Position from /Rect.
        let (x, y, w, h) = match field_dict.get(b"Rect") {
            Ok(Object::Array(arr)) if arr.len() >= 4 => {
                let x1 = obj_to_f32(&arr[0]).unwrap_or(0.0);
                let y1 = obj_to_f32(&arr[1]).unwrap_or(0.0);
                let x2 = obj_to_f32(&arr[2]).unwrap_or(0.0);
                let y2 = obj_to_f32(&arr[3]).unwrap_or(0.0);
                (x1, y1, x2 - x1, y2 - y1)
            }
            _ => (0.0, 0.0, 0.0, 0.0),
        };

        // Determine page.
        let page_num = field_dict
            .get(b"P")
            .ok()
            .and_then(|o| match o {
                Object::Reference(r) => page_id_to_num.get(r).copied(),
                _ => None,
            })
            .unwrap_or(1);

        let text = if fq_name.is_empty() {
            value
        } else {
            format!("{fq_name}: {value}")
        };

        items.push(TextItem {
            text,
            x,
            y,
            width: w,
            height: h,
            font: String::new(),
            font_size: 10.0,
            page: PageNum::new(page_num),
            bold: false,
            italic: false,
            kind: ItemKind::FormField,
            mcid: None,
        });
    }
}
