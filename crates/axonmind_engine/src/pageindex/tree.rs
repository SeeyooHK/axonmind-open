use crate::ingest::{DocumentBlock, NormalizedDocument};

/// In-memory build type for a document's section tree.
/// Flattened to `SectionRow` at persist time; at query time we use the flat rows + FTS.
#[derive(Debug, Clone, PartialEq)]
pub struct PageSection {
    pub id: String,
    pub title: String,
    pub level: u8,
    pub summary: Option<String>,
    pub text: Option<String>,
    pub span_start: usize,
    pub span_end: usize,
    pub children: Vec<PageSection>,
}

impl PageSection {
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }
}

/// Flattened row for storage in `page_sections`.
#[derive(Debug, Clone)]
pub struct SectionRow {
    pub section_id: String,
    pub doc_node_id: String,
    pub parent_section_id: Option<String>,
    pub ordinal: i64,
    pub level: i64,
    pub title: String,
    pub path: String,
    pub summary: Option<String>,
    pub text: Option<String>,
    pub span_start: i64,
    pub span_end: i64,
}

/// Document metadata + flattened sections for one atomic upsert.
pub struct PersistTree {
    pub doc_node_id: String,
    pub sha256: String,
    pub title: String,
    pub doc_summary: Option<String>,
    pub sections: Vec<SectionRow>,
}

fn span_start(block: &DocumentBlock) -> usize {
    match block {
        DocumentBlock::Heading { span, .. } => span.start,
        DocumentBlock::Paragraph { span, .. } => span.start,
        DocumentBlock::ListItem { span, .. } => span.start,
        DocumentBlock::CodeBlock { span, .. } => span.start,
    }
}

fn append_text(dest: &mut Option<String>, text: &str) {
    match dest {
        Some(existing) => {
            existing.push('\n');
            existing.push_str(text);
        }
        None => *dest = Some(text.to_string()),
    }
}

/// Build the in-memory section tree from a `NormalizedDocument`.
///
/// Uses a heading-stack algorithm over `doc.blocks`. Tables are integrated in
/// document order: each table is appended to the currently active section.
/// Headerless docs (no headings, but content present) produce a single synthetic
/// "Document" root. Empty docs return an empty vec.
pub fn build_tree(doc: &NormalizedDocument, doc_node_id: &str) -> Vec<PageSection> {
    let has_headings = doc
        .blocks
        .iter()
        .any(|b| matches!(b, DocumentBlock::Heading { .. }));

    // Sort tables by span start for ordered inline processing.
    let mut sorted_tables: Vec<(usize, String)> = doc
        .tables
        .iter()
        .map(|t| {
            let mut lines = vec![t.headers.join(" | ")];
            for row in &t.rows {
                lines.push(row.join(" | "));
            }
            (t.span.start, lines.join("\n"))
        })
        .collect();
    sorted_tables.sort_by_key(|(start, _)| *start);

    if !has_headings {
        return build_headerless(doc, sorted_tables, doc_node_id);
    }

    let mut stack: Vec<PageSection> = Vec::new();
    let mut roots: Vec<PageSection> = Vec::new();
    let mut counter: usize = 0;
    let mut table_idx = 0;

    // Helper: append table text to the top of stack, or last root if stack is empty.
    let flush_tables_before = |before_offset: usize,
                                table_idx: &mut usize,
                                stack: &mut Vec<PageSection>,
                                roots: &mut Vec<PageSection>| {
        while *table_idx < sorted_tables.len()
            && sorted_tables[*table_idx].0 < before_offset
        {
            let table_text = sorted_tables[*table_idx].1.clone();
            if let Some(top) = stack.last_mut() {
                append_text(&mut top.text, &table_text);
            } else if let Some(root) = roots.last_mut() {
                append_text(&mut root.text, &table_text);
            }
            *table_idx += 1;
        }
    };

    for block in &doc.blocks {
        let block_offset = span_start(block);
        flush_tables_before(block_offset, &mut table_idx, &mut stack, &mut roots);

        match block {
            DocumentBlock::Heading { level, text, span } => {
                let level = *level;
                // Pop sections with level >= current heading: they become children of whatever
                // is left on the stack, or roots.
                while stack
                    .last()
                    .map(|s| s.level >= level)
                    .unwrap_or(false)
                {
                    let popped = stack.pop().unwrap();
                    if let Some(parent) = stack.last_mut() {
                        parent.children.push(popped);
                    } else {
                        roots.push(popped);
                    }
                }
                counter += 1;
                stack.push(PageSection {
                    id: format!("{doc_node_id}#{counter:04}"),
                    title: text.clone(),
                    level,
                    summary: None,
                    text: None,
                    span_start: span.start,
                    span_end: span.end,
                    children: vec![],
                });
            }
            DocumentBlock::Paragraph { text, span }
            | DocumentBlock::ListItem { text, span } => {
                if let Some(top) = stack.last_mut() {
                    append_text(&mut top.text, text);
                    top.span_end = top.span_end.max(span.end);
                }
            }
            DocumentBlock::CodeBlock { text, span, .. } => {
                if let Some(top) = stack.last_mut() {
                    append_text(&mut top.text, text);
                    top.span_end = top.span_end.max(span.end);
                }
            }
        }
    }

    // Flush any remaining tables after all blocks.
    flush_tables_before(usize::MAX, &mut table_idx, &mut stack, &mut roots);

    // Drain the stack: each popped section attaches to its parent or roots.
    while let Some(section) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(section);
        } else {
            roots.push(section);
        }
    }

    roots
}

