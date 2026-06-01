use axonmind_core::{
    AxonMindError, Confidence, Edge, EdgeId, EdgeKind, Evidence, EvidenceId, ExtractorKind, Node,
    NodeId, NodeKind, SourceType,
};
use axonmind_engine::events::EngineEvent;
use axonmind_engine::{
    query::{
        ExplainKpiInput, FocusKpiInput, GetEvidenceInput, GraphSearchInput, ImpactRadiusInput,
        SuggestActionsInput, TraceDecisionInput,
        evidence::{explain_kpi, get_evidence},
        focus::focus_kpi,
        impact::{impact_radius, suggest_actions, trace_decision},
        search::graph_search,
    },
    store::{GraphCache, GraphMutation, GraphStore},
};
use chrono::Utc;
use tempfile::TempDir;
use tokio::sync::{RwLock, broadcast};

// ── helpers ──────────────────────────────────────────────────────────────────

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
        extractor: ExtractorKind::Rule,
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
        created_by: ExtractorKind::Rule,
        is_tainted: false,
        requires_human_review: false,
    }
}

/// Upsert two nodes, one evidence record, and an edge between them. Returns the evidence ID.
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
    let ev = make_evidence(ev_id, from);
    apply(
        store,
        cache,
        tx,
        GraphMutation::UpsertEvidence {
            evidence: ev.clone(),
        },
    )
    .await;
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

// ── focus_kpi ─────────────────────────────────────────────────────────────────

/// WHY: focus_kpi must fail fast on missing IDs; if it silently returned empty output,
/// callers would interpret absence of data as an empty KPI rather than a bad request.
#[tokio::test]
async fn focus_kpi_node_not_found() {
    let dir = TempDir::new().unwrap();
    let (store, cache, _tx) = open_store(&dir).await;

    let res = focus_kpi(
        FocusKpiInput {
            kpi_id: NodeId("ghost".into()),
        },
        &store,
        &cache,
    )
    .await;
    assert!(matches!(res, Err(AxonMindError::NodeNotFound(_))));
}

/// WHY: focus_kpi must reject non-KPI nodes so the caller cannot interpret a Team or Document
/// as a KPI — it would silently return empty drivers/blockers rather than an error.
#[tokio::test]
async fn focus_kpi_wrong_kind_returns_not_a_kpi() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("team.eng", NodeKind::Team),
        },
    )
    .await;

    let res = focus_kpi(
        FocusKpiInput {
            kpi_id: NodeId("team.eng".into()),
        },
        &store,
        &cache,
    )
    .await;
    assert!(matches!(res, Err(AxonMindError::NotAKpi(_))));
}

/// WHY: drivers and blockers are the primary output of focus_kpi; wrong edge-kind routing
/// here means influences become blockers or vice versa, silently inverting the business signal.
#[tokio::test]
async fn focus_kpi_categorises_drivers_and_blockers() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("kpi.rev", NodeKind::Kpi),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("metric.cac", NodeKind::Metric),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("risk.churn", NodeKind::Risk),
        },
    )
    .await;

    // driver: Influences kpi.rev
    wire_edge(
        &store,
        &cache,
        &tx,
        "metric.cac",
        "kpi.rev",
        "e-driver",
        EdgeKind::Influences,
        "ev-driver",
    )
    .await;
    // blocker: Blocks kpi.rev
    wire_edge(
        &store,
        &cache,
        &tx,
        "risk.churn",
        "kpi.rev",
        "e-blocker",
        EdgeKind::Blocks,
        "ev-blocker",
    )
    .await;

    let out = focus_kpi(
        FocusKpiInput {
            kpi_id: NodeId("kpi.rev".into()),
        },
        &store,
        &cache,
    )
    .await
    .unwrap();

    assert_eq!(out.drivers.len(), 1, "expected 1 driver");
    assert_eq!(out.drivers[0].edge.kind, EdgeKind::Influences);
    assert_eq!(out.blockers.len(), 1, "expected 1 blocker");
    assert_eq!(out.blockers[0].edge.kind, EdgeKind::Blocks);
    assert!(
        out.risks.is_empty(),
        "expected no risks (risk is a blocker source, not a risk target)"
    );
}

