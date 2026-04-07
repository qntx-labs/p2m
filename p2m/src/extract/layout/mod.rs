//! Column detection, line grouping, and reading-order layout.

mod columns;
mod lines;
mod spanning;

pub use lines::group_into_lines;

#[cfg(test)]
mod tests {
    use super::columns::{ColumnRegion, detect_columns};
    use super::lines::is_newspaper_layout;
    use super::spanning::identify_spanning_lines;
    use crate::types::{ItemKind, PageNum, TextItem, TextLine};

    /// Helper: create a TextItem at given position with given width text.
    fn make_item(page: u32, x: f32, y: f32, text: &str) -> TextItem {
        TextItem {
            text: text.to_string(),
            x,
            y,
            width: text.len() as f32 * 6.0, // ~6pt per char
            height: 12.0,
            font_size: 12.0,
            font: String::new(),
            page: PageNum::new(page),
            bold: false,
            italic: false,
            kind: ItemKind::Text,
            mcid: None,
        }
    }

    /// Generate dense items in a horizontal zone across many Y positions.
    /// Items are placed with overlapping coverage so no intra-zone valleys appear.
    fn fill_zone(page: u32, x_start: f32, x_end: f32, y_start: f32, y_end: f32) -> Vec<TextItem> {
        let mut items = Vec::new();
        let item_width = 60.0; // "SomeText__" = 10 chars * 6pt
        let step = 55.0; // overlap slightly to avoid intra-zone histogram gaps
        let mut y = y_start;
        while y >= y_end {
            let mut x = x_start;
            while x + item_width <= x_end {
                items.push(make_item(page, x, y, "SomeText__"));
                x += step;
            }
            y -= 14.0;
        }
        items
    }

    #[test]
    fn three_zone_layout_detected() {
        // Left months (x=15..330), right months (x=345..660), sidebar (x=675..800)
        // Each zone is >100pt wide so min_col_width won't reject any.
        let mut items = Vec::new();
        items.extend(fill_zone(1, 15.0, 330.0, 750.0, 50.0));
        items.extend(fill_zone(1, 345.0, 660.0, 750.0, 50.0));
        items.extend(fill_zone(1, 675.0, 800.0, 750.0, 50.0));

        let cols = detect_columns(&items, 1, false);
        assert_eq!(cols.len(), 3, "Expected 3 columns, got {}", cols.len());

        // Gutter 1 should be in the gap between left and middle zones
        let g1 = cols[0].x_max;
        assert!(
            (290.0..=350.0).contains(&g1),
            "First gutter at {g1}, expected between left and middle zones"
        );

        // Gutter 2 should be in the gap between middle and right zones
        let g2 = cols[1].x_max;
        assert!(
            (620.0..=680.0).contains(&g2),
            "Second gutter at {g2}, expected between middle and right zones"
        );
    }

    #[test]
    fn two_column_regression_guard() {
        // Standard 2-column layout with clear gutter at center
        let mut items = Vec::new();
        items.extend(fill_zone(1, 30.0, 280.0, 750.0, 50.0));
        items.extend(fill_zone(1, 320.0, 570.0, 750.0, 50.0));

        let cols = detect_columns(&items, 1, false);
        assert_eq!(cols.len(), 2, "Expected 2 columns, got {}", cols.len());

        let gutter = cols[0].x_max;
        assert!(
            (280.0..=320.0).contains(&gutter),
            "Gutter at {gutter}, expected ~300"
        );
    }

    #[test]
    fn score_prefers_balanced_gutter_over_wide_gap() {
        // 5 valid valleys: 2 are wide but split sparse content, 2 are narrower
        // but separate dense zones. The dense-zone gutters should win.
        let mut items = Vec::new();
        // Dense left zone
        items.extend(fill_zone(1, 15.0, 200.0, 750.0, 50.0));
        // Dense middle zone
        items.extend(fill_zone(1, 220.0, 400.0, 750.0, 50.0));
        // Dense right zone
        items.extend(fill_zone(1, 420.0, 600.0, 750.0, 50.0));
        // Sparse far-right zone (few items)
        for y_off in 0..12 {
            items.push(make_item(
                1,
                700.0,
                750.0 - y_off as f32 * 50.0,
                "Sparse____",
            ));
        }

        let cols = detect_columns(&items, 1, false);
        // Should detect the gutters between the 3 dense zones, not the wide gap
        // before the sparse zone
        assert!(
            cols.len() >= 3,
            "Expected >=3 columns for dense zones, got {}",
            cols.len()
        );
    }

