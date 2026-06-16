use axonmind_engine::{
    AxonMindEngine,
    config::EngineConfig,
    ingest::{DocumentBlock, NormalizedDocument, NormalizedTable, SourceSpan, render_markdown},
};
use std::path::PathBuf;
use tempfile::TempDir;

fn test_engine_config(dir: &TempDir) -> EngineConfig {
    EngineConfig::from_workspace_dir(dir.path().to_path_buf())
}

// ── render_markdown unit tests ────────────────────────────────────────────────

fn make_doc(blocks: Vec<DocumentBlock>, tables: Vec<NormalizedTable>) -> NormalizedDocument {
    NormalizedDocument {
        id: "doc.abc12345".into(),
        source_path: None,
        sha256: "abc123".into(),
        title: Some("Test".into()),
        blocks,
        tables,
    }
}

#[test]
fn render_markdown_heading_and_paragraph() {
    let doc = make_doc(
        vec![
            DocumentBlock::Heading {
                level: 1,
                text: "Title".into(),
                span: SourceSpan { start: 0, end: 5 },
            },
            DocumentBlock::Paragraph {
                text: "Body text.".into(),
                span: SourceSpan { start: 6, end: 16 },
            },
        ],
        vec![],
    );
    let md = render_markdown(&doc);
    // WHY: heading must become `# Title` and paragraph must follow
    assert!(md.contains("# Title"), "heading not rendered: {md}");
    assert!(md.contains("Body text."), "paragraph not rendered: {md}");
}

#[test]
fn render_markdown_table_interleaved() {
    let doc = make_doc(
        vec![
            DocumentBlock::Paragraph {
                text: "Before".into(),
                span: SourceSpan { start: 0, end: 6 },
            },
            DocumentBlock::Paragraph {
                text: "After".into(),
                span: SourceSpan { start: 50, end: 55 },
            },
        ],
        vec![NormalizedTable {
            headers: vec!["A".into(), "B".into()],
            rows: vec![vec!["1".into(), "2".into()]],
            span: SourceSpan { start: 10, end: 40 },
        }],
    );
    let md = render_markdown(&doc);
    let before_pos = md.find("Before").expect("Before not found");
    let table_pos = md.find("| A |").expect("table header not found");
    let after_pos = md.find("After").expect("After not found");
    // WHY: source order (Before → table → After) must be preserved so the rendered
    // markdown faithfully reflects the original document layout
    assert!(before_pos < table_pos, "table should come after Before");
    assert!(table_pos < after_pos, "After should come after table");
}

#[test]
fn render_markdown_list_and_code() {
    let doc = make_doc(
        vec![
            DocumentBlock::ListItem {
                text: "item one".into(),
                span: SourceSpan { start: 0, end: 8 },
            },
            DocumentBlock::CodeBlock {
                language: Some("rust".into()),
                text: "fn main() {}".into(),
                span: SourceSpan { start: 9, end: 21 },
            },
        ],
        vec![],
    );
    let md = render_markdown(&doc);
    assert!(md.contains("- item one"), "list item not rendered: {md}");
    assert!(md.contains("```rust"), "code fence not rendered: {md}");
    assert!(md.contains("fn main() {}"), "code body not rendered: {md}");
}

// ── ingest_file_with_content integration test ─────────────────────────────────

#[tokio::test]
async fn ingest_file_with_content_returns_markdown_and_provenance() {
    let dir = TempDir::new().unwrap();
    let config = test_engine_config(&dir);
    let engine = AxonMindEngine::open(config).await.unwrap();

    // Use the bundled sample fixture
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures/sample.md");

    let result = engine.ingest_file_with_content(&fixture).await.unwrap();

    // WHY: doc_id must be the graph node handle so callers can trace the returned
    // content back to the indexed document and its retained blob
    assert!(
        result.doc_id.starts_with("doc."),
        "doc_id must be a graph node id, got: {}",
        result.doc_id
    );
    assert_eq!(result.sha256.len(), 64, "sha256 must be a hex SHA-256");
    // WHY: markdown must be non-empty so callers can surface the parsed content without re-parsing
    assert!(
        !result.markdown.trim().is_empty(),
        "markdown must not be empty"
    );
    // WHY: blob must be retained for provenance / re-parse / audit
    let blob_path = dir.path().join("blobs").join(&result.sha256);
    assert!(
        blob_path.exists(),
        "original file must be blob-retained at blobs/<sha256>"
    );
}
