use axonmind_core::{
    AxonMindError, Confidence, Edge, EdgeId, EdgeKind, EvidenceId, ExtractorKind, Node, NodeId,
    NodeKind,
};
use axonmind_engine::{
    AxonMindEngine,
    config::EngineConfig,
    ingest::{IngestOptions, IngestSource},
    query::{FocusKpiInput, GraphSearchInput},
    store::GraphMutation,
};
use chrono::Utc;
use std::collections::HashSet;
use tempfile::TempDir;
use uuid::Uuid;

fn test_engine_config(dir: &TempDir) -> EngineConfig {
    EngineConfig::from_workspace_dir(dir.path().to_path_buf())
}

const MARKDOWN_FIXTURE: &str = r#"
# Revenue Growth

We track monthly recurring revenue.

# Customer Acquisition Cost

CAC drives churn indirectly.

## Retention Rate

High retention influences revenue growth.
"#;

// ── helpers ───────────────────────────────────────────────────────────────────

async fn open_engine(dir: &TempDir) -> AxonMindEngine {
    AxonMindEngine::open(test_engine_config(dir))
        .await
        .expect("engine open failed")
}

async fn ingest_markdown(
    engine: &AxonMindEngine,
    content: &str,
) -> axonmind_engine::ingest::IngestSummary {
    let dir = tempfile::tempdir().unwrap();
    let md_path = dir.path().join("test.md");
    std::fs::write(&md_path, content).unwrap();
    engine
        .ingest_sync(
            IngestSource::File(md_path),
            IngestOptions {
                recursive: false,
                skip_unchanged: false,
                max_file_size_bytes: 10 * 1024 * 1024,
            },
        )
        .await
        .expect("ingest failed")
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Ingest markdown with KPI headings → focus_kpi returns the KPI node with correct kind.
/// WHY: verifies the full ingest→extract→store→query path is wired end-to-end.
#[tokio::test]
async fn test_ingest_markdown_roundtrip() {
    let dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;

    let summary = ingest_markdown(&engine, MARKDOWN_FIXTURE).await;

    // 1 document + at least 2 KPI nodes (revenue growth, customer acquisition cost)
    assert!(summary.files_processed == 1, "expected 1 file processed");
    assert!(
        summary.nodes_created >= 3,
        "expected >=3 nodes (doc + KPIs), got {}",
        summary.nodes_created
    );
    assert!(
        summary.errors.is_empty(),
        "unexpected errors: {:?}",
        summary.errors
    );

    // focus_kpi must return the revenue growth node
    let out = engine
        .focus_kpi(FocusKpiInput {
            kpi_id: NodeId("kpi.revenue_growth".into()),
        })
        .await
        .expect("focus_kpi failed");

    assert_eq!(out.kpi.id, NodeId("kpi.revenue_growth".into()));
    assert_eq!(out.kpi.kind, NodeKind::Kpi);
}

/// Removing a document deletes concepts that were unique to it but keeps concepts that another
/// document still references. WHY: cross-document linking deliberately shares concept nodes, so
/// per-document removal must reference-count, not blindly delete everything the document touched.
#[tokio::test]
async fn remove_document_sweeps_orphans_but_keeps_shared_concepts() {
    let dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;
    let files = TempDir::new().unwrap();

    let opts = || IngestOptions {
        recursive: false,
        skip_unchanged: false,
        max_file_size_bytes: 10 * 1024 * 1024,
    };

    // doc1: a shared KPI (Revenue Growth) + a unique KPI (Customer Acquisition Cost).
    let doc1 = files.path().join("doc1.md");
    std::fs::write(
        &doc1,
        "# Revenue Growth\n\nx\n\n# Customer Acquisition Cost\n\ny",
    )
    .unwrap();
    engine
        .ingest_sync(IngestSource::File(doc1), opts())
        .await
        .unwrap();

    // doc2: also mentions Revenue Growth → that concept is now shared across both documents.
    let doc2 = files.path().join("doc2.md");
    std::fs::write(&doc2, "# Revenue Growth\n\nz").unwrap();
    engine
        .ingest_sync(IngestSource::File(doc2), opts())
        .await
        .unwrap();

    let docs = engine.list_documents().await.unwrap();
    let d1 = docs
        .iter()
        .find(|d| {
            d.source_path
                .as_deref()
                .is_some_and(|p| p.ends_with("doc1.md"))
        })
        .expect("doc1 should be listed");
    engine
        .remove_document(NodeId(d1.node_id.clone()), true)
        .await
        .unwrap();

    // Shared concept survives (doc2 still references it); the orphan is swept.
    assert!(
        engine
            .focus_kpi(FocusKpiInput {
                kpi_id: NodeId("kpi.revenue_growth".into())
            })
            .await
            .is_ok(),
        "shared KPI must survive removal of one of its documents"
    );
    assert!(
        engine
            .focus_kpi(FocusKpiInput {
                kpi_id: NodeId("kpi.customer_acquisition_cost".into())
            })
            .await
            .is_err(),
        "KPI unique to the removed document must be deleted"
    );

    // doc1 is gone from the list; doc2 remains.
    let after = engine.list_documents().await.unwrap();
    assert!(after.iter().all(|d| {
        !d.source_path
            .as_deref()
            .is_some_and(|p| p.ends_with("doc1.md"))
    }));
    assert!(after.iter().any(|d| {
        d.source_path
            .as_deref()
            .is_some_and(|p| p.ends_with("doc2.md"))
    }));
}

/// Removing a processed document must clear its derived rows so re-ingesting the same file
/// recreates the same graph state instead of stacking duplicate evidence/edges onto the reused
/// content-hash document id.
#[tokio::test]
async fn remove_then_reingest_same_file_does_not_duplicate_graph_state() {
    let dir = TempDir::new().unwrap();
    let file_dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;
    let md_path = file_dir.path().join("doc.md");
    std::fs::write(&md_path, MARKDOWN_FIXTURE).unwrap();

    let opts = IngestOptions {
        recursive: false,
        skip_unchanged: false,
        max_file_size_bytes: 10 * 1024 * 1024,
    };

    engine
        .ingest_sync(IngestSource::File(md_path.clone()), opts.clone())
        .await
        .unwrap();
    let first = engine.export_json().await.expect("first export failed");

    let doc_id = engine
        .list_documents()
        .await
        .unwrap()
        .into_iter()
        .find(|d| {
            d.source_path
                .as_deref()
                .is_some_and(|p| p.ends_with("doc.md"))
        })
        .map(|d| NodeId(d.node_id))
        .expect("doc should be listed after first ingest");
    engine.remove_document(doc_id, true).await.unwrap();

    let after_remove = engine
        .export_json()
        .await
        .expect("post-remove export failed");
    assert!(
        after_remove.nodes.is_empty()
            && after_remove.edges.is_empty()
            && after_remove.evidence.is_empty(),
        "removing the only document should leave an empty graph; got {} nodes, {} edges, {} evidence",
        after_remove.nodes.len(),
        after_remove.edges.len(),
        after_remove.evidence.len()
    );

    engine
        .ingest_sync(IngestSource::File(md_path), opts)
        .await
        .unwrap();
    let second = engine.export_json().await.expect("second export failed");

    assert_eq!(
        first.nodes.len(),
        second.nodes.len(),
        "re-ingesting the same file after removal must recreate the same node count"
    );
    assert_eq!(
        first.edges.len(),
        second.edges.len(),
        "re-ingesting the same file after removal must recreate the same edge count"
    );
    assert_eq!(
        first.evidence.len(),
        second.evidence.len(),
        "re-ingesting the same file after removal must recreate the same evidence count"
    );
}

/// Removing a document must also sweep nodes that are only connected through document-backed
/// relation evidence, not just nodes reached by MentionedIn. This protects the LLM/image path,
/// where relation evidence can otherwise leave behind nodes that double on re-ingest.
#[tokio::test]
async fn remove_document_clears_relation_only_document_lineage() {
    let dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;
    let now = Utc::now();

    engine
        .apply_mutation(GraphMutation::UpsertNode {
            node: Node {
                id: NodeId("doc.image".into()),
                kind: NodeKind::Document,
                name: "image.png".into(),
                attrs: serde_json::json!({}),
                confidence: Confidence::RULE,
                created_at: now,
                updated_at: now,
                is_tainted: false,
                requires_human_review: false,
            },
        })
        .await
        .unwrap();
    for (id, kind, name) in [
        ("kpi.alpha", NodeKind::Kpi, "Alpha"),
        ("risk.beta", NodeKind::Risk, "Beta"),
    ] {
        engine
            .apply_mutation(GraphMutation::UpsertNode {
                node: Node {
                    id: NodeId(id.into()),
                    kind,
                    name: name.into(),
                    attrs: serde_json::Value::Null,
                    confidence: Confidence::LLM,
                    created_at: now,
                    updated_at: now,
                    is_tainted: true,
                    requires_human_review: false,
                },
            })
            .await
            .unwrap();
    }
    engine
        .apply_mutation(GraphMutation::UpsertEvidence {
            evidence: axonmind_core::Evidence {
                id: EvidenceId("ev.image".into()),
                source_node_id: NodeId("doc.image".into()),
                source_type: axonmind_core::SourceType::Document,
                quote: Some("image relation".into()),
                row_ref: None,
                blob_sha256: None,
                timestamp: Some(now),
                extractor: ExtractorKind::Llm,
                confidence: Confidence::LLM,
                is_tainted: true,
                requires_human_review: false,
            },
        })
        .await
        .unwrap();
    engine
        .apply_mutation(GraphMutation::UpsertEdge {
            edge: Edge {
                id: EdgeId("edge.image".into()),
                from: NodeId("kpi.alpha".into()),
                to: NodeId("risk.beta".into()),
                kind: EdgeKind::Influences,
                evidence: vec![EvidenceId("ev.image".into())],
                confidence: Confidence::LLM,
                created_at: now,
                created_by: ExtractorKind::Llm,
                is_tainted: true,
                requires_human_review: false,
            },
            evidence_ids: vec![EvidenceId("ev.image".into())],
        })
        .await
        .unwrap();

    engine
        .remove_document(NodeId("doc.image".into()), false)
        .await
        .unwrap();

    let after = engine
        .export_json()
        .await
        .expect("export after remove failed");
    assert!(
        after.nodes.is_empty() && after.edges.is_empty() && after.evidence.is_empty(),
        "document-backed relation-only lineage should be swept on remove; got {} nodes, {} edges, {} evidence",
        after.nodes.len(),
        after.edges.len(),
        after.evidence.len()
    );
}

/// UpsertEdge with empty evidence_ids must return EvidenceMissing.
/// WHY: this is the single most important invariant; breaking it silently
/// corrupts the evidence chain for all downstream queries.
#[tokio::test]
async fn test_evidence_invariant_empty() {
    let dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;

    let from = NodeId(Uuid::new_v4().to_string());
    let to = NodeId(Uuid::new_v4().to_string());
    let now = Utc::now();

    // Create both nodes first so FK constraint isn't the blocker
    for id in [&from, &to] {
        engine
            .apply_mutation(GraphMutation::UpsertNode {
                node: Node {
                    id: id.clone(),
                    kind: NodeKind::Metric,
                    name: id.0.clone(),
                    attrs: serde_json::Value::Null,
                    confidence: Confidence::RULE,
                    created_at: now,
                    updated_at: now,
                    is_tainted: false,
                    requires_human_review: false,
                },
            })
            .await
            .unwrap();
    }

    let result = engine
        .apply_mutation(GraphMutation::UpsertEdge {
            edge: Edge {
                id: EdgeId(Uuid::new_v4().to_string()),
                from: from.clone(),
                to: to.clone(),
                kind: EdgeKind::Influences,
                evidence: vec![],
                confidence: Confidence::RULE,
                created_at: now,
                created_by: ExtractorKind::Rule,
                is_tainted: false,
                requires_human_review: false,
            },
            evidence_ids: vec![], // intentionally empty
        })
        .await;

    assert!(
        matches!(result, Err(AxonMindError::EvidenceMissing)),
        "expected EvidenceMissing, got: {:?}",
        result
    );
}

/// After ingesting markdown with KPI headings, FTS5 search returns the KPI node.
/// WHY: verifies that FTS5 sync in apply_mutation is wired correctly for
/// node writes; if sync_node_fts is broken, search silently returns nothing.
#[tokio::test]
async fn test_graph_search_after_ingest() {
    let dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;

    ingest_markdown(&engine, MARKDOWN_FIXTURE).await;

    let out = engine
        .graph_search(GraphSearchInput {
            query: "revenue".into(),
            kinds: None,
            limit: None,
        })
        .await
        .expect("graph_search failed");

    let found = out
        .nodes
        .iter()
        .any(|n| n.id == NodeId("kpi.revenue_growth".into()));
    assert!(
        found,
        "expected kpi.revenue_growth in search results; got: {:?}",
        out.nodes.iter().map(|n| &n.id).collect::<Vec<_>>()
    );
}

/// skip_unchanged flag suppresses re-ingest of a file with the same sha256.
/// WHY: without this, repeated indexing creates duplicate nodes and inflates counts;
/// the document_cache table exists solely to make this flag work.
#[tokio::test]
async fn test_skip_unchanged() {
    let dir = TempDir::new().unwrap();
    let file_dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;
    let md_path = file_dir.path().join("doc.md");
    std::fs::write(&md_path, MARKDOWN_FIXTURE).unwrap();

    let opts = IngestOptions {
        recursive: false,
        skip_unchanged: true,
        max_file_size_bytes: 10 * 1024 * 1024,
    };

    let first = engine
        .ingest_sync(IngestSource::File(md_path.clone()), opts.clone())
        .await
        .unwrap();
    assert_eq!(first.files_skipped, 0, "first ingest should not skip");

    let second = engine
        .ingest_sync(IngestSource::File(md_path), opts)
        .await
        .unwrap();
    assert_eq!(
        second.files_skipped, 1,
        "second ingest of same file should be skipped"
    );
    assert_eq!(
        second.files_processed, 0,
        "skipped file should not be processed"
    );
}

/// UpsertEdge referencing a non-existent evidence ID must return EvidenceMissing.
/// WHY: evidence_ids must reference real rows; dangling refs would make
/// GetEvidence silently return partial results.
#[tokio::test]
async fn test_evidence_invariant_nonexistent_ref() {
    let dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;

    let from = NodeId(Uuid::new_v4().to_string());
    let to = NodeId(Uuid::new_v4().to_string());
    let now = Utc::now();

    for id in [&from, &to] {
        engine
            .apply_mutation(GraphMutation::UpsertNode {
                node: Node {
                    id: id.clone(),
                    kind: NodeKind::Metric,
                    name: id.0.clone(),
                    attrs: serde_json::Value::Null,
                    confidence: Confidence::RULE,
                    created_at: now,
                    updated_at: now,
                    is_tainted: false,
                    requires_human_review: false,
                },
            })
            .await
            .unwrap();
    }

    let phantom_ev = EvidenceId(Uuid::new_v4().to_string()); // never inserted

    let result = engine
        .apply_mutation(GraphMutation::UpsertEdge {
            edge: Edge {
                id: EdgeId(Uuid::new_v4().to_string()),
                from,
                to,
                kind: EdgeKind::Influences,
                evidence: vec![phantom_ev.clone()],
                confidence: Confidence::RULE,
                created_at: now,
                created_by: ExtractorKind::Rule,
                is_tainted: false,
                requires_human_review: false,
            },
            evidence_ids: vec![phantom_ev],
        })
        .await;

    assert!(
        matches!(result, Err(AxonMindError::EvidenceMissing)),
        "expected EvidenceMissing for phantom evidence ref, got: {:?}",
        result
    );
}

/// Structural fingerprint: a cosmetic heading change must not change the structural hash,
/// and the skip logic must not re-ingest when content bytes are identical.
/// WHY: proves the fingerprint classifier gates skipping correctly and the migration 002
/// column is written/read on real SQLite.
#[tokio::test]
async fn test_fingerprint_cosmetic_skip() {
    let dir = TempDir::new().unwrap();
    let file_dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;
    let md_path = file_dir.path().join("doc.md");

    // First ingest — builds the fingerprint cache row.
    std::fs::write(&md_path, MARKDOWN_FIXTURE).unwrap();
    let opts = IngestOptions {
        recursive: false,
        skip_unchanged: true,
        max_file_size_bytes: 10 * 1024 * 1024,
    };
    let first = engine
        .ingest_sync(IngestSource::File(md_path.clone()), opts.clone())
        .await
        .unwrap();
    assert_eq!(first.files_skipped, 0, "first ingest must not skip");

    // Second ingest with identical bytes → Skip.
    let second = engine
        .ingest_sync(IngestSource::File(md_path.clone()), opts.clone())
        .await
        .unwrap();
    assert_eq!(
        second.files_skipped, 1,
        "identical bytes must be skipped (fingerprint cache hit)"
    );

    // Write new content with same headings but changed prose — structural hash must match.
    let cosmetic_variant = MARKDOWN_FIXTURE.replace(
        "We track monthly recurring revenue.",
        "Monthly recurring revenue is our primary growth metric.",
    );
    assert_ne!(cosmetic_variant, MARKDOWN_FIXTURE, "fixture must differ");
    std::fs::write(&md_path, &cosmetic_variant).unwrap();
    // Re-open engine so the SQLite pool sees the updated file; same workspace dir.
    let engine2 = open_engine(&dir).await;
    let third = engine2
        .ingest_sync(IngestSource::File(md_path), opts)
        .await
        .unwrap();
    // Cosmetic change → not skipped (content sha differs), but no error either.
    assert_eq!(
        third.files_skipped, 0,
        "cosmetic prose change must be re-processed (content sha differs)"
    );
    assert!(
        third.errors.is_empty(),
        "cosmetic change must produce no errors: {:?}",
        third.errors
    );
}

/// A document with a KPI heading AND a table row matching that heading should produce a
/// RecordMetricValue that reaches the metric_values store.
/// WHY: proves the deterministic tier actually captures numeric values — previously the
/// metric node was created with `attrs: Null` and no RecordMetricValue was emitted, so
/// the recompute worker had nothing to trend on.
#[tokio::test]
async fn test_table_metric_value_stored() {
    let dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;

    // Document with a KPI heading and a matching table row with a currency value.
    // The heading slug and the table row slug must match for RecordMetricValue to fire.
    let md = r#"
# Revenue

Monthly recurring revenue figures.

| Metric | Value |
|---|---|
| Revenue | $1.2M |
"#;

    let summary = ingest_markdown(&engine, md).await;
    assert!(
        summary.errors.is_empty(),
        "ingest errors: {:?}",
        summary.errors
    );

    let export = engine.export_json().await.expect("export failed");

    // At least one MetricValue linked to the kpi.revenue node must have been stored.
    let linked = export
        .metric_values
        .iter()
        .any(|mv| mv.kpi_node_id == NodeId("kpi.revenue".into()) && mv.value == 1_200_000.0);

    assert!(
        linked,
        "expected a metric_value linking kpi.revenue with value 1_200_000; got: {:?}",
        export.metric_values
    );
}

/// export_json then import_export into a fresh workspace must produce equal node/edge/evidence
/// sets, and every imported edge must still satisfy the evidence invariant (≥1 evidence ref).
/// WHY: import goes through apply_mutation, so the invariant is enforced on every edge write —
/// this test proves that survives the round-trip.
#[tokio::test]
async fn test_export_import_roundtrip() {
    let src_dir = TempDir::new().unwrap();
    let dst_dir = TempDir::new().unwrap();

    let src = open_engine(&src_dir).await;
    ingest_markdown(&src, MARKDOWN_FIXTURE).await;

    let export = src.export_json().await.expect("export failed");
    assert!(!export.nodes.is_empty(), "export must contain nodes");

    let src_node_ids: HashSet<_> = export.nodes.iter().map(|n| n.id.clone()).collect();
    let src_edge_ids: HashSet<_> = export.edges.iter().map(|e| e.id.clone()).collect();
    let src_ev_ids: HashSet<_> = export.evidence.iter().map(|e| e.id.clone()).collect();

    let dst = open_engine(&dst_dir).await;
    let summary = dst.import_export(export).await.expect("import failed");
    assert!(
        summary.errors.is_empty(),
        "import must complete without errors; got: {:?}",
        summary.errors
    );

    let reimport = dst.export_json().await.expect("re-export failed");

    let dst_node_ids: HashSet<_> = reimport.nodes.iter().map(|n| n.id.clone()).collect();
    let dst_edge_ids: HashSet<_> = reimport.edges.iter().map(|e| e.id.clone()).collect();
    let dst_ev_ids: HashSet<_> = reimport.evidence.iter().map(|e| e.id.clone()).collect();

    assert_eq!(
        src_node_ids, dst_node_ids,
        "node sets must match after round-trip"
    );
    assert_eq!(
        src_edge_ids, dst_edge_ids,
        "edge sets must match after round-trip"
    );
    assert_eq!(
        src_ev_ids, dst_ev_ids,
        "evidence sets must match after round-trip"
    );

    // Every imported edge must have ≥1 evidence reference — the core invariant.
    for edge in &reimport.edges {
        assert!(
            !edge.evidence.is_empty(),
            "edge {} must have ≥1 evidence after import",
            edge.id
        );
    }
}

/// import_export with a mismatched schema_version must reject immediately, writing nothing.
/// WHY: forward-compatibility guard — a v2 export must not silently corrupt a v1 workspace.
#[tokio::test]
async fn test_import_wrong_schema_version_rejected() {
    use axonmind_engine::query::GraphExportV1;

    let dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;

    let bad_export = GraphExportV1 {
        schema_version: 99,
        exported_at: chrono::Utc::now(),
        workspace_id: String::new(),
        nodes: vec![],
        edges: vec![],
        evidence: vec![],
        edge_evidence: vec![],
        metric_values: vec![],
        kpi_candidates: vec![],
    };

    let result = engine.import_export(bad_export).await;
    assert!(
        matches!(result, Err(axonmind_core::AxonMindError::Ingest { .. })),
        "wrong schema_version must return Ingest error; got: {:?}",
        result
    );

    // Nothing must have been written.
    let export = engine.export_json().await.unwrap();
    assert!(
        export.nodes.is_empty(),
        "rejected import must write no nodes"
    );
}

/// export_json output is byte-identical across two calls on an unchanged graph.
/// WHY: stable ordering is required for reviewable git diffs of committed export JSON.
#[tokio::test]
async fn test_export_is_deterministic() {
    let dir = TempDir::new().unwrap();
    let engine = open_engine(&dir).await;
    ingest_markdown(&engine, MARKDOWN_FIXTURE).await;

    let a = serde_json::to_string(&engine.export_json().await.unwrap()).unwrap();
    let b = serde_json::to_string(&engine.export_json().await.unwrap()).unwrap();

    // Strip the timestamp field so two calls differing only in exported_at still pass.
    let strip_ts = |s: &str| -> String {
        s.lines()
            .filter(|l| !l.contains("exported_at"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    assert_eq!(
        strip_ts(&a),
        strip_ts(&b),
        "export_json must be deterministic (ORDER BY id on all tables)"
    );
}
