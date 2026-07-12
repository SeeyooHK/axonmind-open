//! Phase 1: Parse Markdown into `NormalizedDocument` using `comrak`.
use super::{DocumentBlock, NormalizedDocument, NormalizedTable, SourceSpan};
use axonmind_core::AxonMindError;
use comrak::{
    nodes::{AstNode, NodeValue},
    Arena, Options,
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
                push_list_items(node, &mut blocks);
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
        text.push_str(&node_text(child));
    }
    text
}

/// Text contributed by a single node: its own literal text if it's a leaf text-bearing
/// node, otherwise the concatenated text of its children.
fn node_text<'a>(node: &'a AstNode<'a>) -> String {
    {
        let data = node.data.borrow();
        match &data.value {
            NodeValue::Text(s) => return s.clone(),
            NodeValue::Code(nc) => return nc.literal.clone(),
            NodeValue::SoftBreak | NodeValue::LineBreak => return " ".to_string(),
            NodeValue::HtmlInline(_) => return String::new(),
            _ => {}
        }
    }
    collect_text(node)
}

/// Push one `DocumentBlock::ListItem` per item in `list_node` (a `NodeValue::List`).
/// A nested list inside an item (e.g. lettered sub-items under a numbered paragraph)
/// becomes its own sibling `ListItem` blocks, recursed after the parent item, instead
/// of being flattened into the parent item's text with no line separation.
fn push_list_items<'a>(list_node: &'a AstNode<'a>, blocks: &mut Vec<DocumentBlock>) {
    for item in list_node.children() {
        let ordinal = match &item.data.borrow().value {
            NodeValue::Item(item_list)
                if item_list.list_type == comrak::nodes::ListType::Ordered =>
            {
                Some(item_list.start)
            }
            _ => None,
        };

        let mut own_text = String::new();
        let mut nested_lists: Vec<&AstNode<'_>> = Vec::new();
        for child in item.children() {
            if matches!(child.data.borrow().value, NodeValue::List(_)) {
                nested_lists.push(child);
            } else {
                own_text.push_str(&node_text(child));
            }
        }

        if !own_text.trim().is_empty() {
            blocks.push(DocumentBlock::ListItem {
                text: own_text,
                ordinal,
                span: SourceSpan::default(),
            });
        }

        for nested in nested_lists {
            push_list_items(nested, blocks);
        }
    }
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

    #[test]
    fn ordered_list_items_capture_their_source_ordinal() {
        let doc = parse_text(
            std::path::Path::new("regulation.md"),
            "Article 33\n\nNotification of a breach\n\n\
             1. In the case of a breach, the controller shall notify.\n\
             2. The notification shall at least describe the nature.\n",
            "abc123abc123abc123".to_string(),
        )
        .expect("valid markdown should parse");

        let ordinals: Vec<Option<usize>> = doc
            .blocks
            .iter()
            .filter_map(|b| match b {
                crate::ingest::DocumentBlock::ListItem { ordinal, .. } => Some(*ordinal),
                _ => None,
            })
            .collect();
        assert_eq!(
            ordinals,
            vec![Some(1), Some(2)],
            "each ordered-list item must carry its own source ordinal, not be discarded \
             (docs/retrieve_guarantee.md: paragraph markers depend on rendering '1. '/'2. ', \
             not a bare '- ' bullet)"
        );
    }

    #[test]
    fn nested_sub_list_items_become_separate_blocks_not_flattened_text() {
        // Article 33(3)'s (a)-(d) shape: a numbered paragraph whose continuation is a nested
        // bullet list. Before the fix, `collect_text` recursed into the nested list and
        // concatenated every sub-item into the parent paragraph's own text with no separator.
        let doc = parse_text(
            std::path::Path::new("regulation.md"),
            "3. The notification shall at least:\n\
             \n   - describe the nature of the breach\n\
             \n   - communicate the name of the contact point\n",
            "abc123abc123abc123".to_string(),
        )
        .expect("valid markdown should parse");

        let list_items: Vec<&crate::ingest::DocumentBlock> = doc
            .blocks
            .iter()
            .filter(|b| matches!(b, crate::ingest::DocumentBlock::ListItem { .. }))
            .collect();
        assert_eq!(
            list_items.len(),
            3,
            "the parent item and each nested sub-item must be their own block, got {list_items:?}"
        );
        let parent_text = match list_items[0] {
            crate::ingest::DocumentBlock::ListItem { text, .. } => text.as_str(),
            _ => unreachable!(),
        };
        assert!(
            !parent_text.contains("describe the nature"),
            "nested sub-item text must not be flattened into the parent item's own text, got: \
             {parent_text:?}"
        );
    }
}