    /// Helper: create items that fill a zone but with widths that extend past
    /// the zone boundary (simulating justified text). Items start within the zone
    /// but their reported width extends `overshoot` points past the zone end.
    fn fill_zone_justified(
        page: u32,
        x_start: f32,
        x_end: f32,
        overshoot: f32,
        y_start: f32,
        y_end: f32,
    ) -> Vec<TextItem> {
        let mut items = Vec::new();
        let mut y = y_start;
        while y >= y_end {
            // Each line: 3-4 items that together span x_start to x_end+overshoot
            let item_width = (x_end - x_start + overshoot) / 3.0;
            for i in 0..3 {
                let x = x_start + i as f32 * (x_end - x_start) / 3.0;
                let text_len = (item_width / 6.0).ceil() as usize;
                let text: String = "W".repeat(text_len);
                items.push(TextItem {
                    text,
                    x,
                    y,
                    width: item_width,
                    height: 12.0,
                    font_size: 12.0,
                    font: String::new(),
                    page: PageNum::new(page),
                    bold: false,
                    italic: false,
                    kind: ItemKind::Text,
                    mcid: None,
                });
            }
            y -= 14.0;
        }
        items
    }

    #[test]
    fn relative_valley_detects_justified_text_columns() {
        // Two columns of justified text where item widths overshoot the gutter
        // by a few points, preventing absolute valley detection from finding
        // an empty gutter.
        let mut items = Vec::new();
        // Left column: x=40..290, items extend to ~297 (7pt overshoot)
        items.extend(fill_zone_justified(1, 40.0, 290.0, 7.0, 750.0, 50.0));
        // Right column: x=300..550, items extend to ~557
        items.extend(fill_zone_justified(1, 300.0, 550.0, 7.0, 750.0, 50.0));

        let cols = detect_columns(&items, 1, false);
        assert_eq!(
            cols.len(),
            2,
            "Expected 2 columns for justified text, got {}",
            cols.len()
        );

        let gutter = cols[0].x_max;
        assert!(
            (280.0..=310.0).contains(&gutter),
            "Gutter at {gutter}, expected ~295"
        );
    }

    #[test]
    fn relative_valley_rejects_single_column_margin() {
        // Single column of text — the right margin drop-off should NOT be
        // detected as a column gutter.
        let items = fill_zone_justified(1, 40.0, 350.0, 0.0, 750.0, 50.0);

        let cols = detect_columns(&items, 1, false);
        assert_eq!(
            cols.len(),
            1,
            "Expected 1 column for single-column text, got {}",
            cols.len()
        );
    }

    /// Helper: build a Vec<TextLine> with `n` lines at given X, starting at Y=700.
    fn make_lines(n: usize, x: f32) -> Vec<TextLine> {
        (0..n)
            .map(|i| {
                let y = 700.0 - i as f32 * 14.0;
                let item = make_item(1, x, y, "SomeText__");
                TextLine {
                    y,
                    page: PageNum::new(1),
                    adaptive_threshold: 0.10,
                    items: vec![item],
                }
            })
            .collect()
    }

    #[test]
    fn sidebar_layout_detected_as_newspaper() {
        // Wide body column (x 0..400) with 40 lines,
        // narrow sidebar (x 420..590, width 170) with 12 lines.
        // width_ratio = 170/400 = 0.425, line_balance = 12/40 = 0.30 → sidebar → newspaper
        // Sidebar lines have ~3x gap of body lines (sparse annotations).
        let body = make_lines(40, 50.0);
        let sidebar: Vec<TextLine> = (0..12)
            .map(|i| {
                let y = 693.0 - i as f32 * 45.0; // sparse annotations: ~3x body gap
                let item = make_item(1, 440.0, y, "SomeText__");
                TextLine {
                    y,
                    page: PageNum::new(1),
                    adaptive_threshold: 0.10,
                    items: vec![item],
                }
            })
            .collect();
        let cols = vec![
            ColumnRegion {
                x_min: 0.0,
                x_max: 400.0,
            },
            ColumnRegion {
                x_min: 420.0,
                x_max: 590.0,
            },
        ];
        assert!(
            is_newspaper_layout(&[body, sidebar], &cols),
            "Wide body + narrow sidebar should be detected as newspaper"
        );
    }

