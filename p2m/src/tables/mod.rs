//! Table detection and Markdown rendering.
//!
//! Orchestrates multiple detection strategies in priority order:
//! 1. **Rect-based**: PDF `re` rectangles clustered into table regions (union-find).
//! 2. **Line-based**: Horizontal/vertical line segments forming grid intersections.
//! 3. **Structure-tree**: Tagged PDF `/Table` elements (if available).
//!
//! The first strategy that produces a valid table wins for each page region.

mod detect_heuristic;
mod detect_lines;
mod detect_rects;
mod detect_struct;
mod financial;
mod format;
mod grid;

use std::collections::HashMap;

use crate::extract::side_by_side::split_side_by_side;
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
    base_font_size: f32,
) -> HashMap<PageNum, Vec<(f32, String)>> {
    let mut pages: Vec<PageNum> = items.iter().map(|i| i.page).collect();
    pages.sort();
    pages.dedup();

    let mut result: HashMap<PageNum, Vec<(f32, String)>> = HashMap::new();

    for &page in &pages {
        let page_items: Vec<&TextItem> = items.iter().filter(|i| i.page == page).collect();
        let page_rects: Vec<&Rect> = rects.iter().filter(|r| r.page == page).collect();
        let page_lines: Vec<&Line> = lines.iter().filter(|l| l.page == page).collect();

        let mut tables = detect_page_tables(
            &page_items,
            &page_rects,
            &page_lines,
            page,
            items,
            struct_tables,
            base_font_size,
        );

        // Fallback: try side-by-side band splitting when full-page detection
        // finds nothing — two independent tables placed side by side.
        if tables.is_empty() {
            let owned: Vec<TextItem> = page_items.iter().map(|i| (*i).clone()).collect();
            let bands = split_side_by_side(&owned);
            if bands.len() == 2 {
                let margin = 2.0;
                for &(x_lo, x_hi) in &bands {
                    let band_items: Vec<&TextItem> = page_items
                        .iter()
                        .filter(|i| i.x >= x_lo - margin && i.x < x_hi + margin)
                        .copied()
                        .collect();
                    let band_rects: Vec<&Rect> = page_rects
                        .iter()
                        .filter(|r| r.x >= x_lo - margin && r.x < x_hi + margin)
                        .copied()
                        .collect();
                    let band_lines: Vec<&Line> = page_lines
                        .iter()
                        .filter(|l| l.x1.min(l.x2) < x_hi + margin && l.x1.max(l.x2) > x_lo - margin)
                        .copied()
                        .collect();
                    let band_all: Vec<TextItem> = items
                        .iter()
                        .filter(|i| i.page == page && i.x >= x_lo - margin && i.x < x_hi + margin)
                        .cloned()
                        .collect();
                    let band_tables = detect_page_tables(
                        &band_items,
                        &band_rects,
                        &band_lines,
                        page,
                        &band_all,
                        struct_tables,
                        base_font_size,
                    );
                    tables.extend(band_tables);
                }
            }
        }

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
    base_font_size: f32,
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

    // Strategy 4: heuristic detection (gap-histogram + body-font)
    let heuristic = detect_heuristic::detect_heuristic_tables(all_items, base_font_size, page);
    if !heuristic.is_empty() {
        return heuristic;
    }

    Vec::new()
}