/// WHY: risks are outgoing edges from the KPI to Risk nodes; if OwnedBy edges are not
/// excluded they would pollute the risks list with the owner node.
#[tokio::test]
async fn focus_kpi_returns_risks_and_owner() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("kpi.rev", NodeKind::Kpi),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("risk.market", NodeKind::Risk),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("person.cfo", NodeKind::Person),
        },
    )
    .await;

    // KPI → Risk (outgoing)
    wire_edge(
        &store,
        &cache,
        &tx,
        "kpi.rev",
        "risk.market",
        "e-risk",
        EdgeKind::Impacts,
        "ev-risk",
    )
    .await;
    // KPI → Owner
    wire_edge(
        &store,
        &cache,
        &tx,
        "kpi.rev",
        "person.cfo",
        "e-owner",
        EdgeKind::OwnedBy,
        "ev-owner",
    )
    .await;

    let out = focus_kpi(
        FocusKpiInput {
            kpi_id: NodeId("kpi.rev".into()),
        },
        &store,
        &cache,
    )
    .await
    .unwrap();

    assert_eq!(out.risks.len(), 1);
    assert_eq!(out.risks[0].to.id, NodeId("risk.market".into()));
    assert!(out.owner.is_some());
    assert_eq!(out.owner.unwrap().id, NodeId("person.cfo".into()));
}

/// WHY: evidence_count is the trust signal shown in focus_kpi output; if it drifts from
/// the actual evidence table, the UI misrepresents how well-supported the KPI is.
#[tokio::test]
async fn focus_kpi_evidence_count_matches_stored() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("kpi.rev", NodeKind::Kpi),
        },
    )
    .await;
    for i in 0..3 {
        let ev = make_evidence(&format!("ev-{i}"), "kpi.rev");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEvidence { evidence: ev },
        )
        .await;
    }

    let out = focus_kpi(
        FocusKpiInput {
            kpi_id: NodeId("kpi.rev".into()),
        },
        &store,
        &cache,
    )
    .await
    .unwrap();

    assert_eq!(out.evidence_count, 3);
}

// ── graph_search ──────────────────────────────────────────────────────────────

/// WHY: empty query must short-circuit rather than send an empty MATCH to FTS5, which
/// would return an FTS5 error or all rows depending on the SQLite version.
#[tokio::test]
async fn graph_search_empty_query_returns_empty() {
    let dir = TempDir::new().unwrap();
    let (store, _cache, _tx) = open_store(&dir).await;

    let out = graph_search(
        GraphSearchInput {
            query: "".into(),
            kinds: None,
            limit: None,
        },
        &store,
    )
    .await
    .unwrap();
    assert!(out.nodes.is_empty());
}

/// WHY: the primary use case for graph_search is finding nodes by name; if name matching
/// is broken, the tool is useless regardless of other functionality.
#[tokio::test]
async fn graph_search_returns_matching_node_by_name() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    let mut n = make_node("kpi.rev", NodeKind::Kpi);
    n.name = "RevenueGrowthUnique".to_owned();
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode { node: n.clone() },
    )
    .await;

    let out = graph_search(
        GraphSearchInput {
            query: "RevenueGrowthUnique".into(),
            kinds: None,
            limit: None,
        },
        &store,
    )
    .await
    .unwrap();

    assert!(out.nodes.iter().any(|node| node.id == n.id));
}

/// WHY: kind filtering is the main way callers narrow search to actionable results;
/// if it doesn't filter, callers get Document noise alongside KPIs with no way to distinguish.
#[tokio::test]
async fn graph_search_kind_filter_excludes_other_kinds() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    let mut kpi = make_node("kpi.rev", NodeKind::Kpi);
    kpi.name = "UniqSearchTerm".to_owned();
    let mut doc = make_node("doc.abc", NodeKind::Document);
    doc.name = "UniqSearchTerm".to_owned();

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode { node: kpi.clone() },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode { node: doc.clone() },
    )
    .await;

    let out = graph_search(
        GraphSearchInput {
            query: "UniqSearchTerm".into(),
            kinds: Some(vec![NodeKind::Kpi]),
            limit: None,
        },
        &store,
    )
    .await
    .unwrap();

    assert!(
        out.nodes.iter().any(|n| n.id == kpi.id),
        "KPI not in results"
    );
    assert!(
        !out.nodes.iter().any(|n| n.id == doc.id),
        "Document leaked through kind filter"
    );
}

