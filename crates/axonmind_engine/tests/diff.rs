use axonmind_core::{
    Confidence, Edge, EdgeId, EdgeKind, Evidence, EvidenceId, ExtractorKind, Node, NodeId,
    NodeKind, SourceType,
};
use axonmind_engine::query::{GraphExportV1, diff_exports};
use chrono::Utc;

// ── Fixtures ──────────────────────────────────────────────────────────────────

fn ev(id: &str) -> Evidence {
    Evidence {
        id: EvidenceId(id.to_string()),
        source_node_id: NodeId("doc.abc".to_string()),
        source_type: SourceType::Document,
        quote: None,
        row_ref: None,
        blob_sha256: None,
        timestamp: None,
        extractor: ExtractorKind::Rule,
        confidence: Confidence(0.8),
        is_tainted: false,
        requires_human_review: false,
    }
}

fn node(id: &str, kind: NodeKind, name: &str, confidence: f32) -> Node {
    Node {
        id: NodeId(id.to_string()),
        kind,
        name: name.to_string(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        attrs: serde_json::json!({}),
        confidence: Confidence(confidence),
        is_tainted: false,
        requires_human_review: false,
    }
}

fn edge(id: &str, from: &str, to: &str, kind: EdgeKind, ev_id: &str) -> Edge {
    Edge {
        id: EdgeId(id.to_string()),
        from: NodeId(from.to_string()),
        to: NodeId(to.to_string()),
        kind,
        evidence: vec![EvidenceId(ev_id.to_string())],
        confidence: Confidence(0.8),
        created_at: Utc::now(),
        created_by: ExtractorKind::Rule,
        is_tainted: false,
        requires_human_review: false,
    }
}

fn empty_export() -> GraphExportV1 {
    GraphExportV1 {
        schema_version: 1,
        exported_at: Utc::now(),
        workspace_id: "test".to_string(),
        nodes: vec![],
        edges: vec![],
        evidence: vec![],
        edge_evidence: vec![],
        metric_values: vec![],
        kpi_candidates: vec![],
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[test]
fn uuid_churn_produces_empty_diff() {
    // Two exports with identical nodes/edges by name/kind but different UUIDs for
    // edges and non-slug nodes. The diff must be empty — this is the core invariant.
    let kpi = node("kpi.revenue_growth", NodeKind::Kpi, "Revenue Growth", 0.7);
    let risk = node("risk.uuid-v1", NodeKind::Risk, "Customer Churn", 0.6);

    let evidence_a = ev("ev-aaaaaaaa");
    let evidence_b = ev("ev-bbbbbbbb");

    // Edge with different UUID but same logical relationship
    let edge_a = edge(
        "edge-uuid-v1",
        "kpi.revenue_growth",
        "risk.uuid-v1",
        EdgeKind::Influences,
        "ev-aaaaaaaa",
    );
    let edge_b = edge(
        "edge-uuid-v2",
        "kpi.revenue_growth",
        "risk.uuid-v1",
        EdgeKind::Influences,
        "ev-bbbbbbbb",
    );

    let mut before = empty_export();
    before.nodes = vec![kpi.clone(), risk.clone()];
    before.edges = vec![edge_a];
    before.evidence = vec![evidence_a];

    let mut after = empty_export();
    after.nodes = vec![kpi, risk];
    after.edges = vec![edge_b];
    after.evidence = vec![evidence_b];

    let diff = diff_exports(&before, &after);

    assert_eq!(diff.summary.nodes_added, 0, "UUID churn must not add nodes");
    assert_eq!(
        diff.summary.nodes_removed, 0,
        "UUID churn must not remove nodes"
    );
    assert_eq!(
        diff.summary.nodes_modified, 0,
        "UUID churn must not modify nodes"
    );
    assert_eq!(diff.summary.edges_added, 0, "UUID churn must not add edges");
    assert_eq!(
        diff.summary.edges_removed, 0,
        "UUID churn must not remove edges"
    );
    assert_eq!(
        diff.summary.edges_modified, 0,
        "UUID churn must not modify edges"
    );
}

#[test]
fn confidence_change_detected() {
    let kpi_before = node("kpi.revenue_growth", NodeKind::Kpi, "Revenue Growth", 0.6);
    let kpi_after = node("kpi.revenue_growth", NodeKind::Kpi, "Revenue Growth", 0.9);

    let mut before = empty_export();
    before.nodes = vec![kpi_before];

    let mut after = empty_export();
    after.nodes = vec![kpi_after];

    let diff = diff_exports(&before, &after);

    assert_eq!(diff.summary.nodes_modified, 1);
    assert_eq!(diff.summary.nodes_added, 0);
    assert_eq!(diff.summary.nodes_removed, 0);
    let change = &diff.nodes.modified[0];
    assert!(change.changed_fields.contains(&"confidence".to_string()));
}

#[test]
fn float_noise_not_a_change() {
    // 0.7 and 0.70000004 should quantize to the same value.
    let kpi_before = node("kpi.revenue_growth", NodeKind::Kpi, "Revenue Growth", 0.7);
    let kpi_after = node(
        "kpi.revenue_growth",
        NodeKind::Kpi,
        "Revenue Growth",
        0.70000004,
    );

    let mut before = empty_export();
    before.nodes = vec![kpi_before];
    let mut after = empty_export();
    after.nodes = vec![kpi_after];

    let diff = diff_exports(&before, &after);

    assert_eq!(
        diff.summary.nodes_modified, 0,
        "float noise must not be a change"
    );
}

#[test]
fn deletion_detected_from_retained_state() {
    // A node present in `before` but absent in `after` must appear as Removed.
    // This proves we diff retained snapshots, not live tables — the test that validates
    // the tombstone-free design.
    let kpi = node("kpi.revenue_growth", NodeKind::Kpi, "Revenue Growth", 0.7);
    let risk = node("risk.customer_churn", NodeKind::Risk, "Customer Churn", 0.5);

    let mut before = empty_export();
    before.nodes = vec![kpi, risk];

    let mut after = empty_export();
    after.nodes = vec![node(
        "kpi.revenue_growth",
        NodeKind::Kpi,
        "Revenue Growth",
        0.7,
    )];

    let diff = diff_exports(&before, &after);

    assert_eq!(diff.summary.nodes_removed, 1);
    assert_eq!(diff.nodes.removed[0].logical_key, "Risk:customer_churn");
}

#[test]
fn rename_is_add_plus_remove() {
    // A name change means a new logical key → reported as remove + add, by design.
    // This is the accepted limitation: without an LLM we can't prove identity on rename.
    let before_node = node("kpi.rev_growth", NodeKind::Kpi, "Rev Growth", 0.7);
    let after_node = node("kpi.revenue_growth", NodeKind::Kpi, "Revenue Growth", 0.7);

    let mut before = empty_export();
    before.nodes = vec![before_node];
    let mut after = empty_export();
    after.nodes = vec![after_node];

    let diff = diff_exports(&before, &after);

    assert_eq!(diff.summary.nodes_added, 1, "rename should appear as 1 add");
    assert_eq!(
        diff.summary.nodes_removed, 1,
        "rename should appear as 1 remove"
    );
    assert_eq!(diff.summary.nodes_modified, 0);
}

#[test]
fn attrs_key_order_not_a_change() {
    let mut kpi_before = node("kpi.revenue_growth", NodeKind::Kpi, "Revenue Growth", 0.7);
    kpi_before.attrs = serde_json::json!({ "a": 1, "b": 2 });

    let mut kpi_after = kpi_before.clone();
    kpi_after.attrs = serde_json::json!({ "b": 2, "a": 1 });

    let mut before = empty_export();
    before.nodes = vec![kpi_before];
    let mut after = empty_export();
    after.nodes = vec![kpi_after];

    let diff = diff_exports(&before, &after);

    assert_eq!(
        diff.summary.nodes_modified, 0,
        "attrs key reorder must not be a change"
    );
}

#[test]
fn attrs_value_change_detected() {
    let mut kpi_before = node("kpi.revenue_growth", NodeKind::Kpi, "Revenue Growth", 0.7);
    kpi_before.attrs = serde_json::json!({ "target": 100 });

    let mut kpi_after = kpi_before.clone();
    kpi_after.attrs = serde_json::json!({ "target": 120 });

    let mut before = empty_export();
    before.nodes = vec![kpi_before];
    let mut after = empty_export();
    after.nodes = vec![kpi_after];

    let diff = diff_exports(&before, &after);

    assert_eq!(diff.summary.nodes_modified, 1);
    let change = &diff.nodes.modified[0];
    assert!(change.changed_fields.contains(&"attrs".to_string()));
}

#[test]
fn volatile_provenance_attrs_not_a_change() {
    // source_refs / last_recomputed_at churn on every re-index (the source doc id changes).
    // The KPI itself is semantically unchanged, so it must NOT be reported as modified.
    // This is the regression test for the false-positive found in end-to-end testing.
    let mut kpi_before = node("kpi.churn_rate", NodeKind::Kpi, "Churn Rate", 0.7);
    kpi_before.attrs = serde_json::json!({
        "definition": "Churn Rate",
        "source_refs": ["doc.2b1149b0"],
        "last_recomputed_at": 1000
    });

    let mut kpi_after = kpi_before.clone();
    kpi_after.attrs = serde_json::json!({
        "definition": "Churn Rate",
        "source_refs": ["doc.ddefb84b"],
        "last_recomputed_at": 2000
    });

    let mut before = empty_export();
    before.nodes = vec![kpi_before];
    let mut after = empty_export();
    after.nodes = vec![kpi_after];

    let diff = diff_exports(&before, &after);

    assert_eq!(
        diff.summary.nodes_modified, 0,
        "provenance-only attrs churn must not be reported as a change"
    );
}

#[test]
fn volatile_change_alongside_real_change_still_reports_attrs() {
    // If a real attrs field changes AND provenance churns, it's still a change — but only
    // because of the real field, not the provenance.
    let mut kpi_before = node("kpi.churn_rate", NodeKind::Kpi, "Churn Rate", 0.7);
    kpi_before.attrs = serde_json::json!({ "target": 5, "source_refs": ["doc.a"] });
    let mut kpi_after = kpi_before.clone();
    kpi_after.attrs = serde_json::json!({ "target": 9, "source_refs": ["doc.b"] });

    let mut before = empty_export();
    before.nodes = vec![kpi_before];
    let mut after = empty_export();
    after.nodes = vec![kpi_after];

    let diff = diff_exports(&before, &after);
    assert_eq!(diff.summary.nodes_modified, 1);
    assert!(
        diff.nodes.modified[0]
            .changed_fields
            .contains(&"attrs".to_string())
    );
}

#[test]
fn node_logical_key_collision_is_warned_not_dropped_silently() {
    // Two distinct node ids that slugify to the same logical key. The diff must keep one and
    // record a warning rather than silently swallowing the collision (fail loud).
    let a = node("kpi.uuid-1", NodeKind::Kpi, "Revenue Growth", 0.7);
    let b = node("kpi.uuid-2", NodeKind::Kpi, "Revenue  Growth", 0.7); // double space → same slug

    let mut before = empty_export();
    before.nodes = vec![a, b];
    let after = empty_export();

    let diff = diff_exports(&before, &after);

    assert!(
        diff.warnings.iter().any(|w| w.contains("collision")),
        "expected a collision warning, got: {:?}",
        diff.warnings
    );
}
