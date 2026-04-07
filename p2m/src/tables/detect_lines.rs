//! Line-based table detection via horizontal/vertical grid intersection.
//!
//! Identifies tables by finding clusters of horizontal and vertical line segments
//! that form a grid pattern (T-junctions and cross intersections).

use super::Table;
use super::grid::{Grid, build_grid};
use crate::types::{Line, PageNum, TextItem};

/// Minimum number of horizontal lines to form a table grid.
const MIN_H_LINES: usize = 3;

/// Minimum number of vertical lines to form a table grid.
const MIN_V_LINES: usize = 2;

/// Tolerance for classifying a line as horizontal or vertical (max slope in pts).
const AXIS_TOLERANCE: f32 = 1.5;

/// Tolerance for snapping line endpoints to the same coordinate.
const SNAP_TOLERANCE: f32 = 3.0;

/// Detect tables from line-segment grids on a single page.
pub(super) fn detect_from_lines(items: &[&TextItem], lines: &[&Line], page: PageNum) -> Vec<Table> {
    let (h_lines, v_lines) = classify_lines(lines);

    if h_lines.len() < MIN_H_LINES || v_lines.len() < MIN_V_LINES {
        return Vec::new();
    }

    let grids = find_grids(&h_lines, &v_lines);
    let mut tables = Vec::new();

    for grid in grids {
        let items_in_region: Vec<&TextItem> = items
            .iter()
            .filter(|item| grid.contains(item.x, item.y))
            .copied()
            .collect();

        if items_in_region.is_empty() {
            continue;
        }

        let cells = grid.assign_items(&items_in_region);
        if is_valid_grid_table(&cells) {
            tables.push(Table {
                cells,
                y_top: grid.y_top(),
                page,
            });
        }
    }

    tables
}

/// A normalized horizontal line segment.
#[derive(Debug, Clone, Copy)]
struct HLine {
    /// Y coordinate of the line.
    y: f32,
    /// Left endpoint X.
    x_min: f32,
    /// Right endpoint X.
    x_max: f32,
}

/// A normalized vertical line segment.
#[derive(Debug, Clone, Copy)]
struct VLine {
    /// X coordinate of the line.
    x: f32,
    /// Bottom endpoint Y.
    y_min: f32,
    /// Top endpoint Y.
    y_max: f32,
}

/// Classify raw line segments into horizontal and vertical buckets.
fn classify_lines(lines: &[&Line]) -> (Vec<HLine>, Vec<VLine>) {
    let mut h_lines = Vec::new();
    let mut v_lines = Vec::new();

    for line in lines {
        let dx = (line.x2 - line.x1).abs();
        let dy = (line.y2 - line.y1).abs();

        if dy <= AXIS_TOLERANCE && dx > AXIS_TOLERANCE {
            h_lines.push(HLine {
                y: f32::midpoint(line.y1, line.y2),
                x_min: line.x1.min(line.x2),
                x_max: line.x1.max(line.x2),
            });
        } else if dx <= AXIS_TOLERANCE && dy > AXIS_TOLERANCE {
            v_lines.push(VLine {
                x: f32::midpoint(line.x1, line.x2),
                y_min: line.y1.min(line.y2),
                y_max: line.y1.max(line.y2),
            });
        }
    }

    h_lines.sort_by(|a, b| a.y.total_cmp(&b.y));
    v_lines.sort_by(|a, b| a.x.total_cmp(&b.x));

    (h_lines, v_lines)
}

/// Find grid regions where horizontal and vertical lines intersect densely.
fn find_grids(h_lines: &[HLine], v_lines: &[VLine]) -> Vec<Grid> {
    // Snap Y coordinates of horizontal lines into rows
    let y_rows = snap_coordinates(&h_lines.iter().map(|h| h.y).collect::<Vec<_>>());
    // Snap X coordinates of vertical lines into columns
    let x_cols = snap_coordinates(&v_lines.iter().map(|v| v.x).collect::<Vec<_>>());

    if y_rows.len() < MIN_H_LINES || x_cols.len() < MIN_V_LINES {
        return Vec::new();
    }

    // Verify grid quality: count intersections
    let mut intersections = 0;
    for &y in &y_rows {
        for &x in &x_cols {
            let h_hits = h_lines.iter().any(|h| {
                (h.y - y).abs() < SNAP_TOLERANCE
                    && h.x_min <= x + SNAP_TOLERANCE
                    && h.x_max >= x - SNAP_TOLERANCE
            });
            let v_hits = v_lines.iter().any(|v| {
                (v.x - x).abs() < SNAP_TOLERANCE
                    && v.y_min <= y + SNAP_TOLERANCE
                    && v.y_max >= y - SNAP_TOLERANCE
            });
            if h_hits && v_hits {
                intersections += 1;
            }
        }
    }

    let expected = y_rows.len() * x_cols.len();
    // Require at least 40% of expected intersections
    if expected == 0 || intersections * 100 / expected < 40 {
        return Vec::new();
    }

    let mut x_edges = x_cols;
    let mut y_edges = y_rows;

    build_grid(&mut x_edges, &mut y_edges).map_or_else(Vec::new, |grid| vec![grid])
}

/// Snap nearby coordinates to the same value, returning unique sorted values.
fn snap_coordinates(coords: &[f32]) -> Vec<f32> {
    if coords.is_empty() {
        return Vec::new();
    }

    let mut sorted: Vec<f32> = coords.to_vec();
    sorted.sort_by(f32::total_cmp);

    let mut snapped: Vec<f32> = vec![sorted[0]];
    for &c in &sorted[1..] {
        if let Some(last) = snapped.last_mut() {
            if (c - *last).abs() < SNAP_TOLERANCE {
                *last = f32::midpoint(*last, c);
            } else {
                snapped.push(c);
            }
        }
    }

    snapped
}

/// Validate that cells form a reasonable table from line-based detection.
fn is_valid_grid_table(cells: &[Vec<String>]) -> bool {
    let rows = cells.len();
    if rows < 2 {
        return false;
    }
    let cols = cells.first().map_or(0, Vec::len);
    if cols < 2 {
        return false;
    }
    // At least 20% of cells should have content (line-based grids can be sparser)
    let total = rows * cols;
    let filled = cells
        .iter()
        .flatten()
        .filter(|c| !c.trim().is_empty())
        .count();
    filled * 100 / total >= 20
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_groups_nearby_values() {
        let coords = vec![10.0, 10.5, 11.0, 30.0, 30.2, 50.0];
        let snapped = snap_coordinates(&coords);
        assert_eq!(snapped.len(), 3);
    }

    #[test]
    fn classify_separates_h_and_v() {
        let h = Line {
            x1: 0.0,
            y1: 100.0,
            x2: 200.0,
            y2: 100.0,
            page: PageNum::new(1),
        };
        let v = Line {
            x1: 50.0,
            y1: 0.0,
            x2: 50.0,
            y2: 200.0,
            page: PageNum::new(1),
        };
        let diag = Line {
            x1: 0.0,
            y1: 0.0,
            x2: 100.0,
            y2: 100.0,
            page: PageNum::new(1),
        };
        let refs: Vec<&Line> = vec![&h, &v, &diag];
        let (hs, vs) = classify_lines(&refs);
        assert_eq!(hs.len(), 1);
        assert_eq!(vs.len(), 1);
    }
}
