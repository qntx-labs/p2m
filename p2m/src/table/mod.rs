//! Table detection and formatting.
//!
//! Provides the [`TableDetector`] trait and three detection strategies
//! (rect-based, line-based, heuristic) run in priority order.

pub mod detect_heuristic;
pub mod detect_lines;
pub mod detect_rects;
pub mod detect_struct;
pub mod format;
pub mod grid;

use crate::types::{Line, PageNum, Rect, TextItem};

/// A detected table with cells arranged in a grid.
#[derive(Debug, Clone)]
pub struct Table {
    /// Column boundary X coordinates.
    pub(crate) columns: Vec<f32>,
    /// Row boundary Y coordinates.
    pub(crate) rows: Vec<f32>,
    /// Cell contents as `rows × columns` grid of strings.
    pub(crate) cells: Vec<Vec<String>>,
    /// Indices into the source `TextItem` slice claimed by this table.
    pub(crate) item_indices: Vec<usize>,
}

/// Strategy for detecting tables in extracted content.
#[allow(clippy::module_name_repetitions)]
pub trait TableDetector {
    /// Detect tables among the given items on a specific page.
    fn detect(
        &self,
        items: &[TextItem],
        rects: &[Rect],
        lines: &[Line],
        page: PageNum,
    ) -> Vec<Table>;
}
