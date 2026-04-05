//! Markdown conversion pipeline.
//!
//! Transforms extracted [`TextLine`]s into Markdown by detecting headings,
//! lists, code blocks, tables, and inline formatting.

mod analysis;
mod classify;
mod convert;
mod postprocess;
mod preprocess;

pub use convert::to_markdown_from_lines;
