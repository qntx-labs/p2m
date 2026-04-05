//! Text processing utilities: Unicode normalization, CJK handling, `BiDi`,
//! and word-boundary spacing detection.

pub mod bidi;
pub mod cjk;
pub mod spacing;
pub mod unicode;

pub use spacing::should_join_items;