    #[test]
    fn borderless_table_not_misclassified() {
        // Two columns of similar width and equal line counts → borderless table, not newspaper.
        // width_ratio = 250/300 = 0.83 (> 0.50), so sidebar guard fails → false.
        let col1 = make_lines(10, 50.0);
        let col2 = make_lines(10, 350.0);
        let cols = vec![
            ColumnRegion {
                x_min: 0.0,
                x_max: 300.0,
            },
            ColumnRegion {
                x_min: 300.0,
                x_max: 550.0,
            },
        ];
        assert!(
            !is_newspaper_layout(&[col1, col2], &cols),
            "Equal-width equal-row columns should NOT be newspaper (borderless table)"
        );
    }

    #[test]
    fn premask_spanning_title_removed_from_columns() {
        // Title spans x=30..550 as 5 adjacent items (no gap near gutter at x=300)
        // Two columns: left (x=0..300), right (x=300..600)
        let cols = vec![
            ColumnRegion {
                x_min: 0.0,
                x_max: 300.0,
            },
            ColumnRegion {
                x_min: 300.0,
                x_max: 600.0,
            },
        ];
        let mut items = Vec::new();

        // Spanning title: 5 items at Y=750, each ~100pt wide, gaps ~4pt
        // No item gap falls near the gutter at x=300
        for i in 0..5 {
            items.push(make_item(
                1,
                30.0 + i as f32 * 104.0,
                750.0,
                "TitleWord_________",
            ));
        }

        // Left column body: 20 lines
        for i in 0..20 {
            items.push(make_item(1, 30.0, 700.0 - i as f32 * 14.0, "LeftText__"));
        }

        // Right column body: 20 lines
        for i in 0..20 {
            items.push(make_item(1, 320.0, 700.0 - i as f32 * 14.0, "RightText_"));
        }

        let mask = identify_spanning_lines(&items, &cols);
        let spanning_count = mask.iter().filter(|&&m| m).count();
        let non_spanning_count = mask.iter().filter(|&&m| !m).count();
        assert_eq!(spanning_count, 5, "Title items should be pre-masked");
        assert_eq!(non_spanning_count, 40, "Column items should remain");
    }

    #[test]
    fn premask_does_not_mask_column_items_at_same_y() {
        // Two items at same Y with gap at gutter → NOT masked
        let cols = vec![
            ColumnRegion {
                x_min: 0.0,
                x_max: 300.0,
            },
            ColumnRegion {
                x_min: 300.0,
                x_max: 600.0,
            },
        ];
        let mut items = Vec::new();

        // Items in two columns at same Y — gap center ~305 is near gutter at 300
        for i in 0..15 {
            let y = 700.0 - i as f32 * 14.0;
            items.push(make_item(1, 30.0, y, "LeftText__"));
            items.push(make_item(1, 320.0, y, "RightText_"));
        }

        let mask = identify_spanning_lines(&items, &cols);
        let spanning_count = mask.iter().filter(|&&m| m).count();
        assert_eq!(
            spanning_count, 0,
            "Column items with gap at gutter should NOT be pre-masked"
        );
    }

    #[test]
    fn premask_narrow_line_not_masked() {
        // Items that form a line spanning only ~40% of column width → not masked
        let cols = vec![
            ColumnRegion {
                x_min: 0.0,
                x_max: 300.0,
            },
            ColumnRegion {
                x_min: 300.0,
                x_max: 600.0,
            },
        ];
        let mut items = Vec::new();

        // Narrow header at top (spans ~240pt, max col width = 300, threshold = 390)
        for i in 0..3 {
            items.push(make_item(
                1,
                180.0 + i as f32 * 84.0,
                750.0,
                "SmallHeader___",
            ));
        }

        // Two columns below
        for i in 0..15 {
            let y = 700.0 - i as f32 * 14.0;
            items.push(make_item(1, 30.0, y, "LeftText__"));
            items.push(make_item(1, 400.0, y, "RightText_"));
        }

        let mask = identify_spanning_lines(&items, &cols);
        let spanning_count = mask.iter().filter(|&&m| m).count();
        assert_eq!(spanning_count, 0, "Narrow header should NOT be pre-masked");
    }
}
