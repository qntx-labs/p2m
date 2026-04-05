//! Markdown conversion pipeline.
//!
//! Transforms extracted [`TextLine`]s into Markdown by detecting headings,
//! lists, code blocks, tables, and inline formatting.

pub mod analysis;
pub mod classify;
pub mod convert;
pub mod postprocess;
pub mod preprocess;