/// WHY: FTS5 MATCH treats *, :, - as operators — a query containing them must not
/// cause a rusqlite error that surfaces as a 500 to the caller.
#[tokio::test]
async fn graph_search_special_chars_do_not_panic() {
    let dir = TempDir::new().unwrap();
    let (store, _cache, _tx) = open_store(&dir).await;

    let res = graph_search(
        GraphSearchInput {
            query: "revenue* :kpi -growth ^anchor".into(),
            kinds: None,
            limit: None,
        },
        &store,
    )
    .await;
    assert!(res.is_ok(), "special char query must not error: {res:?}");
}

// ── impact_radius ─────────────────────────────────────────────────────────────

/// WHY: if the cache is dirty and impact_radius silently proceeds, it reads a stale petgraph
/// snapshot and returns incomplete or phantom paths with no error signal to the caller.
#[tokio::test]
async fn impact_radius_dirty_cache_returns_error() {
    let dir = TempDir::new().unwrap();
    let (store, cache, _tx) = open_store(&dir).await;

    cache.write().await.mark_dirty();

    let res = impact_radius(
        ImpactRadiusInput {
            node_id: NodeId("any".into()),
            max_depth: None,
        },
        &store,
        &cache,
    )
    .await;
    assert!(matches!(res, Err(AxonMindError::CacheDirty)));
}

/// WHY: if impact_radius silently returned empty for a missing node instead of erroring,
/// callers would interpret it as "no impact" rather than "node doesn't exist."
#[tokio::test]
async fn impact_radius_node_not_found() {
    let dir = TempDir::new().unwrap();
    let (store, cache, _tx) = open_store(&dir).await;

    let res = impact_radius(
        ImpactRadiusInput {
            node_id: NodeId("ghost".into()),
            max_depth: None,
        },
        &store,
        &cache,
    )
    .await;
    assert!(matches!(res, Err(AxonMindError::NodeNotFound(_))));
}

/// WHY: impact_radius must traverse the graph correctly; a broken BFS means downstream
/// nodes are missing from the output, silently hiding real business dependencies.
#[tokio::test]
async fn impact_radius_traverses_outgoing_edges() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    // A → B → C
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("a", NodeKind::Initiative),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("b", NodeKind::Initiative),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("c", NodeKind::Initiative),
        },
    )
    .await;

    wire_edge(
        &store,
        &cache,
        &tx,
        "a",
        "b",
        "e-ab",
        EdgeKind::Influences,
        "ev-ab",
    )
    .await;
    wire_edge(
        &store,
        &cache,
        &tx,
        "b",
        "c",
        "e-bc",
        EdgeKind::Influences,
        "ev-bc",
    )
    .await;

    let out = impact_radius(
        ImpactRadiusInput {
            node_id: NodeId("a".into()),
            max_depth: Some(3),
        },
        &store,
        &cache,
    )
    .await
    .unwrap();

    let ids: Vec<&NodeId> = out.affected.iter().map(|a| &a.node.id).collect();
    assert!(
        ids.contains(&&NodeId("b".into())),
        "B missing from affected"
    );
    assert!(
        ids.contains(&&NodeId("c".into())),
        "C missing from affected"
    );
}

/// WHY: max_depth must be honored; if it's ignored, a deeply connected graph causes
/// unbounded traversal that returns unrelated nodes and degrades performance.
#[tokio::test]
async fn impact_radius_max_depth_stops_traversal() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    // A → B → C; max_depth=1 should return only B
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("a", NodeKind::Initiative),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("b", NodeKind::Initiative),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("c", NodeKind::Initiative),
        },
    )
    .await;

    wire_edge(
        &store,
        &cache,
        &tx,
        "a",
        "b",
        "e-ab",
        EdgeKind::Influences,
        "ev-ab",
    )
    .await;
    wire_edge(
        &store,
        &cache,
        &tx,
        "b",
        "c",
        "e-bc",
        EdgeKind::Influences,
        "ev-bc",
    )
    .await;

    let out = impact_radius(
        ImpactRadiusInput {
            node_id: NodeId("a".into()),
            max_depth: Some(1),
        },
        &store,
        &cache,
    )
    .await
    .unwrap();

    let ids: Vec<&NodeId> = out.affected.iter().map(|a| &a.node.id).collect();
    assert!(ids.contains(&&NodeId("b".into())));
    assert!(
        !ids.contains(&&NodeId("c".into())),
        "C must not appear at depth=1"
    );
}

// ── trace_decision ────────────────────────────────────────────────────────────

