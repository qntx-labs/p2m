//! Rect-based table detection via union-find clustering.
//!
//! Groups overlapping/adjacent PDF rectangles into table regions, then builds
//! a grid from the rectangles' edges and assigns text items to cells.

use super::Table;
use super::grid::{Grid, build_grid};
use crate::types::{PageNum, Rect, TextItem};

/// Minimum number of rectangles to consider a cluster as a potential table.
const MIN_RECTS_FOR_TABLE: usize = 4;

/// Maximum gap (in points) between rectangles to consider them part of the same table.
const ADJACENCY_TOLERANCE: f32 = 2.0;

/// Detect tables from clustered rectangles on a single page.
pub(super) fn detect_from_rects(items: &[&TextItem], rects: &[&Rect], page: PageNum) -> Vec<Table> {
    if rects.len() < MIN_RECTS_FOR_TABLE {
        return Vec::new();
    }

    let clusters = cluster_rects(rects);
    let mut tables = Vec::new();

    for cluster in clusters {
        if cluster.len() < MIN_RECTS_FOR_TABLE {
            continue;
        }

        if let Some(grid) = build_grid_from_rects(&cluster) {
            let items_in_region: Vec<&TextItem> = items
                .iter()
                .filter(|item| grid.contains(item.x, item.y))
                .copied()
                .collect();

            if items_in_region.is_empty() {
                continue;
            }

            let cells = grid.assign_items(&items_in_region);
            if is_valid_table(&cells) {
                tables.push(Table {
                    cells,
                    y_top: grid.y_top(),
                    page,
                });
            }
        }
    }

    tables
}

/// Cluster rectangles using union-find: merge rects that overlap or are adjacent.
fn cluster_rects<'a>(rects: &[&'a Rect]) -> Vec<Vec<&'a Rect>> {
    let n = rects.len();
    let mut parent: Vec<usize> = (0..n).collect();
    let mut rank: Vec<usize> = vec![0; n];

    for i in 0..n {
        for j in (i + 1)..n {
            if rects_adjacent(rects[i], rects[j]) {
                union(&mut parent, &mut rank, i, j);
            }
        }
    }

    let mut clusters: std::collections::HashMap<usize, Vec<&'a Rect>> =
        std::collections::HashMap::new();
    for (i, &rect) in rects.iter().enumerate() {
        let root = find(&mut parent, i);
        clusters.entry(root).or_default().push(rect);
    }

    clusters.into_values().collect()
}

/// Union-find: path-compressed find.
fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Union-find: union by rank.
fn union(parent: &mut [usize], rank: &mut [usize], a: usize, b: usize) {
    let ra = find(parent, a);
    let rb = find(parent, b);
    if ra == rb {
        return;
    }
    match rank[ra].cmp(&rank[rb]) {
        std::cmp::Ordering::Less => parent[ra] = rb,
        std::cmp::Ordering::Greater => parent[rb] = ra,
        std::cmp::Ordering::Equal => {
            parent[rb] = ra;
            rank[ra] += 1;
        }
    }
}

/// Check if two rectangles are overlapping or within tolerance distance.
fn rects_adjacent(a: &Rect, b: &Rect) -> bool {
    let a_left = a.x.min(a.x + a.width);
    let a_right = a.x.max(a.x + a.width);
    let a_bottom = a.y.min(a.y + a.height);
    let a_top = a.y.max(a.y + a.height);

    let b_left = b.x.min(b.x + b.width);
    let b_right = b.x.max(b.x + b.width);
    let b_bottom = b.y.min(b.y + b.height);
    let b_top = b.y.max(b.y + b.height);

    let h_gap = (a_left - b_right).max(b_left - a_right).max(0.0);
    let v_gap = (a_bottom - b_top).max(b_bottom - a_top).max(0.0);

    h_gap <= ADJACENCY_TOLERANCE && v_gap <= ADJACENCY_TOLERANCE
}

/// Build a grid from rectangle edges.
fn build_grid_from_rects(rects: &[&Rect]) -> Option<Grid> {
    let mut x_edges: Vec<f32> = Vec::new();
    let mut y_edges: Vec<f32> = Vec::new();

    for r in rects {
        let left = r.x.min(r.x + r.width);
        let right = r.x.max(r.x + r.width);
        let bottom = r.y.min(r.y + r.height);
        let top = r.y.max(r.y + r.height);

        x_edges.push(left);
        x_edges.push(right);
        y_edges.push(bottom);
        y_edges.push(top);
    }

    build_grid(&mut x_edges, &mut y_edges)
}

/// Validate that a cell grid represents a real table, not just decorative rects.
fn is_valid_table(cells: &[Vec<String>]) -> bool {
    let rows = cells.len();
    if rows < 2 {
        return false;
    }
    let cols = cells.first().map_or(0, Vec::len);
    if cols < 2 {
        return false;
    }
    // At least 30% of cells should have content
    let total = rows * cols;
    let filled = cells
        .iter()
        .flatten()
        .filter(|c| !c.trim().is_empty())
        .count();
    filled * 100 / total >= 30
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PageNum;

    fn make_rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect {
            x,
            y,
            width: w,
            height: h,
            page: PageNum::new(1),
        }
    }

    #[test]
    fn adjacent_rects_cluster_together() {
        let r1 = make_rect(0.0, 0.0, 100.0, 20.0);
        let r2 = make_rect(0.0, 20.0, 100.0, 20.0);
        let r3 = make_rect(0.0, 40.0, 100.0, 20.0);
        let r4 = make_rect(200.0, 200.0, 50.0, 50.0); // isolated
        let refs: Vec<&Rect> = vec![&r1, &r2, &r3, &r4];
        let clusters = cluster_rects(&refs);
        assert_eq!(clusters.len(), 2);
    }

    #[test]
    fn non_overlapping_rects_separate() {
        let r1 = make_rect(0.0, 0.0, 10.0, 10.0);
        let r2 = make_rect(100.0, 100.0, 10.0, 10.0);
        let refs: Vec<&Rect> = vec![&r1, &r2];
        let clusters = cluster_rects(&refs);
        assert_eq!(clusters.len(), 2);
    }
}
