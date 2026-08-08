use axonmind_core::{AxonMindError, NodeId, NodeKind};
use axonmind_engine::{
    AxonMindEngine,
    config::EngineConfig,
    ingest::{IngestOptions, IngestSource},
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/anydoc")
        .join(name)
}

fn copy_fixture(name: &str, destination: &Path) -> Vec<u8> {
    let bytes = std::fs::read(fixture(name)).unwrap();
    std::fs::write(destination, &bytes).unwrap();
    bytes
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

async fn engine(workspace: &TempDir) -> AxonMindEngine {
    AxonMindEngine::open(EngineConfig::from_workspace_dir(
        workspace.path().to_path_buf(),
    ))
    .await
    .unwrap()
}

fn ingest_options() -> IngestOptions {
    IngestOptions {
        recursive: false,
        skip_unchanged: false,
        max_file_size_bytes: 50 * 1024 * 1024,
    }
}

#[test]
fn wrappers_keep_family_ownership_and_dispatch_surface() {
    for name in ["docx-text.docx", "pptx-title-order.pptx"] {
        let path = fixture(name);
        assert!(
            axonmind_engine::ingest::docx::parse(&path, &std::fs::read(&path).unwrap()).is_ok()
        );
    }
    for name in [
        "xlsx-sheet.xlsx",
        "xls-sheet.xls",
        "xlsb-issue2.xlsb",
        "ods-sheet.ods",
    ] {
        let path = fixture(name);
        assert!(
            axonmind_engine::ingest::spreadsheet::parse(&path, &std::fs::read(&path).unwrap())
                .is_ok()
        );
    }
    let csv = b"name,value\nRevenue,42\n";
    assert!(axonmind_engine::ingest::spreadsheet::parse(Path::new("sample.csv"), csv).is_ok());
    assert!(axonmind_engine::ingest::docx::parse(Path::new("wrong.xlsx"), &[]).is_err());
    assert!(axonmind_engine::ingest::spreadsheet::parse(Path::new("wrong.docx"), &[]).is_err());
    for path in ["file.DOCX", "file.XLSX", "file.odt"] {
        let error = axonmind_engine::ingest::dispatch_parse(Path::new(path), &[]).unwrap_err();
        let extension = Path::new(path).extension().unwrap().to_str().unwrap();
        assert!(
            matches!(error, AxonMindError::Ingest { message } if message == format!("unsupported file type: .{extension}"))
        );
    }
    assert!(matches!(
        axonmind_engine::ingest::dispatch_parse(Path::new("extensionless"), &[]).unwrap_err(),
        AxonMindError::Ingest { message } if message == "file has no extension"
    ));
}

#[tokio::test]
async fn docx_ingest_preserves_blob_graph_metrics_evidence_and_pageindex() {
    let workspace = TempDir::new().unwrap();
    let sources = TempDir::new().unwrap();
    let path = sources.path().join("revenue.docx");
    let bytes = copy_fixture("axonmind-revenue-v1.docx", &path);
    let expected_sha = sha256(&bytes);
    let engine = engine(&workspace).await;

    let result = engine.ingest_file_with_content(&path).await.unwrap();
    assert_eq!(result.sha256, expected_sha);
    assert_eq!(result.doc_id, format!("doc.{}", &expected_sha[..8]));
    assert_eq!(
        std::fs::read(workspace.path().join("blobs").join(&expected_sha)).unwrap(),
        bytes
    );
    assert!(result.markdown.contains("# Revenue"));
    assert!(result.markdown.contains("| Revenue | $1.2M |"));
    assert!(
        result.summary.errors.is_empty(),
        "{:?}",
        result.summary.errors
    );

    let sections = engine
        .parsed_document_sections(&result.doc_id)
        .await
        .unwrap();
    assert!(sections.iter().any(|section| {
        section
            .text
            .as_deref()
            .is_some_and(|text| text.contains("Monthly recurring revenue"))
    }));
    let export = engine.export_json().await.unwrap();
    assert!(
        export
            .nodes
            .iter()
            .any(|node| node.id == NodeId("kpi.revenue".into()))
    );
    assert!(
        export
            .metric_values
            .iter()
            .any(|value| value.value == 1_200_000.0)
    );
    let evidence_ids: std::collections::HashSet<_> = export
        .evidence
        .iter()
        .map(|evidence| evidence.id.clone())
        .collect();
    assert!(export.edges.iter().all(|edge| {
        !edge.evidence.is_empty() && edge.evidence.iter().all(|id| evidence_ids.contains(id))
    }));
}

#[tokio::test]
async fn spreadsheet_ingest_discards_sheet_headings_but_keeps_tables_and_pageindex() {
    let workspace = TempDir::new().unwrap();
    let sources = TempDir::new().unwrap();
    let path = sources.path().join("business.xlsx");
    copy_fixture("axonmind-revenue-two-sheet.xlsx", &path);
    let engine = engine(&workspace).await;

    let preview = engine.parse_file_preview(&path).await.unwrap();
    assert!(preview.blocks.is_empty());
    assert_eq!(preview.title.as_deref(), Some("business"));
    assert_eq!(preview.tables.len(), 2);
    assert_eq!(preview.tables[0].rows[0], ["Revenue", "$1.2M"]);
    assert_eq!(preview.tables[1].rows[0], ["Operating Margin", "18%"]);

    let result = engine.ingest_file_with_content(&path).await.unwrap();
    assert!(!result.markdown.contains("## Revenue"));
    assert!(result.markdown.contains("Operating Margin"));
    let export = engine.export_json().await.unwrap();
    assert!(
        !export
            .nodes
            .iter()
            .any(|node| node.id == NodeId("kpi.revenue".into()))
    );
    assert_eq!(
        export
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Metric)
            .count(),
        2
    );
    let sections = engine
        .parsed_document_sections(&result.doc_id)
        .await
        .unwrap();
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].title, "Document");
    assert!(
        sections[0]
            .text
            .as_deref()
            .is_some_and(|text| text.contains("Revenue") && text.contains("Operating Margin"))
    );
}

