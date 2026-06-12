use axonmind_core::{
    AxonMindError, Confidence, Edge, EdgeId, EdgeKind, Evidence, EvidenceId, ExtractorKind, Node,
    NodeId, NodeKind, SourceType,
};
use axonmind_engine::{
    events::EngineEvent,
    query::{FindConflictsInput, conflicts::find_conflicts},
    store::{GraphCache, GraphMutation, GraphStore},
};
use chrono::Utc;
use tempfile::TempDir;
use tokio::sync::{RwLock, broadcast};

// ── helpers ───────────────────────────────────────────────────────────────────

async fn open_store(
    dir: &TempDir,
) -> (
    GraphStore,
    RwLock<GraphCache>,
    broadcast::Sender<EngineEvent>,
) {
    let store = GraphStore::open(&dir.path().join("axonmind.db"))
        .await
        .unwrap();
    let cache = RwLock::new(GraphCache::new());
    let (tx, _rx) = broadcast::channel(64);
    (store, cache, tx)
}

async fn apply(
    store: &GraphStore,
    cache: &RwLock<GraphCache>,
    tx: &broadcast::Sender<EngineEvent>,
    m: GraphMutation,
) {
    store.apply_mutation(m, cache, tx).await.unwrap();
}

fn make_node(id: &str, kind: NodeKind) -> Node {
    let now = Utc::now();
    Node {
        id: NodeId(id.to_owned()),
        kind,
        name: id.to_owned(),
        attrs: serde_json::Value::Null,
        confidence: Confidence(0.8),
        is_tainted: false,
        requires_human_review: false,
        created_at: now,
        updated_at: now,
    }
}

fn make_evidence(id: &str, source: &str) -> Evidence {
    Evidence {
        id: EvidenceId(id.to_owned()),
        source_node_id: NodeId(source.to_owned()),
        source_type: SourceType::Document,
        quote: Some(format!("quote for {id}")),
        row_ref: None,
        blob_sha256: None,
        timestamp: None,
        extractor: ExtractorKind::Llm,
        confidence: Confidence(0.9),
        is_tainted: false,
        requires_human_review: false,
    }
}

fn make_edge(id: &str, from: &str, to: &str, kind: EdgeKind) -> Edge {
    Edge {
        id: EdgeId(id.to_owned()),
        from: NodeId(from.to_owned()),
        to: NodeId(to.to_owned()),
        kind,
        evidence: vec![],
        confidence: Confidence(0.75),
        created_at: Utc::now(),
        created_by: ExtractorKind::Llm,
        is_tainted: false,
        requires_human_review: false,
    }
}

/// Upsert both nodes, one evidence item, and an edge. Returns the evidence id.
async fn wire_edge(
    store: &GraphStore,
    cache: &RwLock<GraphCache>,
    tx: &broadcast::Sender<EngineEvent>,
    from: &str,
    to: &str,
    edge_id: &str,
    kind: EdgeKind,
    ev_id: &str,
) -> EvidenceId {
    apply(store, cache, tx, GraphMutation::UpsertNode { node: make_node(from, NodeKind::Kpi) }).await;
    apply(store, cache, tx, GraphMutation::UpsertNode { node: make_node(to, NodeKind::Kpi) }).await;
    let ev = make_evidence(ev_id, from);
    apply(store, cache, tx, GraphMutation::UpsertEvidence { evidence: ev.clone() }).await;
    apply(
        store,
        cache,
        tx,
        GraphMutation::UpsertEdge {
            edge: make_edge(edge_id, from, to, kind),
            evidence_ids: vec![ev.id.clone()],
        },
    )
    .await;
    ev.id
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// WHY: an explicit Contradicts edge is the strongest signal the system can emit; it must
/// always surface in find_conflicts even when there is no opposing positive edge.
#[tokio::test]
async fn explicit_contradicts_edge_surfaces() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    let ev_id = wire_edge(&store, &cache, &tx, "kpi.a", "kpi.b", "e1", EdgeKind::Contradicts, "ev1").await;

    let out = find_conflicts(
        FindConflictsInput { node_id: None, limit: None },
        &store,
        &cache,
    )
    .await
    .unwrap();

    assert_eq!(out.conflicts.len(), 1, "one conflict pair expected");
    let pair = &out.conflicts[0];
    // The Contradicts edge has negative polarity.
    assert_eq!(pair.negative.len(), 1);
    assert_eq!(pair.positive.len(), 0);
    // Evidence must be attached.
    assert_eq!(pair.negative[0].evidence.len(), 1);
    assert_eq!(pair.negative[0].evidence[0].id, ev_id);
}