fn build_headerless(
    doc: &NormalizedDocument,
    sorted_tables: Vec<(usize, String)>,
    doc_node_id: &str,
) -> Vec<PageSection> {
    let mut text = String::new();
    let mut span_start_val = 0usize;
    let mut span_end_val = 0usize;

    for block in &doc.blocks {
        let block_text = match block {
            DocumentBlock::Paragraph { text, span }
            | DocumentBlock::ListItem { text, span } => {
                span_start_val = span_start_val.min(span.start);
                span_end_val = span_end_val.max(span.end);
                text.as_str()
            }
            DocumentBlock::CodeBlock { text, span, .. } => {
                span_start_val = span_start_val.min(span.start);
                span_end_val = span_end_val.max(span.end);
                text.as_str()
            }
            DocumentBlock::Heading { text, span, .. } => {
                span_start_val = span_start_val.min(span.start);
                span_end_val = span_end_val.max(span.end);
                text.as_str()
            }
        };
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(block_text);
    }

    for (_, table_text) in &sorted_tables {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(table_text);
    }

    if text.is_empty() {
        return vec![];
    }

    vec![PageSection {
        id: format!("{doc_node_id}#0001"),
        title: "Document".to_string(),
        level: 0,
        summary: None,
        text: Some(text),
        span_start: span_start_val,
        span_end: span_end_val,
        children: vec![],
    }]
}

/// Flatten the in-memory tree to rows for storage. Computes path breadcrumbs,
/// parent_section_id, and monotonic ordinal within each parent.
pub fn flatten_tree(
    roots: &[PageSection],
    doc_node_id: &str,
    sha256: &str,
    doc_title: &str,
    doc_summary: Option<String>,
) -> PersistTree {
    let mut sections = Vec::new();
    for (i, root) in roots.iter().enumerate() {
        flatten_section(root, None, i as i64, &[], &mut sections, doc_node_id);
    }
    PersistTree {
        doc_node_id: doc_node_id.to_string(),
        sha256: sha256.to_string(),
        title: doc_title.to_string(),
        doc_summary,
        sections,
    }
}

