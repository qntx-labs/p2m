//! Tagged PDF structure tree parsing.
//!
//! Reads the `/StructTreeRoot` from the document catalog and builds an
//! in-memory tree of [`StructElement`] nodes. Each leaf maps back to
//! content-stream marked content via MCID (Marked Content ID), which lets
//! downstream code attach semantic roles (heading, paragraph, table cell,
//! list item, ...) to extracted [`TextItem`](crate::types::TextItem)s.

use std::borrow::Cow;
use std::collections::HashMap;

use log::debug;
use lopdf::{Document, Object, ObjectId};

/// Standard PDF structure element types (ISO 32000-1, Table 333-340).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StructRole {
    /// Document root.
    Document,
    /// Part.
    Part,
    /// Article.
    Art,
    /// Section.
    Sect,
    /// Division.
    Div,
    /// Block quote.
    BlockQuote,
    /// Caption.
    Caption,
    /// Table of contents.
    TOC,
    /// Table of contents item.
    TOCI,
    /// Index.
    Index,
    /// Non-structural.
    NonStruct,
    /// Private.
    Private,
    /// Generic heading.
    H,
    /// Heading level 1.
    H1,
    /// Heading level 2.
    H2,
    /// Heading level 3.
    H3,
    /// Heading level 4.
    H4,
    /// Heading level 5.
    H5,
    /// Heading level 6.
    H6,
    /// Paragraph.
    P,
    /// List.
    L,
    /// List item.
    LI,
    /// Label.
    Lbl,
    /// List body.
    LBody,
    /// Table.
    Table,
    /// Table row.
    TR,
    /// Table header cell.
    TH,
    /// Table data cell.
    TD,
    /// Table head group.
    THead,
    /// Table body group.
    TBody,
    /// Table foot group.
    TFoot,
    /// Span.
    Span,
    /// Quote.
    Quote,
    /// Note.
    Note,
    /// Reference.
    Reference,
    /// Bibliography entry.
    BibEntry,
    /// Code.
    Code,
    /// Link.
    Link,
    /// Annotation.
    Annot,
    /// Figure.
    Figure,
    /// Formula.
    Formula,
    /// Form.
    Form,
    /// Ruby (CJK).
    Ruby,
    /// Ruby base.
    RB,
    /// Ruby text.
    RT,
    /// Ruby punctuation.
    RP,
    /// Warichu (CJK).
    Warichu,
    /// Warichu text.
    WT,
    /// Warichu punctuation.
    WP,
    /// Unknown / custom role.
    Other(String),
}

impl StructRole {
    fn from_name(name: &str) -> Self {
        match name {
            "Document" => Self::Document,
            "Part" => Self::Part,
            "Art" => Self::Art,
            "Sect" => Self::Sect,
            "Div" => Self::Div,
            "BlockQuote" => Self::BlockQuote,
            "Caption" => Self::Caption,
            "TOC" => Self::TOC,
            "TOCI" => Self::TOCI,
            "Index" => Self::Index,
            "NonStruct" => Self::NonStruct,
            "Private" => Self::Private,
            "H" => Self::H,
            "H1" => Self::H1,
            "H2" => Self::H2,
            "H3" => Self::H3,
            "H4" => Self::H4,
            "H5" => Self::H5,
            "H6" => Self::H6,
            "P" => Self::P,
            "L" => Self::L,
            "LI" => Self::LI,
            "Lbl" => Self::Lbl,
            "LBody" => Self::LBody,
            "Table" => Self::Table,
            "TR" => Self::TR,
            "TH" => Self::TH,
            "TD" => Self::TD,
            "THead" => Self::THead,
            "TBody" => Self::TBody,
            "TFoot" => Self::TFoot,
            "Span" => Self::Span,
            "Quote" => Self::Quote,
            "Note" => Self::Note,
            "Reference" => Self::Reference,
            "BibEntry" => Self::BibEntry,
            "Code" => Self::Code,
            "Link" => Self::Link,
            "Annot" => Self::Annot,
            "Figure" => Self::Figure,
            "Formula" => Self::Formula,
            "Form" => Self::Form,
            "Ruby" => Self::Ruby,
            "RB" => Self::RB,
            "RT" => Self::RT,
            "RP" => Self::RP,
            "Warichu" => Self::Warichu,
            "WT" => Self::WT,
            "WP" => Self::WP,
            other => Self::Other(other.to_string()),
        }
    }

