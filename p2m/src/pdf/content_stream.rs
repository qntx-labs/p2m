//! PDF content stream operator state machine.
//!
//! Walks the PDF content stream and emits [`TextItem`]s, [`Rect`]s, and
//! [`Line`]s for each page.
