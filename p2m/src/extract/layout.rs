//! Column detection, line grouping, and reading order.
//!
//! Uses horizontal projection histograms to detect multi-column layouts
//! and groups [`TextItem`]s into [`TextLine`]s in reading order.