    /// Resolve a possibly-custom tag name through a role map.
    fn from_name_with_role_map(name: &str, role_map: &HashMap<String, String>) -> Self {
        let mut current = name.to_string();
        for _ in 0..8 {
            let role = Self::from_name(&current);
            if !matches!(role, Self::Other(_)) {
                return role;
            }
            if let Some(mapped) = role_map.get(current.as_str()) {
                current.clone_from(mapped);
            } else {
                return role;
            }
        }
        Self::Other(name.to_string())
    }
}

/// A leaf reference linking a structure element to content-stream content.
#[derive(Debug, Clone)]
pub struct MarkedContentRef {
    /// The Marked Content ID used in the content stream's `BDC`/`BMC`.
    pub mcid: i64,
    /// Page `ObjectId` this content belongs to (from `/Pg` key).
    pub page_id: Option<ObjectId>,
}

/// A node in the PDF structure tree.
#[derive(Debug, Clone)]
pub struct StructElement {
    /// Semantic role (H1, P, Table, TD, ...).
    pub role: StructRole,
    /// Alternative text for figures / illustrations.
    pub alt_text: Option<String>,
    /// Actual text override (e.g. for ligatures).
    pub actual_text: Option<String>,
    /// Language override (e.g. "en-US").
    pub lang: Option<String>,
    /// Direct marked-content references (leaf content).
    pub content_refs: Vec<MarkedContentRef>,
    /// Child structure elements.
    pub children: Vec<StructElement>,
}

/// Parsed PDF structure tree.
///
/// Built from `/StructTreeRoot` in the document catalog. Use
/// [`StructTree::from_doc`] to parse, then [`StructTree::mcid_to_roles`]
/// to get per-page MCID -> role lookup tables.
#[derive(Debug, Clone)]
pub struct StructTree {
    /// Root children (the top-level structure elements).
    pub children: Vec<StructElement>,
}

impl StructTree {
    /// Attempt to parse the structure tree from a PDF document.
    ///
    /// Returns `None` if the PDF is not tagged (no `/StructTreeRoot`).
    pub fn from_doc(doc: &Document) -> Option<Self> {
        let catalog = doc.catalog().ok()?;
        let struct_root_obj = catalog.get(b"StructTreeRoot").ok()?;
        let struct_root = resolve_dict(doc, struct_root_obj)?;

        let role_map = parse_role_map(doc, struct_root);
        debug!("structure tree: {} role map entries", role_map.len());

        let children = parse_kids(doc, struct_root, &role_map, None, 0);
        debug!("structure tree: {} top-level elements", children.len());

        if children.is_empty() {
            return None;
        }

        Some(Self { children })
    }

    /// Build per-page MCID -> `StructRole` lookup.
    ///
    /// Returns a map: page_number (1-indexed) -> (MCID -> `StructRole`).
    pub fn mcid_to_roles(
        &self,
        page_ids: &std::collections::BTreeMap<u32, ObjectId>,
    ) -> HashMap<u32, HashMap<i64, StructRole>> {
        let obj_to_page: HashMap<ObjectId, u32> =
            page_ids.iter().map(|(&num, &id)| (id, num)).collect();

        let mut result: HashMap<u32, HashMap<i64, StructRole>> = HashMap::new();
        Self::collect_mcid_roles(&self.children, &obj_to_page, &mut result);
        result
    }

    fn collect_mcid_roles(
        elements: &[StructElement],
        obj_to_page: &HashMap<ObjectId, u32>,
        result: &mut HashMap<u32, HashMap<i64, StructRole>>,
    ) {
        for elem in elements {
            for mcref in &elem.content_refs {
                if let Some(page_id) = mcref.page_id {
                    if let Some(&page_num) = obj_to_page.get(&page_id) {
                        result
                            .entry(page_num)
                            .or_default()
                            .insert(mcref.mcid, elem.role.clone());
                    }
                }
            }
            Self::collect_mcid_roles(&elem.children, obj_to_page, result);
        }
    }

    /// Count total marked-content references across the tree.
    #[must_use]
    pub fn mcid_count(&self) -> usize {
        fn count(elements: &[StructElement]) -> usize {
            elements
                .iter()
                .map(|e| e.content_refs.len() + count(&e.children))
                .sum()
        }
        count(&self.children)
    }

    /// Build a flat list of structure elements preserving document order.
    #[must_use]
    pub fn flatten(&self) -> Vec<FlatStructElement> {
        let mut out = Vec::new();
        flatten_recursive(&self.children, &mut out, 0);
        out
    }

