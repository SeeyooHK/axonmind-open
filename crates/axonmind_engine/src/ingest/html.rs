use super::NormalizedDocument;
use axonmind_core::AxonMindError;
use scraper::{Html, Selector};
use sha2::{Digest, Sha256};

pub fn parse(path: &std::path::Path, bytes: &[u8]) -> Result<NormalizedDocument, AxonMindError> {
    let text = std::str::from_utf8(bytes).map_err(|e| AxonMindError::Ingest {
        message: format!("invalid UTF-8: {e}"),
    })?;
    let sha256: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let md = html_to_markdown(text);
    super::markdown::parse_text(path, &md, sha256)
}

/// Convert HTML to a GFM-compatible Markdown string.
/// Handles: h1–h6, p, li, pre/code (with language detection), GFM tables.
/// li and p elements that are descendants of a table are skipped — their
/// content is already captured in the table rows.
fn html_to_markdown(html: &str) -> String {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("h1, h2, h3, h4, h5, h6, p, li, pre, table")
        .expect("static selector is valid");
    let code_sel = Selector::parse("code").expect("static");
    let tr_sel = Selector::parse("tr").expect("static");
    let cell_sel = Selector::parse("th, td").expect("static");

    let mut md = String::new();

    for el in doc.select(&sel) {
        let tag = el.value().name();

        // p and li inside a table would duplicate cell content — skip them.
        if matches!(tag, "p" | "li") {
            let in_table = el
                .ancestors()
                .any(|a| matches!(a.value(), scraper::Node::Element(e) if e.name() == "table"));
            if in_table {
                continue;
            }
        }

        match tag {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = tag[1..].parse::<usize>().unwrap_or(1);
                let text = el.text().collect::<String>();
                let text = text.trim();
                if !text.is_empty() {
                    md.push_str(&"#".repeat(level));
                    md.push(' ');
                    md.push_str(text);
                    md.push_str("\n\n");
                }
            }
            "p" => {
                let text = el.text().collect::<String>();
                let text = text.trim();
                if !text.is_empty() {
                    md.push_str(text);
                    md.push_str("\n\n");
                }
            }
            "li" => {
                let text = el.text().collect::<String>();
                let text = text.trim();
                if !text.is_empty() {
                    md.push_str("- ");
                    md.push_str(text);
                    md.push('\n');
                }
            }
            "pre" => {
                let code_text = el.text().collect::<String>();
                if !code_text.trim().is_empty() {
                    let lang = el
                        .select(&code_sel)
                        .next()
                        .and_then(|c| c.value().attr("class"))
                        .and_then(|cls| cls.split_whitespace().find(|s| s.starts_with("language-")))
                        .map(|s| s.trim_start_matches("language-"))
                        .unwrap_or("");
                    md.push_str("```");
                    md.push_str(lang);
                    md.push('\n');
                    md.push_str(&code_text);
                    if !code_text.ends_with('\n') {
                        md.push('\n');
                    }
                    md.push_str("```\n\n");
                }
            }
            "table" => {
                let rows: Vec<Vec<String>> = el
                    .select(&tr_sel)
                    .map(|row| {
                        row.select(&cell_sel)
                            .map(|c| c.text().collect::<String>().trim().replace('|', "\\|"))
                            .collect::<Vec<_>>()
                    })
                    .filter(|r| !r.is_empty())
                    .collect();

                if !rows.is_empty() {
                    md.push_str("| ");
                    md.push_str(&rows[0].join(" | "));
                    md.push_str(" |\n| ");
                    md.push_str(
                        &rows[0]
                            .iter()
                            .map(|_| "---")
                            .collect::<Vec<_>>()
                            .join(" | "),
                    );
                    md.push_str(" |\n");
                    for row in &rows[1..] {
                        md.push_str("| ");
                        md.push_str(&row.join(" | "));
                        md.push_str(" |\n");
                    }
                    md.push('\n');
                }
            }
            _ => {}
        }
    }

    md
}
