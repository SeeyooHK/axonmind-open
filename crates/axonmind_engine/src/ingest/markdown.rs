//! Phase 1: Parse Markdown into `NormalizedDocument` using `comrak`.
use super::{DocumentBlock, NormalizedDocument, NormalizedTable, SourceSpan};
use axonmind_core::AxonMindError;
use comrak::{
    Arena, Options,
    nodes::{AstNode, NodeValue},
};
use sha2::{Digest, Sha256};

/// GFM tables with more rows than this are treated as pure data.
/// Their preceding heading is removed from `blocks` to avoid polluting
/// KPI text extraction with data-section labels.
const LARGE_TABLE_ROW_THRESHOLD: usize = 200;

pub fn parse(path: &std::path::Path, bytes: &[u8]) -> Result<NormalizedDocument, AxonMindError> {
    let text = std::str::from_utf8(bytes).map_err(|e| AxonMindError::Ingest {
        message: format!("invalid UTF-8: {e}"),
    })?;
    let sha256: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    parse_text(path, text, sha256)
}

/// Parse Markdown text with a pre-computed sha256 (used by docx/pdf adapters
/// so the sha256 reflects the original file, not the Markdown intermediary).
pub(crate) fn parse_text(
    path: &std::path::Path,
    text: &str,
    sha256: String,
) -> Result<NormalizedDocument, AxonMindError> {
    let arena = Arena::new();
    let mut opts = Options::default();
    opts.extension.table = true;
    opts.extension.strikethrough = true;
    let root = comrak::parse_document(&arena, text, &opts);

    let mut blocks: Vec<DocumentBlock> = Vec::new();
    let mut tables = Vec::new();
    let mut title: Option<String> = None;

    for node in root.children() {
        let value = {
            let data = node.data.borrow();
            data.value.clone()
        };
        match value {
            NodeValue::Heading(h) => {
                let text = collect_text(node);
                if h.level == 1 && title.is_none() {
                    title = Some(text.clone());
                }
                blocks.push(DocumentBlock::Heading {
                    level: h.level,
                    text,
                    span: SourceSpan::default(),
                });
            }
            NodeValue::Paragraph => {
                let text = collect_text(node);
                if !text.trim().is_empty() {
                    blocks.push(DocumentBlock::Paragraph {
                        text,
                        span: SourceSpan::default(),
                    });
                }
            }
            NodeValue::List(_) => {
                for item in node.children() {
                    let text = collect_text(item);
                    if !text.trim().is_empty() {
                        blocks.push(DocumentBlock::ListItem {
                            text,
                            span: SourceSpan::default(),
                        });
                    }
                }
            }
            NodeValue::CodeBlock(cb) => {
                let language = if cb.info.is_empty() {
                    None
                } else {
                    Some(cb.info.clone())
                };
                blocks.push(DocumentBlock::CodeBlock {
                    language,
                    text: cb.literal.clone(),
                    span: SourceSpan::default(),
                });
            }
            NodeValue::Table(_) => {
                let mut headers = Vec::new();
                let mut rows = Vec::new();
                for (i, row_node) in node.children().enumerate() {
                    let cells: Vec<String> =
                        row_node.children().map(|cell| collect_text(cell)).collect();
                    if i == 0 {
                        headers = cells;
                    } else {
                        rows.push(cells);
                    }
                }
                if !headers.is_empty() {
                    let is_large = rows.len() > LARGE_TABLE_ROW_THRESHOLD;
                    // Large data tables: strip the immediately preceding heading
                    // so it doesn't appear as a KPI candidate.
                    if is_large {
                        if matches!(blocks.last(), Some(DocumentBlock::Heading { .. })) {
                            blocks.pop();
                        }
                    }
                    tables.push(NormalizedTable {
                        headers,
                        rows,
                        span: SourceSpan::default(),
                    });
                }
            }
            _ => {}
        }
    }

    Ok(NormalizedDocument {
        id: format!("doc.{}", &sha256[..8]),
        source_path: Some(path.to_path_buf()),
        sha256,
        title,
        blocks,
        tables,
    })
}

fn collect_text<'a>(node: &'a AstNode<'a>) -> String {
    let mut text = String::new();
    for child in node.children() {
        {
            let data = child.data.borrow();
            match &data.value {
                NodeValue::Text(s) => {
                    text.push_str(s);
                    continue;
                }
                NodeValue::Code(nc) => {
                    text.push_str(&nc.literal);
                    continue;
                }
                NodeValue::SoftBreak | NodeValue::LineBreak => {
                    text.push(' ');
                    continue;
                }
                NodeValue::HtmlInline(_) => {
                    continue;
                }
                _ => {}
            }
        }
        text.push_str(&collect_text(child));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::parse_text;

    #[test]
    fn parses_markdown_blocks_from_ocr_text() {
        let doc = parse_text(
            std::path::Path::new("receipt.png"),
            "# Invoice\n\n- Customer: Acme\n- Total: $42\n\n| Metric | Value |\n| --- | --- |\n| Revenue | 42 |",
            "0123456789abcdef".to_string(),
        )
        .expect("valid OCR markdown should parse");

        assert!(
            doc.blocks.iter().any(|block| matches!(
                block,
                crate::ingest::DocumentBlock::Heading { text, .. } if text == "Invoice"
            )),
            "OCR markdown headings must become searchable document blocks"
        );
        assert!(
            doc.blocks.iter().any(|block| matches!(
                block,
                crate::ingest::DocumentBlock::ListItem { text, .. } if text.contains("Customer")
            )),
            "OCR markdown list items must not be dropped before extraction"
        );
        assert_eq!(doc.tables.len(), 1, "OCR markdown tables must be retained");
    }
}