    /// Extract table structures from the tagged PDF tree.
    #[must_use]
    pub fn extract_tables(
        &self,
        page_ids: &std::collections::BTreeMap<u32, ObjectId>,
    ) -> Vec<StructTable> {
        let obj_to_page: HashMap<ObjectId, u32> =
            page_ids.iter().map(|(&num, &id)| (id, num)).collect();
        let mut tables = Vec::new();
        collect_tables(&self.children, &obj_to_page, &mut tables);
        tables
    }
}

/// A table cell extracted from the structure tree.
#[derive(Debug, Clone)]
pub struct StructTableCell {
    /// Whether this cell is a header cell (`/TH`).
    pub is_header: bool,
    /// MCIDs with their resolved page numbers.
    pub mcids: Vec<(i64, u32)>,
}

/// A table row extracted from the structure tree.
#[derive(Debug, Clone)]
pub struct StructTableRow {
    /// Cells in this row.
    pub cells: Vec<StructTableCell>,
}

/// A complete table extracted from the structure tree.
#[derive(Debug, Clone)]
pub struct StructTable {
    /// Rows in this table.
    pub rows: Vec<StructTableRow>,
}

fn collect_tables(
    elements: &[StructElement],
    obj_to_page: &HashMap<ObjectId, u32>,
    tables: &mut Vec<StructTable>,
) {
    for elem in elements {
        if elem.role == StructRole::Table {
            let mut rows = Vec::new();
            collect_rows(&elem.children, obj_to_page, &mut rows);
            if rows.len() >= 2 && rows.iter().any(|r| !r.cells.is_empty()) {
                tables.push(StructTable { rows });
            }
        } else {
            collect_tables(&elem.children, obj_to_page, tables);
        }
    }
}

fn collect_rows(
    elements: &[StructElement],
    obj_to_page: &HashMap<ObjectId, u32>,
    rows: &mut Vec<StructTableRow>,
) {
    for elem in elements {
        match elem.role {
            StructRole::TR => {
                let mut cells = Vec::new();
                for child in &elem.children {
                    if child.role == StructRole::TD || child.role == StructRole::TH {
                        let is_header = child.role == StructRole::TH;
                        let mut mcids = Vec::new();
                        collect_mcids_recursive(child, obj_to_page, &mut mcids);
                        cells.push(StructTableCell { is_header, mcids });
                    }
                }
                rows.push(StructTableRow { cells });
            }
            StructRole::THead | StructRole::TBody | StructRole::TFoot => {
                collect_rows(&elem.children, obj_to_page, rows);
            }
            _ => {}
        }
    }
}

fn collect_mcids_recursive(
    elem: &StructElement,
    obj_to_page: &HashMap<ObjectId, u32>,
    mcids: &mut Vec<(i64, u32)>,
) {
    for mcref in &elem.content_refs {
        if let Some(page_id) = mcref.page_id {
            if let Some(&page_num) = obj_to_page.get(&page_id) {
                mcids.push((mcref.mcid, page_num));
            }
        }
    }
    for child in &elem.children {
        collect_mcids_recursive(child, obj_to_page, mcids);
    }
}

/// A flattened view of a structure element for linear traversal.
#[derive(Debug, Clone)]
pub struct FlatStructElement {
    /// Semantic role.
    pub role: StructRole,
    /// Nesting depth (0 = top-level).
    pub depth: usize,
    /// Alt text (figures).
    pub alt_text: Option<String>,
    /// Direct MCIDs with page `ObjectId`s.
    pub content_refs: Vec<MarkedContentRef>,
    /// Number of child elements (in the original tree).
    pub child_count: usize,
}

fn flatten_recursive(elements: &[StructElement], out: &mut Vec<FlatStructElement>, depth: usize) {
    for elem in elements {
        out.push(FlatStructElement {
            role: elem.role.clone(),
            depth,
            alt_text: elem.alt_text.clone(),
            content_refs: elem.content_refs.clone(),
            child_count: elem.children.len(),
        });
        flatten_recursive(&elem.children, out, depth + 1);
    }
}

/// Parse the `/RoleMap` dictionary (custom tag -> standard tag).
fn parse_role_map(doc: &Document, struct_root: &lopdf::Dictionary) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(rm_obj) = struct_root.get(b"RoleMap") else {
        return map;
    };
    let Some(rm_dict) = resolve_dict(doc, rm_obj) else {
        return map;
    };
    for (key, val) in rm_dict.iter() {
        let key_str = String::from_utf8_lossy(key).to_string();
        if let Ok(name) = val.as_name() {
            let val_str = String::from_utf8_lossy(name).to_string();
            map.insert(key_str, val_str);
        }
    }
    map
}

