//! Line classification: header, list, code, caption.

/// Check if text is a figure/table caption or source citation.
pub(crate) fn is_caption_line(text: &str) -> bool {
    let trimmed = text.trim();

    let always_prefixes = [
        "Figura ",
        "Fig. ",
        "Fig ",
        "Tabela ",
        "Source:",
        "Fonte:",
        "Source ",
        "Fonte ",
        "Note:",
        "Nota:",
        "Chart ",
        "Gráfico ",
        "Graph ",
        "Diagram ",
        "Image ",
        "Imagem ",
        "Photo ",
        "Foto ",
    ];
    for prefix in &always_prefixes {
        if trimmed.starts_with(prefix) {
            return true;
        }
    }

    // "Figure" and "Table" need a digit/reference after them to distinguish
    // captions ("Table 1") from headings ("Table of Contents").
    for prefix in ["Figure ", "Table "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            if rest
                .trim_start()
                .starts_with(|c: char| c.is_ascii_digit() || c == '(' || c == '#')
            {
                return true;
            }
        }
    }

    let lower = trimmed.to_lowercase();
    for pfx in ["figure ", "table "] {
        if let Some(rest) = lower.strip_prefix(pfx) {
            if rest
                .trim_start()
                .starts_with(|c: char| c.is_ascii_digit() || c == '(' || c == '#')
            {
                return true;
            }
        }
    }
    if lower.starts_with("source:") {
        return true;
    }

    false
}

/// Check if text looks like a list item.
pub(crate) fn is_list_item(text: &str) -> bool {
    let trimmed = text.trim_start();

    if trimmed.starts_with("• ")
        || trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("○ ")
        || trimmed.starts_with("● ")
        || trimmed.starts_with("◦ ")
    {
        return true;
    }

    // Numbered list patterns: "1.", "1)", "(1)", "a.", "a)"
    let first_chars: String = trimmed.chars().take(5).collect();
    if first_chars.contains(|c: char| c.is_ascii_digit()) {
        if let Some(idx) = first_chars.find(['.', ')']) {
            let prefix = &first_chars[..idx];
            if prefix.chars().all(|c| c.is_ascii_digit()) {
                return true;
            }
        }
    }

    // Letter list: "a.", "a)", "(a)"
    let mut chars = trimmed.chars();
    if let (Some(first), Some(second)) = (chars.next(), chars.next()) {
        if first.is_ascii_alphabetic() && (second == '.' || second == ')') {
            return true;
        }
        if first == '(' && chars.next() == Some(')') {
            return true;
        }
    }

    false
}

/// Format list item to markdown.
pub(crate) fn format_list_item(text: &str) -> String {
    let trimmed = text.trim_start();

    for bullet in &['•', '○', '●', '◦'] {
        if let Some(rest) = trimmed.strip_prefix(*bullet) {
            return format!("- {}", rest.trim_start());
        }
    }

    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        return trimmed.to_string();
    }

    trimmed.to_string()
}

/// Check if font name indicates monospace.
pub(crate) fn is_monospace_font(font_name: &str) -> bool {
    let lower = font_name.to_lowercase();
    let patterns = [
        "courier",
        "consolas",
        "monaco",
        "menlo",
        "mono",
        "fixed",
        "terminal",
        "typewriter",
        "source code",
        "fira code",
        "jetbrains",
        "inconsolata",
        "dejavu sans mono",
        "liberation mono",
    ];

    patterns.iter().any(|p| lower.contains(p))
}