/// WHY: trace_decision must return the full provenance chain (caused_by, evidence, next_actions);
/// any missing piece silently hides decision rationale from the caller.
#[tokio::test]
async fn trace_decision_node_not_found() {
    let dir = TempDir::new().unwrap();
    let (store, cache, _tx) = open_store(&dir).await;

    let res = trace_decision(
        TraceDecisionInput {
            decision_node_id: NodeId("ghost".into()),
        },
        &store,
        &cache,
    )
    .await;
    assert!(matches!(res, Err(AxonMindError::NodeNotFound(_))));
}

#[tokio::test]
async fn trace_decision_returns_caused_by_and_next_actions() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("dec.layoff", NodeKind::Decision),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("insight.costs", NodeKind::Insight),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("action.reorg", NodeKind::Action),
        },
    )
    .await;

    // cause → decision
    wire_edge(
        &store,
        &cache,
        &tx,
        "insight.costs",
        "dec.layoff",
        "e-cause",
        EdgeKind::Causes,
        "ev-cause",
    )
    .await;
    // decision → next action
    wire_edge(
        &store,
        &cache,
        &tx,
        "dec.layoff",
        "action.reorg",
        "e-next",
        EdgeKind::NextAction,
        "ev-next",
    )
    .await;

    let out = trace_decision(
        TraceDecisionInput {
            decision_node_id: NodeId("dec.layoff".into()),
        },
        &store,
        &cache,
    )
    .await
    .unwrap();

    assert_eq!(out.decision.id, NodeId("dec.layoff".into()));
    assert_eq!(out.caused_by.len(), 1);
    assert_eq!(out.caused_by[0].from.id, NodeId("insight.costs".into()));
    assert_eq!(out.next_actions.len(), 1);
    assert_eq!(out.next_actions[0].to.id, NodeId("action.reorg".into()));
}

// ── suggest_actions ───────────────────────────────────────────────────────────

/// WHY: suggest_actions powers the "what should I do?" query; if it returns wrong node kinds
/// or ignores the review filter, users get irrelevant or unverified actions silently.
#[tokio::test]
async fn suggest_actions_returns_action_nodes_only() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("kpi.rev", NodeKind::Kpi),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("act.pricing", NodeKind::Action),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("risk.churn", NodeKind::Risk),
        },
    )
    .await;

    wire_edge(
        &store,
        &cache,
        &tx,
        "kpi.rev",
        "act.pricing",
        "e-act",
        EdgeKind::NextAction,
        "ev-act",
    )
    .await;
    wire_edge(
        &store,
        &cache,
        &tx,
        "kpi.rev",
        "risk.churn",
        "e-risk",
        EdgeKind::Impacts,
        "ev-risk",
    )
    .await;

    let out = suggest_actions(
        SuggestActionsInput {
            kpi_id: NodeId("kpi.rev".into()),
            status_filter: None,
            include_unreviewed: None,
        },
        &store,
        &cache,
    )
    .await
    .unwrap();

    assert_eq!(out.actions.len(), 1);
    assert_eq!(out.actions[0].id, NodeId("act.pricing".into()));
    assert_eq!(out.actions[0].kind, NodeKind::Action);
}

/// WHY: the default `include_unreviewed=false` is the safety gate preventing unvetted actions
/// from surfacing; if it defaults to true, users act on AI-generated content without human review.
#[tokio::test]
async fn suggest_actions_excludes_review_required_by_default() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("kpi.rev", NodeKind::Kpi),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("act.draft", NodeKind::Action),
        },
    )
    .await;

    // Edge requires human review
    let ev = make_evidence("ev-1", "kpi.rev");
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertEvidence {
            evidence: ev.clone(),
        },
    )
    .await;
    let mut e = make_edge("e1", "kpi.rev", "act.draft", EdgeKind::NextAction);
    e.requires_human_review = true;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertEdge {
            edge: e,
            evidence_ids: vec![ev.id],
        },
    )
    .await;

    let out = suggest_actions(
        SuggestActionsInput {
            kpi_id: NodeId("kpi.rev".into()),
            status_filter: None,
            include_unreviewed: None, // defaults to false
        },
        &store,
        &cache,
    )
    .await
    .unwrap();

    assert!(
        out.actions.is_empty(),
        "review-required action must be excluded by default"
    );
}

// ── get_evidence ──────────────────────────────────────────────────────────────

