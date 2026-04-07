//! Table → Markdown rendering.
//!
//! Produces compact Markdown tables optimized for token efficiency:
//! minimal padding, no trailing whitespace.

use super::Table;

/// Render a [`Table`] as a Markdown table string.
pub(super) fn table_to_markdown(table: &Table) -> String {
    let cells = &table.cells;
    if cells.is_empty() {
        return String::new();
    }

    let cols = cells.first().map_or(0, Vec::len);
    if cols == 0 {
        return String::new();
    }

    // Compute column widths (minimum 3 for the separator dashes).
    let mut col_widths: Vec<usize> = vec![3; cols];
    for row in cells {
        for (c, cell) in row.iter().enumerate() {
            if c < cols {
                col_widths[c] = col_widths[c].max(cell.len());
            }
        }
    }

    let mut out = String::new();

    // Header row (first row of cells).
    write_row(&mut out, &cells[0], &col_widths);

    // Separator row.
    out.push('|');
    for &w in &col_widths {
        out.push_str(&"-".repeat(w));
        out.push('|');
    }
    out.push('\n');

    // Data rows.
    for row in &cells[1..] {
        write_row(&mut out, row, &col_widths);
    }

    out
}

/// Write a single table row.
fn write_row(out: &mut String, row: &[String], col_widths: &[usize]) {
    out.push('|');
    for (c, cell) in row.iter().enumerate() {
        let w = col_widths.get(c).copied().unwrap_or(3);
        // Escape pipe characters in cell text.
        let escaped = cell.replace('|', "\\|");
        out.push_str(&escaped);
        // Pad to column width.
        let pad = w.saturating_sub(escaped.len());
        for _ in 0..pad {
            out.push(' ');
        }
        out.push('|');
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PageNum;

    #[test]
    fn simple_2x2_table() {
        let table = Table {
            cells: vec![vec!["A".into(), "B".into()], vec!["1".into(), "2".into()]],
            y_top: 700.0,
            page: PageNum::new(1),
        };
        let md = table_to_markdown(&table);
        assert!(md.contains("|A"));
        assert!(md.contains("|B"));
        assert!(md.contains("---"));
        assert!(md.contains("|1"));
        assert!(md.contains("|2"));
    }

    #[test]
    fn pipe_in_cell_is_escaped() {
        let table = Table {
            cells: vec![
                vec!["Header".into(), "Col2".into()],
                vec!["a|b".into(), "c".into()],
            ],
            y_top: 700.0,
            page: PageNum::new(1),
        };
        let md = table_to_markdown(&table);
        assert!(md.contains(r"a\|b"));
    }

    #[test]
    fn empty_table_returns_empty() {
        let table = Table {
            cells: vec![],
            y_top: 0.0,
            page: PageNum::new(1),
        };
        assert!(table_to_markdown(&table).is_empty());
    }
}
