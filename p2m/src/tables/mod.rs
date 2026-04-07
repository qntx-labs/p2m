//! Table detection and Markdown rendering.
//!
//! Orchestrates multiple detection strategies in priority order:
//! 1. **Rect-based**: PDF `re` rectangles clustered into table regions (union-find).
//! 2. **Line-based**: Horizontal/vertical line segments forming grid intersections.
//! 3. **Structure-tree**: Tagged PDF `/Table` elements (if available).
//!
//! The first strategy that produces a valid table wins for each page region.

mod detect_lines;
mod detect_rects;
mod detect_struct;
mod format;
mod grid;

use std::collections::HashMap;

use crate::pdf::structure_tree::StructTable;
use crate::types::{Line, PageNum, Rect, TextItem};

/// A detected table with its cells arranged in rows and columns.
#[derive(Debug, Clone)]
pub struct Table {
    /// Row-major cell grid. `cells[row][col]` is the cell text.
    pub cells: Vec<Vec<String>>,
    /// Y coordinate of the table's top edge (for insertion ordering).
    pub y_top: f32,
    /// Page this table belongs to.
    #[allow(dead_code)]
    pub page: PageNum,
}

/// Detect tables on all pages and return per-page tables with Y positions
/// and pre-rendered Markdown.
pub fn detect_tables(
    items: &[TextItem],
    rects: &[Rect],
    lines: &[Line],
    struct_tables: &[StructTable],
) -> HashMap<PageNum, Vec<(f32, String)>> {
    let mut pages: Vec<PageNum> = items.iter().map(|i| i.page).collect();
    pages.sort();
    pages.dedup();

    let mut result: HashMap<PageNum, Vec<(f32, String)>> = HashMap::new();

    for &page in &pages {
        let page_items: Vec<&TextItem> = items.iter().filter(|i| i.page == page).collect();
        let page_rects: Vec<&Rect> = rects.iter().filter(|r| r.page == page).collect();
        let page_lines: Vec<&Line> = lines.iter().filter(|l| l.page == page).collect();

        let tables = detect_page_tables(
            &page_items,
            &page_rects,
            &page_lines,
            page,
            items,
            struct_tables,
        );
        if !tables.is_empty() {
            let entries: Vec<(f32, String)> = tables
                .into_iter()
                .map(|t| (t.y_top, format::table_to_markdown(&t)))
                .collect();
            result.insert(page, entries);
        }
    }

    result
}

/// Detect tables on a single page using the priority cascade.
fn detect_page_tables(
    items: &[&TextItem],
    rects: &[&Rect],
    lines: &[&Line],
    page: PageNum,
    all_items: &[TextItem],
    struct_tables: &[StructTable],
) -> Vec<Table> {
    // Strategy 1: rect-based detection
    let tables = detect_rects::detect_from_rects(items, rects, page);
    if !tables.is_empty() {
        return tables;
    }

    // Strategy 2: line-based detection
    let line_tables = detect_lines::detect_from_lines(items, lines, page);
    if !line_tables.is_empty() {
        return line_tables;
    }

    // Strategy 3: structure-tree-based detection
    let struct_result = detect_struct::detect_from_struct_tree(all_items, struct_tables, page);
    if !struct_result.is_empty() {
        return struct_result;
    }

    Vec::new()
}