/// WHY: requiring exactly one of node_id/edge_id prevents callers from accidentally passing
/// neither and getting back empty results they interpret as "no evidence."
#[tokio::test]
async fn get_evidence_neither_id_returns_error() {
    let dir = TempDir::new().unwrap();
    let (store, _cache, _tx) = open_store(&dir).await;

    let res = get_evidence(
        GetEvidenceInput {
            node_id: None,
            edge_id: None,
        },
        &store,
    )
    .await;
    assert!(matches!(res, Err(AxonMindError::ValidationFailed { .. })));
}

#[tokio::test]
async fn get_evidence_by_node_id() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("doc.abc", NodeKind::Document),
        },
    )
    .await;
    let ev = make_evidence("ev-1", "doc.abc");
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertEvidence {
            evidence: ev.clone(),
        },
    )
    .await;

    let out = get_evidence(
        GetEvidenceInput {
            node_id: Some(NodeId("doc.abc".into())),
            edge_id: None,
        },
        &store,
    )
    .await
    .unwrap();

    assert_eq!(out.evidence.len(), 1);
    assert_eq!(out.evidence[0].id, ev.id);
}

#[tokio::test]
async fn get_evidence_by_edge_id() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("a", NodeKind::Initiative),
        },
    )
    .await;
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("b", NodeKind::Initiative),
        },
    )
    .await;
    let ev_id = wire_edge(
        &store,
        &cache,
        &tx,
        "a",
        "b",
        "e1",
        EdgeKind::Influences,
        "ev-1",
    )
    .await;

    let out = get_evidence(
        GetEvidenceInput {
            node_id: None,
            edge_id: Some(EdgeId("e1".into())),
        },
        &store,
    )
    .await
    .unwrap();

    assert_eq!(out.evidence.len(), 1);
    assert_eq!(out.evidence[0].id, ev_id);
}

// ── explain_kpi ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn explain_kpi_node_not_found() {
    let dir = TempDir::new().unwrap();
    let (store, cache, _tx) = open_store(&dir).await;

    let res = explain_kpi(
        ExplainKpiInput {
            kpi_id: NodeId("ghost".into()),
            depth: None,
        },
        &store,
        &cache,
        None,
    )
    .await;
    assert!(matches!(res, Err(AxonMindError::NodeNotFound(_))));
}

#[tokio::test]
async fn explain_kpi_wrong_kind_returns_not_a_kpi() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("team.eng", NodeKind::Team),
        },
    )
    .await;

    let res = explain_kpi(
        ExplainKpiInput {
            kpi_id: NodeId("team.eng".into()),
            depth: None,
        },
        &store,
        &cache,
        None,
    )
    .await;
    assert!(matches!(res, Err(AxonMindError::NotAKpi(_))));
}

/// WHY: if there is no evidence the confidence must be 0, not some default; a non-zero
/// confidence with no backing evidence would misrepresent a KPI as supported.
#[tokio::test]
async fn explain_kpi_no_evidence_gives_zero_confidence() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("kpi.rev", NodeKind::Kpi),
        },
    )
    .await;

    let out = explain_kpi(
        ExplainKpiInput {
            kpi_id: NodeId("kpi.rev".into()),
            depth: None,
        },
        &store,
        &cache,
        None,
    )
    .await
    .unwrap();

    assert_eq!(out.confidence, 0.0);
    assert!(
        out.rationale.contains("kpi.rev"),
        "rationale must include KPI name"
    );
}

/// WHY: evidence quotes are the primary content of the deterministic rationale; if they
/// don't appear, the explain output is useless for human review.
#[tokio::test]
async fn explain_kpi_rationale_includes_evidence_quotes() {
    let dir = TempDir::new().unwrap();
    let (store, cache, tx) = open_store(&dir).await;

    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertNode {
            node: make_node("kpi.rev", NodeKind::Kpi),
        },
    )
    .await;

    let mut ev = make_evidence("ev-1", "kpi.rev");
    ev.quote = Some("revenues increased by 42 percent last quarter".to_owned());
    apply(
        &store,
        &cache,
        &tx,
        GraphMutation::UpsertEvidence { evidence: ev },
    )
    .await;

    let out = explain_kpi(
        ExplainKpiInput {
            kpi_id: NodeId("kpi.rev".into()),
            depth: None,
        },
        &store,
        &cache,
        None,
    )
    .await
    .unwrap();

    assert!(
        out.rationale.contains("revenues increased by 42 percent"),
        "rationale must contain evidence quote; got: {}",
        out.rationale
    );
    assert!(
        out.confidence > 0.0,
        "confidence must be non-zero with evidence"
    );
}