/// WHY: a polarity clash (Improves + Degrades on the same pair) is the definition of a
/// conflicting claim. Both sides must appear in the output with their citations so that
/// a reader can compare sources without further queries.
#[tokio::test]
async fn polarity_clash_surfaces_both_sides_with_citations() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    let pos_ev = wire_edge(&store, &cache, &tx, "kpi.a", "kpi.b", "e-pos", EdgeKind::Improves, "ev-pos").await;
    let neg_ev = wire_edge(&store, &cache, &tx, "kpi.a", "kpi.b", "e-neg", EdgeKind::Degrades, "ev-neg").await;

    let out = find_conflicts(
        FindConflictsInput { node_id: None, limit: None },
        &store,
        &cache,
    )
    .await
    .unwrap();

    assert_eq!(out.conflicts.len(), 1, "exactly one conflict pair");
    let pair = &out.conflicts[0];
    assert_eq!(pair.positive.len(), 1);
    assert_eq!(pair.negative.len(), 1);
    assert_eq!(pair.positive[0].evidence[0].id, pos_ev);
    assert_eq!(pair.negative[0].evidence[0].id, neg_ev);
}

/// WHY: pairs connected only by neutral edges (Influences, Causes, etc.) are NOT
/// contradictions — every KPI legitimately has both positive and negative drivers from
/// *different* node pairs. Including neutral edges would make virtually every graph look
/// conflicted, destroying precision.
#[tokio::test]
async fn neutral_only_pair_is_silent() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    // Neutral edge between A and B.
    wire_edge(&store, &cache, &tx, "kpi.a", "kpi.b", "e-inf", EdgeKind::Influences, "ev-inf").await;
    // Positive driver A→C and negative driver D→C — different pairs, not a conflict.
    wire_edge(&store, &cache, &tx, "kpi.a", "kpi.c", "e-imp", EdgeKind::Improves, "ev-imp").await;
    wire_edge(&store, &cache, &tx, "kpi.d", "kpi.c", "e-deg", EdgeKind::Degrades, "ev-deg").await;

    let out = find_conflicts(
        FindConflictsInput { node_id: None, limit: None },
        &store,
        &cache,
    )
    .await
    .unwrap();

    assert_eq!(out.conflicts.len(), 0, "no conflicts expected for distinct node pairs / neutral edges");
}

/// WHY: the node_id filter narrows results to pairs where that specific node is involved.
/// Without it, a workspace-wide scan on a large graph would overwhelm output — callers
/// scoping to one KPI must only see that KPI's conflicts.
#[tokio::test]
async fn node_id_filter_restricts_to_touching_pairs() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    // Conflict on pair (a, b).
    wire_edge(&store, &cache, &tx, "kpi.a", "kpi.b", "e1", EdgeKind::Improves, "ev1").await;
    wire_edge(&store, &cache, &tx, "kpi.a", "kpi.b", "e2", EdgeKind::Degrades, "ev2").await;
    // Conflict on pair (c, d) — unrelated to a.
    wire_edge(&store, &cache, &tx, "kpi.c", "kpi.d", "e3", EdgeKind::Improves, "ev3").await;
    wire_edge(&store, &cache, &tx, "kpi.c", "kpi.d", "e4", EdgeKind::Degrades, "ev4").await;

    // Scope to kpi.a — should return only the (a, b) pair.
    let out = find_conflicts(
        FindConflictsInput {
            node_id: Some(NodeId("kpi.a".into())),
            limit: None,
        },
        &store,
        &cache,
    )
    .await
    .unwrap();

    assert_eq!(out.conflicts.len(), 1, "only the pair touching kpi.a");
    let ids: Vec<_> = [&out.conflicts[0].node_a.id.0, &out.conflicts[0].node_b.id.0].into_iter().collect();
    assert!(ids.contains(&&"kpi.a".to_owned()));
}

/// WHY: a missing node_id must produce NodeNotFound, not an empty result, so callers can
/// distinguish "no conflicts for this node" from "this node doesn't exist."
#[tokio::test]
async fn missing_node_id_returns_node_not_found() {
    let dir = TempDir::new().unwrap();
    let (store, cache, _tx) = open_store(&dir).await;

    let res = find_conflicts(
        FindConflictsInput {
            node_id: Some(NodeId("ghost".into())),
            limit: None,
        },
        &store,
        &cache,
    )
    .await;

    assert!(matches!(res, Err(AxonMindError::NodeNotFound(_))));
}
