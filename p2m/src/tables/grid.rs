//! Grid construction and text-item-to-cell assignment.
//!
//! Given sorted X and Y edge coordinates, builds a column/row grid and assigns
//! text items to cells based on their centroid position.

use crate::types::TextItem;

/// A table grid defined by column and row boundaries.
#[derive(Debug, Clone)]
pub(super) struct Grid {
    /// Sorted column boundaries (N+1 edges for N columns).
    x_edges: Vec<f32>,
    /// Sorted row boundaries (N+1 edges for N rows), ascending Y.
    y_edges: Vec<f32>,
}

impl Grid {
    /// Number of columns.
    pub const fn cols(&self) -> usize {
        self.x_edges.len().saturating_sub(1)
    }

    /// Number of rows.
    pub const fn rows(&self) -> usize {
        self.y_edges.len().saturating_sub(1)
    }

    /// Y coordinate of the table's top edge (highest Y in PDF coordinates).
    pub fn y_top(&self) -> f32 {
        self.y_edges.last().copied().unwrap_or(0.0)
    }

    /// Check if a point falls within the grid's bounding box (with small margin).
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let margin = 2.0;
        let x_min = self.x_edges.first().copied().unwrap_or(0.0) - margin;
        let x_max = self.x_edges.last().copied().unwrap_or(0.0) + margin;
        let y_min = self.y_edges.first().copied().unwrap_or(0.0) - margin;
        let y_max = self.y_edges.last().copied().unwrap_or(0.0) + margin;
        x >= x_min && x <= x_max && y >= y_min && y <= y_max
    }

    /// Assign text items to cells, returning a row-major grid of cell strings.
    ///
    /// Rows are ordered top-to-bottom (descending Y), matching reading order.
    pub fn assign_items(&self, items: &[&TextItem]) -> Vec<Vec<String>> {
        let rows = self.rows();
        let cols = self.cols();

        if rows == 0 || cols == 0 {
            return Vec::new();
        }

        // Accumulate items per cell: cells[row][col] = Vec<(x, text)>
        let mut cell_items: Vec<Vec<Vec<(f32, String)>>> = vec![vec![Vec::new(); cols]; rows];

        for item in items {
            let cx = item.x + item.width / 2.0;
            let cy = item.y;

            if let Some((row, col)) = self.locate(cx, cy) {
                cell_items[row][col].push((item.x, item.text.clone()));
            }
        }

        // Build cell text: sort items within each cell by X position,
        // then join with spaces. Rows are reversed so row 0 = top of page.
        cell_items
            .into_iter()
            .rev()
            .map(|row| {
                row.into_iter()
                    .map(|mut cell_parts| {
                        cell_parts.sort_by(|a, b| a.0.total_cmp(&b.0));
                        let text: String = cell_parts
                            .into_iter()
                            .map(|(_, t)| t)
                            .collect::<Vec<_>>()
                            .join(" ");
                        text.trim().to_string()
                    })
                    .collect()
            })
            .collect()
    }

    /// Find the (row, col) cell for a point. Row 0 is the bottom-most row
    /// (lowest Y). Returns `None` if the point is outside the grid.
    fn locate(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        let col = find_bin(&self.x_edges, x)?;
        let row = find_bin(&self.y_edges, y)?;
        Some((row, col))
    }
}

/// Build a grid from edge coordinates. Deduplicates and sorts edges.
/// Returns `None` if there aren't enough edges for at least a 2×2 grid.
pub(super) fn build_grid(x_edges: &mut Vec<f32>, y_edges: &mut Vec<f32>) -> Option<Grid> {
    dedup_edges(x_edges);
    dedup_edges(y_edges);

    if x_edges.len() < 3 || y_edges.len() < 3 {
        return None;
    }

    Some(Grid {
        x_edges: x_edges.clone(),
        y_edges: y_edges.clone(),
    })
}

/// Sort and deduplicate edges within a tolerance of 1.5 points.
fn dedup_edges(edges: &mut Vec<f32>) {
    edges.sort_by(f32::total_cmp);
    edges.dedup_by(|a, b| (*a - *b).abs() < 1.5);
}

/// Binary-search for the bin index containing `val` in sorted `edges`.
fn find_bin(edges: &[f32], val: f32) -> Option<usize> {
    if edges.len() < 2 {
        return None;
    }
    let margin = 2.0;
    if val < edges[0] - margin || val > edges[edges.len() - 1] + margin {
        return None;
    }
    // Find the rightmost edge <= val
    let pos = edges.partition_point(|&e| e <= val + margin);
    if pos == 0 {
        Some(0)
    } else if pos >= edges.len() {
        Some(edges.len() - 2)
    } else {
        Some((pos - 1).min(edges.len() - 2))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn build_grid_deduplicates() {
        let mut xs = vec![0.0, 0.5, 100.0, 100.2, 200.0];
        let mut ys = vec![0.0, 50.0, 50.5, 100.0];
        let grid = build_grid(&mut xs, &mut ys).expect("grid should be built from valid edges");
        assert_eq!(grid.cols(), 2);
        assert_eq!(grid.rows(), 2);
    }

    #[test]
    fn find_bin_returns_correct_index() {
        let edges = vec![0.0, 100.0, 200.0, 300.0];
        assert_eq!(find_bin(&edges, 50.0), Some(0));
        assert_eq!(find_bin(&edges, 150.0), Some(1));
        assert_eq!(find_bin(&edges, 250.0), Some(2));
    }

    #[test]
    fn too_few_edges_returns_none() {
        let mut xs = vec![0.0, 100.0];
        let mut ys = vec![0.0, 50.0, 100.0];
        assert!(build_grid(&mut xs, &mut ys).is_none());
    }
}