/// Max recursion depth for structure tree parsing.
const MAX_DEPTH: usize = 64;

/// Parse child elements from a `/K` entry.
#[allow(clippy::too_many_lines)]
fn parse_kids(
    doc: &Document,
    dict: &lopdf::Dictionary,
    role_map: &HashMap<String, String>,
    inherited_page: Option<ObjectId>,
    depth: usize,
) -> Vec<StructElement> {
    if depth >= MAX_DEPTH {
        return Vec::new();
    }

    let Ok(k_obj) = dict.get(b"K") else {
        return Vec::new();
    };

    let page_id = get_page_ref(doc, dict).or(inherited_page);

    match k_obj {
        Object::Array(arr) => {
            let mut children = Vec::new();
            for item in arr {
                let resolved = resolve_obj(doc, item);
                parse_kid(doc, resolved, role_map, page_id, depth, &mut children);
            }
            children
        }
        other => {
            let resolved = resolve_obj(doc, other);
            let mut children = Vec::new();
            parse_kid(doc, resolved, role_map, page_id, depth, &mut children);
            children
        }
    }
}

/// Parse a single child (struct element dict or MCID integer).
fn parse_kid(
    doc: &Document,
    obj: &Object,
    role_map: &HashMap<String, String>,
    inherited_page: Option<ObjectId>,
    depth: usize,
    out: &mut Vec<StructElement>,
) {
    match obj {
        Object::Integer(mcid) => {
            out.push(StructElement {
                role: StructRole::Span,
                alt_text: None,
                actual_text: None,
                lang: None,
                content_refs: vec![MarkedContentRef {
                    mcid: *mcid,
                    page_id: inherited_page,
                }],
                children: Vec::new(),
            });
        }
        Object::Dictionary(d) => {
            parse_struct_element_dict(doc, d, role_map, inherited_page, depth, out);
        }
        Object::Stream(s) => {
            parse_struct_element_dict(doc, &s.dict, role_map, inherited_page, depth, out);
        }
        _ => {}
    }
}

/// Parse a dictionary that could be a struct element or MCR dict.
#[allow(clippy::too_many_lines)]
fn parse_struct_element_dict(
    doc: &Document,
    dict: &lopdf::Dictionary,
    role_map: &HashMap<String, String>,
    inherited_page: Option<ObjectId>,
    depth: usize,
    out: &mut Vec<StructElement>,
) {
    if depth >= MAX_DEPTH {
        return;
    }

    if is_mcr_dict(dict) {
        if let Ok(Object::Integer(mcid)) = dict.get(b"MCID") {
            let page_id = get_page_ref(doc, dict).or(inherited_page);
            out.push(StructElement {
                role: StructRole::Span,
                alt_text: None,
                actual_text: None,
                lang: None,
                content_refs: vec![MarkedContentRef {
                    mcid: *mcid,
                    page_id,
                }],
                children: Vec::new(),
            });
        }
        return;
    }

    if is_objr_dict(dict) {
        return;
    }

    let role_name = match dict.get(b"S") {
        Ok(s_obj) => {
            let resolved = resolve_obj(doc, s_obj);
            match resolved.as_name() {
                Ok(name) => String::from_utf8_lossy(name).to_string(),
                Err(_) => return,
            }
        }
        Err(_) => return,
    };

    let role = StructRole::from_name_with_role_map(&role_name, role_map);
    let page_id = get_page_ref(doc, dict).or(inherited_page);

    let alt_text = get_text_string(dict, b"Alt");
    let actual_text = get_text_string(dict, b"ActualText");
    let lang = get_text_string(dict, b"Lang");

    let mut content_refs = Vec::new();
    let mut children = Vec::new();

    if let Ok(k_obj) = dict.get(b"K") {
        let k_resolved = resolve_obj(doc, k_obj);
        match k_resolved {
            Object::Integer(mcid) => {
                content_refs.push(MarkedContentRef {
                    mcid: *mcid,
                    page_id,
                });
            }
            Object::Array(arr) => {
                for item in arr {
                    let resolved = resolve_obj(doc, item);
                    match resolved {
                        Object::Integer(mcid) => {
                            content_refs.push(MarkedContentRef {
                                mcid: *mcid,
                                page_id,
                            });
                        }
                        Object::Dictionary(d) => {
                            if is_mcr_dict(d) {
                                if let Ok(Object::Integer(mcid)) = d.get(b"MCID") {
                                    let pg = get_page_ref(doc, d).or(page_id);
                                    content_refs.push(MarkedContentRef {
                                        mcid: *mcid,
                                        page_id: pg,
                                    });
                                }
                            } else if !is_objr_dict(d) {
                                parse_struct_element_dict(
                                    doc,
                                    d,
                                    role_map,
                                    page_id,
                                    depth + 1,
                                    &mut children,
                                );
                            }
                        }
                        Object::Stream(s) => {
                            parse_struct_element_dict(
                                doc,
                                &s.dict,
                                role_map,
                                page_id,
                                depth + 1,
                                &mut children,
                            );
                        }
                        _ => {}
                    }
                }
            }
            Object::Dictionary(d) => {
                if is_mcr_dict(d) {
                    if let Ok(Object::Integer(mcid)) = d.get(b"MCID") {
                        let pg = get_page_ref(doc, d).or(page_id);
                        content_refs.push(MarkedContentRef {
                            mcid: *mcid,
                            page_id: pg,
                        });
                    }
                } else {
                    parse_struct_element_dict(doc, d, role_map, page_id, depth + 1, &mut children);
                }
            }
            _ => {}
        }
    }

    out.push(StructElement {
        role,
        alt_text,
        actual_text,
        lang,
        content_refs,
        children,
    });
}

