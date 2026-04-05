//! Form `XObject` and image detection.
//!
//! Provides helpers to categorize page-level `XObject` resources as
//! Image or Form references, used by the content stream processor
//! to decide how to handle `Do` operators.

use lopdf::{Document, Object, ObjectId};

/// Classification of a page-level `XObject`.
#[derive(Debug, Clone)]
pub enum XObjectKind {
    /// An image `XObject`.
    Image,
    /// A Form `XObject` that may contain text (recursive extraction needed).
    Form(ObjectId),
}

/// Retrieve `XObject`s from a page's `/Resources/XObject` dictionary.
///
/// Returns a map of resource name -> `XObjectKind`.
pub fn get_page_xobjects(
    doc: &Document,
    page_id: ObjectId,
) -> std::collections::HashMap<Vec<u8>, XObjectKind> {
    let mut result = std::collections::HashMap::new();

    let Ok(page_dict) = doc.get_dictionary(page_id) else {
        return result;
    };

    let resources = match page_dict.get(b"Resources") {
        Ok(Object::Dictionary(d)) => d,
        Ok(Object::Reference(r)) => match doc.get_dictionary(*r) {
            Ok(d) => d,
            _ => return result,
        },
        _ => return result,
    };

    let xobject_dict = match resources.get(b"XObject") {
        Ok(Object::Dictionary(d)) => d,
        Ok(Object::Reference(r)) => match doc.get_dictionary(*r) {
            Ok(d) => d,
            _ => return result,
        },
        _ => return result,
    };

    collect_xobjects_from_dict(doc, xobject_dict, &mut result);
    result
}

/// Categorize entries in an `XObject` dictionary.
fn collect_xobjects_from_dict(
    doc: &Document,
    dict: &lopdf::Dictionary,
    result: &mut std::collections::HashMap<Vec<u8>, XObjectKind>,
) {
    for (name, obj) in dict.iter() {
        let obj_id = match obj {
            Object::Reference(r) => *r,
            _ => continue,
        };

        let Ok(xobj_dict) = doc.get_dictionary(obj_id) else {
            continue;
        };

        let subtype = xobj_dict
            .get(b"Subtype")
            .ok()
            .and_then(|o| o.as_name().ok());

        let kind = match subtype {
            Some(b"Image") => XObjectKind::Image,
            Some(b"Form") => XObjectKind::Form(obj_id),
            _ => continue,
        };

        result.insert(name.clone(), kind);
    }
}
