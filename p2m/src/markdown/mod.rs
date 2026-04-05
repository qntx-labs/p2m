//! Markdown conversion pipeline.
//!
//! Transforms extracted [`TextLine`]s into Markdown by detecting headings,
//! lists, code blocks, tables, and inline formatting.

pub(crate) mod analysis;
mod classify;
mod convert;
mod postprocess;
mod preprocess;

pub use convert::to_markdown_from_lines;
pub(crate) use convert::to_markdown_with_tables_and_images;
pub(crate) use preprocess::strip_repeated_lines;