#[tokio::test]
async fn migrated_docx_keeps_exact_dedup_and_changed_content_versioning() {
    let workspace = TempDir::new().unwrap();
    let sources = TempDir::new().unwrap();
    let path = sources.path().join("stable.docx");
    let first_bytes = copy_fixture("axonmind-revenue-v1.docx", &path);
    let engine = engine(&workspace).await;
    let first = engine.ingest_file_with_content(&path).await.unwrap();
    let duplicate = engine.ingest_file_with_content(&path).await.unwrap();
    assert_eq!(duplicate.summary.files_skipped, 1);

    let second_bytes = copy_fixture("axonmind-cac-v2.docx", &path);
    let second = engine.ingest_file_with_content(&path).await.unwrap();
    assert_eq!(second.sha256, sha256(&second_bytes));
    assert!(
        workspace
            .path()
            .join("blobs")
            .join(sha256(&first_bytes))
            .exists()
    );
    assert!(
        workspace
            .path()
            .join("blobs")
            .join(sha256(&second_bytes))
            .exists()
    );

    let export = engine.export_json().await.unwrap();
    assert!(
        export
            .nodes
            .iter()
            .any(|node| node.id == NodeId("kpi.customer_acquisition_cost".into()))
    );
    assert!(
        !export
            .nodes
            .iter()
            .any(|node| node.id == NodeId("kpi.revenue".into()))
    );
    let conn = engine.db_pool().get().await.unwrap();
    let versions = conn
        .interact(|conn| {
            conn.query_row("SELECT COUNT(*) FROM document_versions", [], |row| {
                row.get::<_, i64>(0)
            })
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(versions, 2);
    assert_ne!(first.doc_id, second.doc_id);
}

#[tokio::test]
async fn parse_failure_keeps_only_preparse_blob_and_failed_status() {
    let workspace = TempDir::new().unwrap();
    let sources = TempDir::new().unwrap();
    let path = sources.path().join("broken.docx");
    let bytes = copy_fixture("docx-truncated.docx", &path);
    let engine = engine(&workspace).await;
    let error = engine
        .ingest_sync(IngestSource::File(path.clone()), ingest_options())
        .await
        .unwrap_err();
    assert!(
        matches!(error, AxonMindError::Ingest { message } if message.starts_with("document parse:"))
    );
    assert!(workspace.path().join("blobs").join(sha256(&bytes)).exists());

    let conn = engine.db_pool().get().await.unwrap();
    conn.interact(move |conn| {
        let (status, phase): (String, String) = conn.query_row(
            "SELECT status, phase FROM document_ingest_status WHERE source_path = ?1",
            [path.to_string_lossy().as_ref()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!((status.as_str(), phase.as_str()), ("failed", "parsing"));
        for table in [
            "nodes",
            "edges",
            "evidence",
            "metric_values",
            "kpi_candidates",
            "document_cache",
            "document_versions",
            "document_markdown",
            "page_tree",
            "page_sections",
        ] {
            let count: i64 =
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            assert_eq!(count, 0, "{table}");
        }
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn cache_is_first_and_missing_cache_reparses_without_graph_changes() {
    let workspace = TempDir::new().unwrap();
    let sources = TempDir::new().unwrap();
    let path = sources.path().join("cached.docx");
    copy_fixture("axonmind-revenue-v1.docx", &path);
    let engine = engine(&workspace).await;
    let result = engine.ingest_file_with_content(&path).await.unwrap();
    let before = engine.export_json().await.unwrap();
    let sha = result.sha256.clone();

    let conn = engine.db_pool().get().await.unwrap();
    conn.interact(move |conn| {
        conn.execute(
            "UPDATE document_markdown SET markdown = 'old parser sentinel' WHERE sha256 = ?1",
            [&sha],
        )?;
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        engine
            .get_document_content(&NodeId(result.doc_id.clone()))
            .await
            .unwrap(),
        "old parser sentinel"
    );

    let sha = result.sha256.clone();
    let conn = engine.db_pool().get().await.unwrap();
    conn.interact(move |conn| {
        conn.execute("DELETE FROM document_markdown WHERE sha256 = ?1", [&sha])?;
        Ok::<_, rusqlite::Error>(())
    })
    .await
    .unwrap()
    .unwrap();
    let regenerated = engine
        .get_document_content(&NodeId(result.doc_id))
        .await
        .unwrap();
    assert!(regenerated.contains("# Revenue"));
    let after = engine.export_json().await.unwrap();
    assert_eq!(before.nodes.len(), after.nodes.len());
    assert_eq!(before.edges.len(), after.edges.len());
    assert_eq!(before.evidence.len(), after.evidence.len());
}

#[tokio::test]
async fn utf16_csv_is_functional_through_engine_ingest_and_preview() {
    let workspace = TempDir::new().unwrap();
    let sources = TempDir::new().unwrap();
    let path = sources.path().join("unicode.csv");
    let text = "name,note,value\r\ncafé,\"left,right\",\"first\r\nsecond\"\r\n";
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(&path, &bytes).unwrap();
    let engine = engine(&workspace).await;

    let result = engine.ingest_file_with_content(&path).await.unwrap();
    assert_eq!(result.sha256, sha256(&bytes));
    assert_eq!(
        std::fs::read(workspace.path().join("blobs").join(&result.sha256)).unwrap(),
        bytes
    );
    assert!(
        result.summary.errors.is_empty(),
        "{:?}",
        result.summary.errors
    );
    for expected in ["café", "left,right", "first", "second"] {
        assert!(
            result.markdown.contains(expected),
            "{expected}: {}",
            result.markdown
        );
    }
    let preview = engine.parse_file_preview(&path).await.unwrap();
    assert!(preview.blocks.is_empty());
    assert_eq!(preview.title.as_deref(), Some("unicode"));
    assert_eq!(preview.tables[0].headers, ["name", "note", "value"]);
    let sections = engine
        .parsed_document_sections(&result.doc_id)
        .await
        .unwrap();
    assert_eq!(sections.len(), 1);
    assert!(
        sections[0]
            .text
            .as_deref()
            .is_some_and(|text| text.contains("café"))
    );
}
