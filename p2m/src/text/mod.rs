//! Text processing utilities: Unicode normalization, CJK handling, `BiDi`,
//! and word-boundary spacing detection.

mod bidi;
mod cjk;
mod spacing;
mod unicode;

pub use spacing::should_join_items;
