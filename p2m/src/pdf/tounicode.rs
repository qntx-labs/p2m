//! `CMap` and `ToUnicode` parsing for CID font decoding.

use std::collections::HashMap;

/// A single entry in a CMap mapping.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub(crate) struct CMapEntry {
    /// Source code (character code from the font).
    pub(crate) src_code: u32,
    /// Destination Unicode string.
    pub(crate) dst_string: String,
}

/// A parsed `ToUnicode` CMap.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub(crate) struct ToUnicodeCMap {
    /// Mappings from character codes to Unicode strings.
    pub(crate) mappings: HashMap<u32, String>,
}

impl ToUnicodeCMap {
    /// Look up a character code and return the corresponding Unicode string.
    pub(crate) fn decode(&self, code: u32) -> Option<&str> {
        self.mappings.get(&code).map(String::as_str)
    }
}

/// All font CMaps for a page, keyed by font resource name.
///
/// Each font name maps to a pair: `(primary_cmap, Option<remapped_cmap>)`.
/// The remapped CMap is built from `FontFile2` GID-to-Unicode mapping when the
/// primary CMap produces poor results.
pub(crate) type FontCMaps = HashMap<String, (ToUnicodeCMap, Option<ToUnicodeCMap>)>;