/// Check if dict has `/Type /MCR`.
fn is_mcr_dict(dict: &lopdf::Dictionary) -> bool {
    dict.get(b"Type")
        .ok()
        .and_then(|o| o.as_name().ok())
        .is_some_and(|n| n == b"MCR")
}

/// Check if dict has `/Type /OBJR`.
fn is_objr_dict(dict: &lopdf::Dictionary) -> bool {
    dict.get(b"Type")
        .ok()
        .and_then(|o| o.as_name().ok())
        .is_some_and(|n| n == b"OBJR")
}

/// Get the `/Pg` page reference from a dictionary.
fn get_page_ref(doc: &Document, dict: &lopdf::Dictionary) -> Option<ObjectId> {
    let pg = dict.get(b"Pg").ok()?;
    match pg {
        Object::Reference(id) => Some(*id),
        _ => {
            let resolved = resolve_obj(doc, pg);
            if let Object::Reference(id) = resolved {
                Some(*id)
            } else {
                None
            }
        }
    }
}

/// Extract a text string from a dictionary key.
fn get_text_string(dict: &lopdf::Dictionary, key: &[u8]) -> Option<String> {
    let obj = dict.get(key).ok()?;
    match obj {
        Object::String(bytes, _) => Some(crate::text::unicode::decode_text_string(bytes)),
        _ => None,
    }
}

/// Resolve an Object reference.
fn resolve_obj<'a>(doc: &'a Document, obj: &'a Object) -> &'a Object {
    match obj {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(obj),
        _ => obj,
    }
}

/// Resolve an Object to a dictionary.
fn resolve_dict<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a lopdf::Dictionary> {
    match obj {
        Object::Dictionary(d) => Some(d),
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        _ => None,
    }
}

