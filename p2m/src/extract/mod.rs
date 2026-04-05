//! Per-page extraction orchestrator.
//!
//! Iterates over document pages, extracts text items via the content stream
//! parser, and merges results into a unified [`Extraction`].

pub mod layout;
pub mod text;