fn flatten_section(
    section: &PageSection,
    parent_id: Option<&str>,
    ordinal: i64,
    parent_path: &[String],
    out: &mut Vec<SectionRow>,
    doc_node_id: &str,
) {
    let mut path_parts = parent_path.to_vec();
    path_parts.push(section.title.clone());
    let path = path_parts.join(" \u{203a} ");

    out.push(SectionRow {
        section_id: section.id.clone(),
        doc_node_id: doc_node_id.to_string(),
        parent_section_id: parent_id.map(|s| s.to_string()),
        ordinal,
        level: section.level as i64,
        title: section.title.clone(),
        path: path.clone(),
        summary: section.summary.clone(),
        text: section.text.clone(),
        span_start: section.span_start as i64,
        span_end: section.span_end as i64,
    });

    for (i, child) in section.children.iter().enumerate() {
        flatten_section(child, Some(&section.id), i as i64, &path_parts, out, doc_node_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{DocumentBlock, NormalizedDocument, NormalizedTable, SourceSpan};

    fn heading(level: u8, text: &str, start: usize, end: usize) -> DocumentBlock {
        DocumentBlock::Heading {
            level,
            text: text.to_string(),
            span: SourceSpan { start, end },
        }
    }

    fn para(text: &str, start: usize, end: usize) -> DocumentBlock {
        DocumentBlock::Paragraph {
            text: text.to_string(),
            span: SourceSpan { start, end },
        }
    }

    fn doc(blocks: Vec<DocumentBlock>) -> NormalizedDocument {
        NormalizedDocument {
            id: "doc.testabcd".to_string(),
            source_path: None,
            sha256: "testsha256".to_string(),
            title: None,
            blocks,
            tables: vec![],
        }
    }

    #[test]
    fn test_builder_basic_structure() {
        // h1 A with children [h2 B, h2 C], each with body text
        let d = doc(vec![
            heading(1, "A", 0, 5),
            para("a body", 6, 12),
            heading(2, "B", 13, 18),
            para("b body", 19, 25),
            heading(2, "C", 26, 31),
            para("c body", 32, 38),
        ]);
        let roots = build_tree(&d, "doc.test1234");
        assert_eq!(roots.len(), 1, "should have one root");
        assert_eq!(roots[0].title, "A");
        assert_eq!(roots[0].level, 1);
        assert_eq!(roots[0].children.len(), 2, "A should have two children");
        assert_eq!(roots[0].children[0].title, "B");
        assert_eq!(roots[0].children[1].title, "C");
        assert!(roots[0].text.as_deref().unwrap_or("").contains("a body"));
        assert!(roots[0].children[0].text.as_deref().unwrap_or("").contains("b body"));
        assert!(roots[0].children[1].text.as_deref().unwrap_or("").contains("c body"));
    }

    #[test]
    fn test_level_skip() {
        // h1 A → h3 C (skip h2): h3 becomes child of h1
        let d = doc(vec![
            heading(1, "A", 0, 5),
            heading(3, "C", 6, 11),
        ]);
        let roots = build_tree(&d, "doc.test1234");
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].children.len(), 1);
        assert_eq!(roots[0].children[0].level, 3);
        assert_eq!(roots[0].children[0].title, "C");
    }

    #[test]
    fn test_empty_heading() {
        // Heading with no following body text (children only)
        let d = doc(vec![
            heading(1, "Parent", 0, 10),
            heading(2, "Child", 11, 20),
        ]);
        let roots = build_tree(&d, "doc.test1234");
        assert_eq!(roots.len(), 1);
        assert!(roots[0].text.is_none(), "empty heading has no body text");
        assert_eq!(roots[0].children.len(), 1);
        assert_eq!(roots[0].children[0].title, "Child");
    }

    #[test]
    fn test_headerless_fallback() {
        // No headings: single synthetic "Document" root
        let d = doc(vec![
            para("line one", 0, 8),
            para("line two", 9, 17),
        ]);
        let roots = build_tree(&d, "doc.test1234");
        assert_eq!(roots.len(), 1, "should produce one synthetic root");
        assert_eq!(roots[0].title, "Document");
        assert_eq!(roots[0].level, 0);
        let text = roots[0].text.as_deref().unwrap_or("");
        assert!(text.contains("line one"));
        assert!(text.contains("line two"));
    }

    #[test]
    fn test_empty_doc() {
        let d = NormalizedDocument {
            id: "doc.empty".to_string(),
            source_path: None,
            sha256: "sha".to_string(),
            title: None,
            blocks: vec![],
            tables: vec![],
        };
        let roots = build_tree(&d, "doc.empty");
        assert!(roots.is_empty(), "empty doc should produce no sections");
    }

    #[test]
    fn test_multiple_roots() {
        // h1 A, h1 B → two roots
        let d = doc(vec![
            heading(1, "A", 0, 5),
            para("a body", 6, 12),
            heading(1, "B", 13, 18),
            para("b body", 19, 25),
        ]);
        let roots = build_tree(&d, "doc.test1234");
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].title, "A");
        assert_eq!(roots[1].title, "B");
    }

    #[test]
    fn test_flatten_breadcrumb_and_parent() {
        // h1 Top → h2 Middle → h3 Leaf: verify path and parent_section_id
        let d = doc(vec![
            heading(1, "Top", 0, 5),
            heading(2, "Middle", 6, 15),
            heading(3, "Leaf", 16, 25),
            para("content", 26, 33),
        ]);
        let roots = build_tree(&d, "doc.test1234");
        let tree = flatten_tree(&roots, "doc.test1234", "testsha", "Top", None);

        let leaf = tree.sections.iter().find(|r| r.title == "Leaf").unwrap();
        assert_eq!(leaf.path, "Top \u{203a} Middle \u{203a} Leaf");

        let middle = tree.sections.iter().find(|r| r.title == "Middle").unwrap();
        assert_eq!(middle.path, "Top \u{203a} Middle");
        assert!(middle.parent_section_id.is_some());

        let top = tree.sections.iter().find(|r| r.title == "Top").unwrap();
        assert!(top.parent_section_id.is_none());
        assert_eq!(top.ordinal, 0);
    }

    #[test]
    fn test_flatten_monotonic_ordinal() {
        // h1 A with children B, C: B.ordinal=0, C.ordinal=1
        let d = doc(vec![
            heading(1, "A", 0, 5),
            heading(2, "B", 6, 11),
            para("b", 12, 13),
            heading(2, "C", 14, 19),
            para("c", 20, 21),
        ]);
        let roots = build_tree(&d, "doc.test1234");
        let tree = flatten_tree(&roots, "doc.test1234", "sha", "A", None);
        let b = tree.sections.iter().find(|r| r.title == "B").unwrap();
        let c = tree.sections.iter().find(|r| r.title == "C").unwrap();
        assert_eq!(b.ordinal, 0);
        assert_eq!(c.ordinal, 1);
    }

    #[test]
    fn test_table_appended_to_current_section() {
        let mut d = doc(vec![
            heading(1, "Section", 0, 10),
            para("intro", 11, 16),
        ]);
        d.tables = vec![NormalizedTable {
            headers: vec!["Name".to_string(), "Value".to_string()],
            rows: vec![vec!["A".to_string(), "1".to_string()]],
            span: SourceSpan { start: 17, end: 40 },
        }];
        let roots = build_tree(&d, "doc.test1234");
        let text = roots[0].text.as_deref().unwrap_or("");
        assert!(text.contains("Name | Value"), "table headers should be in section text");
        assert!(text.contains("A | 1"), "table row should be in section text");
    }
}