/// Fix malformed structure element `/S` entries in raw PDF bytes.
///
/// Some PDF generators (notably fpdf2) write bare names like `/S Code`
/// instead of the correct `/S /Code`. This function scans for the pattern
/// and prepends `/` to make them valid PDF names.
///
/// Returns `Cow::Borrowed` if no fixes were needed.
pub fn fix_bare_struct_names(buf: &[u8]) -> Cow<'_, [u8]> {
    if !contains_bytes(buf, b"/StructTreeRoot") {
        return Cow::Borrowed(buf);
    }

    const KNOWN_NAMES: &[&[u8]] = &[
        b"Document",
        b"Part",
        b"Art",
        b"Sect",
        b"Div",
        b"BlockQuote",
        b"Caption",
        b"TOC",
        b"TOCI",
        b"Index",
        b"NonStruct",
        b"Private",
        b"H",
        b"H1",
        b"H2",
        b"H3",
        b"H4",
        b"H5",
        b"H6",
        b"P",
        b"L",
        b"LI",
        b"Lbl",
        b"LBody",
        b"Table",
        b"TR",
        b"TH",
        b"TD",
        b"THead",
        b"TBody",
        b"TFoot",
        b"Span",
        b"Quote",
        b"Note",
        b"Reference",
        b"BibEntry",
        b"Code",
        b"Link",
        b"Annot",
        b"Figure",
        b"Formula",
        b"Form",
        b"Ruby",
        b"RB",
        b"RT",
        b"RP",
        b"Warichu",
        b"WT",
        b"WP",
    ];

    let pattern = b"/S ";
    let mut result: Option<Vec<u8>> = None;
    let mut pos = 0;

    while pos + pattern.len() < buf.len() {
        let Some(idx) = find_bytes(&buf[pos..], pattern).map(|i| i + pos) else {
            break;
        };

        let after = idx + pattern.len();
        if after < buf.len() && buf[after] == b'/' {
            pos = after;
            continue;
        }

        let mut matched = false;
        for name in KNOWN_NAMES {
            let end = after + name.len();
            if end <= buf.len()
                && &buf[after..end] == *name
                && (end >= buf.len() || matches!(buf[end], b'\n' | b'\r' | b' ' | b'/' | b'>'))
            {
                let out = result.get_or_insert_with(|| buf[..after].to_vec());
                if out.len() < after {
                    out.extend_from_slice(&buf[out.len()..after]);
                }
                out.push(b'/');
                out.extend_from_slice(name);
                pos = end;
                matched = true;
                debug!(
                    "fix_bare_struct_names: patched /S {} -> /S /{}",
                    String::from_utf8_lossy(name),
                    String::from_utf8_lossy(name)
                );
                break;
            }
        }

        if !matched {
            pos = after;
        }
    }

    match result {
        Some(mut out) => {
            if out.len() < buf.len() {
                out.extend_from_slice(&buf[out.len()..]);
            }
            Cow::Owned(out)
        }
        None => Cow::Borrowed(buf),
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    find_bytes(haystack, needle).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_role_from_name() {
        assert_eq!(StructRole::from_name("H1"), StructRole::H1);
        assert_eq!(StructRole::from_name("P"), StructRole::P);
        assert_eq!(StructRole::from_name("Table"), StructRole::Table);
        assert_eq!(
            StructRole::from_name("CustomTag"),
            StructRole::Other("CustomTag".to_string())
        );
    }

    #[test]
    fn struct_role_with_role_map() {
        let mut role_map = HashMap::new();
        role_map.insert("Heading1".to_string(), "H1".to_string());
        role_map.insert("Body".to_string(), "P".to_string());
        role_map.insert("MyTag".to_string(), "Heading1".to_string());

        assert_eq!(
            StructRole::from_name_with_role_map("Heading1", &role_map),
            StructRole::H1
        );
        assert_eq!(
            StructRole::from_name_with_role_map("MyTag", &role_map),
            StructRole::H1
        );
        assert_eq!(
            StructRole::from_name_with_role_map("H2", &role_map),
            StructRole::H2
        );
    }

    #[test]
    fn struct_role_role_map_cycle() {
        let mut role_map = HashMap::new();
        role_map.insert("A".to_string(), "B".to_string());
        role_map.insert("B".to_string(), "A".to_string());

        let role = StructRole::from_name_with_role_map("A", &role_map);
        assert!(matches!(role, StructRole::Other(_)));
    }

    #[test]
    fn fix_bare_struct_names_patches() {
        let input = b"/StructTreeRoot /S Code\n/Type /StructElem";
        let fixed = fix_bare_struct_names(input);
        assert!(fixed.windows(b"/S /Code".len()).any(|w| w == b"/S /Code"));
    }

    #[test]
    fn fix_bare_struct_names_already_correct() {
        let input = b"/StructTreeRoot /S /Code\n/Type /StructElem";
        let fixed = fix_bare_struct_names(input);
        assert!(matches!(fixed, Cow::Borrowed(_)));
    }

    #[test]
    fn fix_bare_struct_names_unknown_not_touched() {
        let input = b"/StructTreeRoot /S FooBar\n";
        let fixed = fix_bare_struct_names(input);
        let s = String::from_utf8_lossy(&fixed);
        assert!(s.contains("/S FooBar"));
    }

    #[test]
    fn fix_bare_struct_names_no_tree() {
        let input = b"/S Code\nno struct tree";
        let fixed = fix_bare_struct_names(input);
        assert!(matches!(fixed, Cow::Borrowed(_)));
    }
}
