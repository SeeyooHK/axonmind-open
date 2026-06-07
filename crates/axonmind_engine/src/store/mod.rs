/// Blocker A: GraphMutation enum and transaction contract.
///
/// ALL writes to the graph — nodes, edges, evidence, candidates, metric values —
/// must go through `GraphStore::apply_mutation`. No code may write to SQLite directly.
///
/// Required transaction order for every mutation:
/// ```text
/// 1. Begin SQLite transaction
/// 2. Validate invariants (e.g. EvidenceMissing, NodeNotFound)
/// 3. Write to SQLite tables
/// 4. Update FTS5 search_index (see store/sqlite.rs for sync rules)
/// 5. Commit SQLite transaction
/// 6. Patch petgraph cache (GraphCache)
/// 7. Emit EngineEvent via broadcast::Sender
/// ```
///
/// If step 6 (cache patch) fails after step 5 (DB commit):
/// - Call `GraphCache::mark_dirty()`
/// - The engine rebuilds the cache before serving the next graph query
/// - Emit `EngineEvent::CacheRebuilt` after rebuild
///
/// This is the highest-risk module. Drift here breaks evidence invariants,
/// cache consistency, search index, and taint propagation simultaneously.
pub mod generations;
pub mod graph_cache;
pub mod migrations;
pub mod sqlite;

use axonmind_core::{
    AxonMindError, Confidence, Edge, EdgeId, EdgeKind, Evidence, EvidenceId, ExtractorKind, Node,
    NodeId, NodeKind, SourceType,
};
use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

pub use graph_cache::GraphCache;
pub use sqlite::GraphDb;

// ── Candidate types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CandidateId(pub String);

impl From<String> for CandidateId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateStatus {
    Pending,
    Approved,
    Rejected,
    Merged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KpiCandidate {
    pub id: CandidateId,
    pub name: String,
    pub definition: Option<String>,
    pub detected_in: Vec<NodeId>,
    pub confidence: axonmind_core::Confidence,
    pub proposed_at: DateTime<Utc>,
    pub status: CandidateStatus,
    pub merged_into: Option<NodeId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CandidateResolution {
    Approve,
    Reject,
    Merge { into: NodeId },
}

// ── MetricValue ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricValue {
    pub id: String,
    pub kpi_node_id: NodeId,
    pub metric_node_id: NodeId,
    pub value: f64,
    pub unit: String,
    pub period_start: Option<DateTime<Utc>>,
    pub period_end: Option<DateTime<Utc>>,
    /// Business reporting timestamp ("as-of"). Prefer for latest-value selection when present.
    pub as_of: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
    pub evidence_id: EvidenceId,
}

// ── GraphMutation ─────────────────────────────────────────────────────────────

/// The only way to modify graph state. Pass to `GraphStore::apply_mutation`.
///
/// Validation rules per variant:
/// - `UpsertEdge`: `evidence_ids` must be non-empty → `AxonMindError::EvidenceMissing`
/// - `UpsertEdge`: all `evidence_ids` must exist → `AxonMindError::EvidenceMissing`
/// - `UpsertNode`: if `kind == NodeKind::Kpi`, `attrs` must deserialize as `KpiAttrs`
/// - `DeleteNode`: cascades to edges and evidence via SQLite FK
/// - `ResolveKpiCandidate`: candidate must exist and be `CandidateStatus::Pending`
#[derive(Debug, Clone)]
pub enum GraphMutation {
    UpsertNode {
        node: Node,
    },
    UpsertEvidence {
        evidence: Evidence,
    },
    UpsertEdge {
        edge: Edge,
        evidence_ids: Vec<EvidenceId>,
    },
    DeleteNode {
        node_id: NodeId,
    },
    DeleteEdge {
        edge_id: EdgeId,
    },
    RecordMetricValue {
        value: MetricValue,
    },
    ProposeKpiCandidate {
        candidate: KpiCandidate,
    },
    ResolveKpiCandidate {
        candidate_id: CandidateId,
        resolution: CandidateResolution,
    },
}

/// One processed document with extraction counts, for the file-list UI. Returned by
/// `AxonMindEngine::list_documents`. `source_path`/`sha256` are `None` for documents ingested
/// without a backing file (e.g. raw markdown text).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocumentSummary {
    pub node_id: String,
    pub name: String,
    pub source_path: Option<String>,
    pub sha256: Option<String>,
    /// Unix seconds: when the document was last indexed (falls back to node creation time).
    pub indexed_at: i64,
    /// Concept nodes this document points at via `MentionedIn`.
    pub concept_count: usize,
    /// Evidence records sourced from this document.
    pub evidence_count: usize,
}

// ── GraphStore ────────────────────────────────────────────────────────────────

pub struct GraphStore {
    pub db: GraphDb,
}

impl GraphStore {
    pub async fn open(db_path: &std::path::Path) -> Result<Self, AxonMindError> {
        let db = GraphDb::new(db_path).await?;
        Ok(Self { db })
    }

    pub async fn apply_mutation(
        &self,
        mutation: GraphMutation,
        cache: &tokio::sync::RwLock<GraphCache>,
        event_tx: &tokio::sync::broadcast::Sender<crate::events::EngineEvent>,
    ) -> Result<(), AxonMindError> {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;

        // Steps 1–5: SQLite transaction + FTS5 sync inside interact
        let (graph_op, maybe_event) =
            conn
                .interact(
                    move |conn| -> Result<
                        (GraphOp, Option<crate::events::EngineEvent>),
                        AxonMindError,
                    > {
                        let tx = conn
                            .transaction()
                            .map_err(|e| AxonMindError::Database(e.to_string()))?;
                        let result = apply_in_tx(&tx, &mutation);
                        match result {
                            Ok(outcome) => {
                                tx.commit()
                                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                                Ok(outcome)
                            }
                            Err(e) => Err(e), // tx dropped → auto-rollback
                        }
                    },
                )
                .await
                .map_err(|e| AxonMindError::Database(format!("interact: {e}")))??;

        // Step 6: patch petgraph cache
        {
            let mut guard = cache.write().await;
            if let Err(e) = apply_graph_op(&mut guard, graph_op) {
                guard.mark_dirty();
                tracing::warn!("cache patch failed after commit, marked dirty: {e}");
            }
        }

        // Step 7: emit event
        if let Some(event) = maybe_event {
            let _ = event_tx.send(event);
        }

        Ok(())
    }

    /// Apply many mutations in a SINGLE SQLite transaction — all-or-nothing. If any mutation
    /// fails, the whole transaction rolls back, so the graph is never left half-modified.
    /// Cache patches and events are applied only after a successful commit. Use this for
    /// multi-step operations (document removal/regeneration) where a partial apply would corrupt
    /// the graph or lose data. Reuses the same per-mutation logic as `apply_mutation`.
    pub async fn apply_batch(
        &self,
        mutations: Vec<GraphMutation>,
        cache: &tokio::sync::RwLock<GraphCache>,
        event_tx: &tokio::sync::broadcast::Sender<crate::events::EngineEvent>,
    ) -> Result<(), AxonMindError> {
        if mutations.is_empty() {
            return Ok(());
        }
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;

        let outcomes: Vec<(GraphOp, Option<crate::events::EngineEvent>)> = conn
            .interact(
                move |conn| -> Result<
                    Vec<(GraphOp, Option<crate::events::EngineEvent>)>,
                    AxonMindError,
                > {
                    let tx = conn
                        .transaction()
                        .map_err(|e| AxonMindError::Database(e.to_string()))?;
                    let mut outs = Vec::with_capacity(mutations.len());
                    for m in &mutations {
                        outs.push(apply_in_tx(&tx, m)?); // first Err → tx dropped → auto-rollback
                    }
                    tx.commit()
                        .map_err(|e| AxonMindError::Database(e.to_string()))?;
                    Ok(outs)
                },
            )
            .await
            .map_err(|e| AxonMindError::Database(format!("interact: {e}")))??;

        // After commit: patch the cache and collect events, then emit.
        let mut events = Vec::new();
        {
            let mut guard = cache.write().await;
            for (op, ev) in outcomes {
                if let Err(e) = apply_graph_op(&mut guard, op) {
                    guard.mark_dirty();
                    tracing::warn!("cache patch failed after batch commit, marked dirty: {e}");
                }
                if let Some(e) = ev {
                    events.push(e);
                }
            }
        }
        for e in events {
            let _ = event_tx.send(e);
        }
        Ok(())
    }
}

// ── GraphOp (cache patch descriptor) ─────────────────────────────────────────

#[derive(Debug)]
enum GraphOp {
    AddNode(NodeId),
    RemoveNode(NodeId),
    AddEdge {
        edge_id: EdgeId,
        from: NodeId,
        to: NodeId,
        kind: EdgeKind,
    },
    RemoveEdge(EdgeId),
    None,
}

// ── Serialization helpers ─────────────────────────────────────────────────────

/// Serialize a serde unit-variant enum to its bare string name for SQLite TEXT storage.
/// e.g. EdgeKind::Influences → "Influences"
pub(crate) fn to_db_str<T: Serialize>(v: &T) -> Result<String, AxonMindError> {
    serde_json::to_string(v)
        .map(|s| s.trim_matches('"').to_owned())
        .map_err(|e| AxonMindError::Serialization(e.to_string()))
}

/// Deserialize a bare string from SQLite TEXT back into an enum.
pub(crate) fn from_db_str<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T, AxonMindError> {
    serde_json::from_value(serde_json::Value::String(s.to_owned()))
        .map_err(|e| AxonMindError::Serialization(e.to_string()))
}

// ── apply_in_tx — runs all per-variant logic inside a rusqlite Transaction ───

fn apply_in_tx(
    conn: &rusqlite::Connection,
    mutation: &GraphMutation,
) -> Result<(GraphOp, Option<crate::events::EngineEvent>), AxonMindError> {
    use crate::events::EngineEvent;

    match mutation {
        GraphMutation::UpsertNode { node } => {
            let attrs_bytes = rmp_serde::to_vec_named(&node.attrs)
                .map_err(|e| AxonMindError::Serialization(e.to_string()))?;
            let kind_str = to_db_str(&node.kind)?;

            conn.execute(
                "INSERT INTO nodes
                   (id, kind, name, attrs, confidence, is_tainted, requires_human_review, created_at, updated_at)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                   ON CONFLICT(id) DO UPDATE SET
                     kind=excluded.kind, name=excluded.name, attrs=excluded.attrs,
                     confidence=excluded.confidence, is_tainted=excluded.is_tainted,
                     requires_human_review=excluded.requires_human_review,
                     updated_at=excluded.updated_at",
                rusqlite::params![
                    node.id.0, kind_str, node.name, attrs_bytes,
                    node.confidence.0 as f64,
                    node.is_tainted as i32,
                    node.requires_human_review as i32,
                    node.created_at.timestamp(),
                    node.updated_at.timestamp(),
                ],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;

            sqlite::sync_node_fts(conn, &node.id.0)?;

            Ok((
                GraphOp::AddNode(node.id.clone()),
                Some(EngineEvent::NodeUpserted {
                    node_id: node.id.clone(),
                }),
            ))
        }

        GraphMutation::UpsertEvidence { evidence } => {
            let source_type_str = to_db_str(&evidence.source_type)?;
            let extractor_str = to_db_str(&evidence.extractor)?;

            conn.execute(
                "INSERT INTO evidence
                   (id, source_node_id, source_type, quote, row_ref, blob_sha256,
                    timestamp, extractor, confidence, is_tainted, requires_human_review)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
                   ON CONFLICT(id) DO UPDATE SET
                     source_node_id=excluded.source_node_id,
                     source_type=excluded.source_type,
                     quote=excluded.quote, row_ref=excluded.row_ref,
                     blob_sha256=excluded.blob_sha256, timestamp=excluded.timestamp,
                     extractor=excluded.extractor, confidence=excluded.confidence,
                     is_tainted=excluded.is_tainted,
                     requires_human_review=excluded.requires_human_review",
                rusqlite::params![
                    evidence.id.0,
                    evidence.source_node_id.0,
                    source_type_str,
                    evidence.quote,
                    evidence.row_ref,
                    evidence.blob_sha256,
                    evidence.timestamp.map(|t| t.timestamp()),
                    extractor_str,
                    evidence.confidence.0 as f64,
                    evidence.is_tainted as i32,
                    evidence.requires_human_review as i32,
                ],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;

            sqlite::sync_node_fts(conn, &evidence.source_node_id.0)?;

            // Recompute FTS for all edge-endpoint nodes linked to this evidence
            let affected: Vec<String> = {
                let mut stmt = conn
                    .prepare(
                        "SELECT DISTINCT n.id FROM nodes n
                     JOIN edges ed ON n.id = ed.from_id OR n.id = ed.to_id
                     JOIN edge_evidence ee ON ee.edge_id = ed.id
                     WHERE ee.evidence_id = ?1",
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt
                    .query_map([&evidence.id.0], |row| row.get(0))
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                x
            };
            for nid in &affected {
                sqlite::sync_node_fts(conn, nid)?;
            }

            Ok((
                GraphOp::None,
                Some(EngineEvent::EvidenceAdded {
                    evidence_id: evidence.id.clone(),
                }),
            ))
        }

        GraphMutation::UpsertEdge { edge, evidence_ids } => {
            if evidence_ids.is_empty() {
                return Err(AxonMindError::EvidenceMissing);
            }

            // Validate endpoint nodes exist
            for (node_id, col) in [(&edge.from, "from"), (&edge.to, "to")] {
                let exists = conn
                    .query_row("SELECT 1 FROM nodes WHERE id=?1", [&node_id.0], |_| Ok(()))
                    .optional()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .is_some();
                if !exists {
                    let _ = col; // suppress unused warning
                    return Err(AxonMindError::NodeNotFound(node_id.clone()));
                }
            }

            // Validate all evidence IDs exist
            for eid in evidence_ids {
                let exists = conn
                    .query_row("SELECT 1 FROM evidence WHERE id=?1", [&eid.0], |_| Ok(()))
                    .optional()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .is_some();
                if !exists {
                    return Err(AxonMindError::EvidenceMissing);
                }
            }

            let kind_str = to_db_str(&edge.kind)?;
            let created_by_str = to_db_str(&edge.created_by)?;

            conn.execute(
                "INSERT INTO edges
                   (id, from_id, to_id, kind, confidence, created_by,
                    is_tainted, requires_human_review, created_at)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                   ON CONFLICT(id) DO UPDATE SET
                     kind=excluded.kind, confidence=excluded.confidence,
                     is_tainted=excluded.is_tainted,
                     requires_human_review=excluded.requires_human_review",
                rusqlite::params![
                    edge.id.0,
                    edge.from.0,
                    edge.to.0,
                    kind_str,
                    edge.confidence.0 as f64,
                    created_by_str,
                    edge.is_tainted as i32,
                    edge.requires_human_review as i32,
                    edge.created_at.timestamp(),
                ],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;

            for eid in evidence_ids {
                conn.execute(
                    "INSERT OR IGNORE INTO edge_evidence (edge_id, evidence_id) VALUES (?1,?2)",
                    rusqlite::params![edge.id.0, eid.0],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            }

            sqlite::sync_node_fts(conn, &edge.from.0)?;
            sqlite::sync_node_fts(conn, &edge.to.0)?;

            Ok((
                GraphOp::AddEdge {
                    edge_id: edge.id.clone(),
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    kind: edge.kind,
                },
                Some(EngineEvent::EdgeUpserted {
                    edge_id: edge.id.clone(),
                }),
            ))
        }

        GraphMutation::DeleteNode { node_id } => {
            let exists = conn
                .query_row("SELECT 1 FROM nodes WHERE id=?1", [&node_id.0], |_| Ok(()))
                .optional()
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .is_some();
            if !exists {
                return Err(AxonMindError::NodeNotFound(node_id.clone()));
            }

            // FTS5 must be deleted manually before the node row (no FK cascade on virtual tables)
            conn.execute("DELETE FROM search_index WHERE node_id=?1", [&node_id.0])
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            // Delete the node, which cascades to evidence and edge_evidence, and edges attached directly
            conn.execute("DELETE FROM nodes WHERE id=?1", [&node_id.0])
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            // Clean up any edges that lost all their evidence due to the cascade
            let mut stmt = conn
                .prepare("SELECT id, from_id, to_id FROM edges WHERE NOT EXISTS (SELECT 1 FROM edge_evidence WHERE edge_id = edges.id)")
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let orphaned_edges: Vec<(String, String, String)> = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .filter_map(|r| r.ok())
                .collect();

            for (edge_id, from_id, to_id) in orphaned_edges {
                conn.execute("DELETE FROM edges WHERE id=?1", [&edge_id])
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                sqlite::sync_node_fts(conn, &from_id)?;
                sqlite::sync_node_fts(conn, &to_id)?;
            }

            Ok((
                GraphOp::RemoveNode(node_id.clone()),
                Some(EngineEvent::NodeDeleted {
                    node_id: node_id.clone(),
                }),
            ))
        }

        GraphMutation::DeleteEdge { edge_id } => {
            let endpoints: Option<(String, String)> = conn
                .query_row(
                    "SELECT from_id, to_id FROM edges WHERE id=?1",
                    [&edge_id.0],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            let (from_id, to_id) = endpoints.ok_or_else(|| AxonMindError::ValidationFailed {
                message: format!("edge not found: {}", edge_id.0),
            })?;

            conn.execute("DELETE FROM edges WHERE id=?1", [&edge_id.0])
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            sqlite::sync_node_fts(conn, &from_id)?;
            sqlite::sync_node_fts(conn, &to_id)?;

            Ok((
                GraphOp::RemoveEdge(edge_id.clone()),
                Some(EngineEvent::EdgeDeleted {
                    edge_id: edge_id.clone(),
                }),
            ))
        }

        GraphMutation::RecordMetricValue { value } => {
            conn.execute(
                "INSERT INTO metric_values
                   (id, kpi_node_id, metric_node_id, value, unit,
                    period_start, period_end, as_of, observed_at, evidence_id)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                rusqlite::params![
                    value.id,
                    value.kpi_node_id.0,
                    value.metric_node_id.0,
                    value.value,
                    value.unit,
                    value.period_start.map(|t| t.timestamp()),
                    value.period_end.map(|t| t.timestamp()),
                    value.as_of.map(|t| t.timestamp()),
                    value.observed_at.timestamp(),
                    value.evidence_id.0,
                ],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;

            Ok((GraphOp::None, None))
        }

        GraphMutation::ProposeKpiCandidate { candidate } => {
            let detected_in_json = serde_json::to_string(&candidate.detected_in)
                .map_err(|e| AxonMindError::Serialization(e.to_string()))?;
            let status_str = to_db_str(&candidate.status)?;

            conn.execute(
                "INSERT INTO kpi_candidates
                   (id, name, definition, detected_in, confidence, proposed_at, status, merged_into)
                   VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![
                    candidate.id.0,
                    candidate.name,
                    candidate.definition,
                    detected_in_json,
                    candidate.confidence.0 as f64,
                    candidate.proposed_at.timestamp(),
                    status_str,
                    candidate.merged_into.as_ref().map(|n| &n.0),
                ],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;

            Ok((
                GraphOp::None,
                Some(EngineEvent::KpiCandidateProposed {
                    candidate_id: candidate.id.clone(),
                }),
            ))
        }

        GraphMutation::ResolveKpiCandidate {
            candidate_id,
            resolution,
        } => {
            let status_str: Option<String> = conn
                .query_row(
                    "SELECT status FROM kpi_candidates WHERE id=?1",
                    [&candidate_id.0],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            let status_str = status_str.ok_or_else(|| AxonMindError::ValidationFailed {
                message: format!("candidate not found: {}", candidate_id.0),
            })?;

            let status: CandidateStatus = from_db_str(&status_str)?;
            if status != CandidateStatus::Pending {
                return Err(AxonMindError::ValidationFailed {
                    message: format!("candidate is not pending (status: {status_str})"),
                });
            }

            let new_status = match resolution {
                CandidateResolution::Approve => CandidateStatus::Approved,
                CandidateResolution::Reject => CandidateStatus::Rejected,
                CandidateResolution::Merge { .. } => CandidateStatus::Merged,
            };
            let merged_into = match resolution {
                CandidateResolution::Merge { into } => Some(into.0.clone()),
                _ => None,
            };

            conn.execute(
                "UPDATE kpi_candidates SET status=?1, merged_into=?2 WHERE id=?3",
                rusqlite::params![to_db_str(&new_status)?, merged_into, candidate_id.0],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;

            Ok((
                GraphOp::None,
                Some(EngineEvent::KpiCandidateResolved {
                    candidate_id: candidate_id.clone(),
                    status: new_status,
                }),
            ))
        }
    }
}

// ── apply_graph_op — patches petgraph cache after DB commit ──────────────────

// ── Row → struct converters (pub(crate) for query modules) ───────────────────

pub(crate) fn node_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
    let kind_str: String = row.get(1)?;
    let attrs_blob: Vec<u8> = row.get(3)?;
    let created_at_ts: i64 = row.get(7)?;
    let updated_at_ts: i64 = row.get(8)?;
    let kind: NodeKind =
        serde_json::from_value(serde_json::Value::String(kind_str)).unwrap_or(NodeKind::Document);
    let attrs: serde_json::Value =
        rmp_serde::from_slice(&attrs_blob).unwrap_or(serde_json::Value::Null);
    Ok(Node {
        id: NodeId(row.get(0)?),
        kind,
        name: row.get(2)?,
        attrs,
        confidence: Confidence(row.get::<_, f64>(4)? as f32),
        is_tainted: row.get(5)?,
        requires_human_review: row.get(6)?,
        created_at: chrono::DateTime::from_timestamp(created_at_ts, 0).unwrap_or_default(),
        updated_at: chrono::DateTime::from_timestamp(updated_at_ts, 0).unwrap_or_default(),
    })
}

pub(crate) fn evidence_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Evidence> {
    let source_type_str: String = row.get(2)?;
    let extractor_str: String = row.get(7)?;
    let ts: Option<i64> = row.get(6)?;
    let source_type: SourceType =
        serde_json::from_value(serde_json::Value::String(source_type_str))
            .unwrap_or(SourceType::Document);
    let extractor: ExtractorKind = serde_json::from_value(serde_json::Value::String(extractor_str))
        .unwrap_or(ExtractorKind::Rule);
    Ok(Evidence {
        id: EvidenceId(row.get(0)?),
        source_node_id: NodeId(row.get(1)?),
        source_type,
        quote: row.get(3)?,
        row_ref: row.get(4)?,
        blob_sha256: row.get(5)?,
        timestamp: ts.and_then(|t| chrono::DateTime::from_timestamp(t, 0)),
        extractor,
        confidence: Confidence(row.get::<_, f64>(8)? as f32),
        is_tainted: row.get(9)?,
        requires_human_review: row.get(10)?,
    })
}

// ── GraphStore read methods ───────────────────────────────────────────────────

impl GraphStore {
    pub(crate) async fn fetch_node(&self, node_id: &NodeId) -> Result<Option<Node>, AxonMindError> {
        let id = node_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| {
            conn.query_row(
                "SELECT id,kind,name,attrs,confidence,is_tainted,requires_human_review,
                        created_at,updated_at FROM nodes WHERE id=?1",
                [&id],
                node_from_row,
            )
            .optional()
            .map_err(|e| AxonMindError::Database(e.to_string()))
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    pub(crate) async fn fetch_nodes_by_ids(
        &self,
        ids: &[NodeId],
    ) -> Result<Vec<Node>, AxonMindError> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        let id_strs: Vec<String> = ids.iter().map(|n| n.0.clone()).collect();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<Vec<Node>, AxonMindError> {
            let placeholders: String = id_strs
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT id,kind,name,attrs,confidence,is_tainted,requires_human_review,
                        created_at,updated_at FROM nodes WHERE id IN ({placeholders})"
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let params: Vec<&dyn rusqlite::ToSql> =
                id_strs.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
            let x = stmt
                .query_map(params.as_slice(), node_from_row)
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(x)
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    pub(crate) async fn fetch_incoming_edges(
        &self,
        node_id: &NodeId,
    ) -> Result<Vec<Edge>, AxonMindError> {
        let id = node_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<Vec<Edge>, AxonMindError> {
            let edge_ids: Vec<String> = {
                let mut stmt = conn
                    .prepare("SELECT id FROM edges WHERE to_id=?1")
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt
                    .query_map([&id], |row| row.get(0))
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                x
            };
            edge_ids
                .iter()
                .filter_map(|eid| fetch_edge_inner(conn, eid).transpose())
                .collect()
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    pub(crate) async fn fetch_outgoing_edges(
        &self,
        node_id: &NodeId,
    ) -> Result<Vec<Edge>, AxonMindError> {
        let id = node_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<Vec<Edge>, AxonMindError> {
            let edge_ids: Vec<String> = {
                let mut stmt = conn
                    .prepare("SELECT id FROM edges WHERE from_id=?1")
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt
                    .query_map([&id], |row| row.get(0))
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                x
            };
            edge_ids
                .iter()
                .filter_map(|eid| fetch_edge_inner(conn, eid).transpose())
                .collect()
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    pub(crate) async fn fetch_evidence_for_node(
        &self,
        node_id: &NodeId,
    ) -> Result<Vec<Evidence>, AxonMindError> {
        let id = node_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<Vec<Evidence>, AxonMindError> {
            let mut stmt = conn
                .prepare(
                    "SELECT id,source_node_id,source_type,quote,row_ref,blob_sha256,
                        timestamp,extractor,confidence,is_tainted,requires_human_review
                 FROM evidence WHERE source_node_id=?1",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let x = stmt
                .query_map([&id], evidence_from_row)
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(x)
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    /// Fetch all evidence records backing every incoming edge to `node_id`, together with
    /// the kind of the edge they support.
    ///
    /// Used by `kpi_recompute` to partition supporting vs. contradicting evidence so that
    /// `Confidence::aggregate_signed` can be applied. This fixes the pre-existing issue where
    /// `fetch_evidence_for_node` returned empty for KPI nodes (KPIs are never evidence sources).
    pub(crate) async fn fetch_incoming_edge_evidence(
        &self,
        node_id: &NodeId,
    ) -> Result<Vec<(EdgeKind, Evidence)>, AxonMindError> {
        let id = node_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(
            move |conn| -> Result<Vec<(EdgeKind, Evidence)>, AxonMindError> {
                let mut stmt = conn
                    .prepare(
                        "SELECT ev.id, ev.source_node_id, ev.source_type, ev.quote, ev.row_ref,
                        ev.blob_sha256, ev.timestamp, ev.extractor, ev.confidence,
                        ev.is_tainted, ev.requires_human_review, e.kind
                 FROM edges e
                 JOIN edge_evidence ee ON e.id = ee.edge_id
                 JOIN evidence ev ON ee.evidence_id = ev.id
                 WHERE e.to_id = ?1",
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt
                    .query_map([&id], |row| {
                        let evidence = evidence_from_row(row)?;
                        let kind_str: String = row.get(11)?;
                        Ok((kind_str, evidence))
                    })
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;

                // Convert kind strings; skip rows whose kind cannot be deserialized (forward-compat).
                let result = x
                    .into_iter()
                    .filter_map(|(kind_str, ev)| {
                        from_db_str::<EdgeKind>(&kind_str).ok().map(|k| (k, ev))
                    })
                    .collect();
                Ok(result)
            },
        )
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    pub(crate) async fn fetch_evidence_for_edge(
        &self,
        edge_id: &EdgeId,
    ) -> Result<Vec<Evidence>, AxonMindError> {
        let id = edge_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<Vec<Evidence>, AxonMindError> {
            let mut stmt = conn
                .prepare(
                    "SELECT e.id,e.source_node_id,e.source_type,e.quote,e.row_ref,e.blob_sha256,
                        e.timestamp,e.extractor,e.confidence,e.is_tainted,e.requires_human_review
                 FROM evidence e
                 JOIN edge_evidence ee ON e.id=ee.evidence_id
                 WHERE ee.edge_id=?1",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let x = stmt
                .query_map([&id], evidence_from_row)
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(x)
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    pub(crate) async fn count_evidence_for_node(
        &self,
        node_id: &NodeId,
    ) -> Result<usize, AxonMindError> {
        let id = node_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<usize, AxonMindError> {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM evidence WHERE source_node_id=?1",
                    [&id],
                    |row| row.get(0),
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(n as usize)
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    /// Returns matched node IDs from FTS5. Caller fetches full Nodes if needed.
    pub(crate) async fn search_fts(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<NodeId>, AxonMindError> {
        let q = query.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<Vec<NodeId>, AxonMindError> {
            let mut stmt = conn
                .prepare("SELECT node_id FROM search_index WHERE search_index MATCH ?1 LIMIT ?2")
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let x = stmt
                .query_map(rusqlite::params![q, limit as i64], |row| {
                    row.get::<_, String>(0).map(NodeId)
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(x)
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    /// Fetch the stored DocFingerprint for `path_str`, if any.
    /// Returns `None` when the path is not cached (new file → FullReextract).
    pub(crate) async fn fetch_document_fingerprint(
        &self,
        path_str: &str,
    ) -> Result<Option<crate::extract::fingerprint::DocFingerprint>, AxonMindError> {
        let p = path_str.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn
            .interact(
                move |conn| -> Result<
                    Option<crate::extract::fingerprint::DocFingerprint>,
                    AxonMindError,
                > {
                    let row: Option<(String, Option<String>)> = conn
                        .query_row(
                            "SELECT sha256, structural_sha256 FROM document_cache WHERE path=?1",
                            [&p],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .optional()
                        .map_err(|e| AxonMindError::Database(e.to_string()))?;

                    Ok(row.and_then(|(content_sha256, structural_sha256)| {
                        // NULL structural_sha256 means migrated row not yet re-indexed → treat as no cache.
                        Some(crate::extract::fingerprint::DocFingerprint {
                            content_sha256,
                            structural_sha256: structural_sha256?,
                        })
                    }))
                },
            )
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    /// Insert or update document_cache entry after successful ingestion.
    pub(crate) async fn upsert_document_cache(
        &self,
        path_str: &str,
        content_sha256: &str,
        structural_sha256: &str,
        node_id: &NodeId,
    ) -> Result<(), AxonMindError> {
        let p = path_str.to_owned();
        let h = content_sha256.to_owned();
        let sh = structural_sha256.to_owned();
        let nid = node_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            let now = chrono::Utc::now().timestamp();
            conn.execute(
                "INSERT INTO document_cache (path, sha256, structural_sha256, indexed_at, node_id)
                 VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(path) DO UPDATE SET sha256=excluded.sha256,
                   structural_sha256=excluded.structural_sha256,
                   indexed_at=excluded.indexed_at, node_id=excluded.node_id",
                rusqlite::params![p, h, sh, now, nid],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    pub(crate) async fn fetch_nodes_by_kind(
        &self,
        kind: NodeKind,
    ) -> Result<Vec<Node>, AxonMindError> {
        let kind_str = to_db_str(&kind)?;
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<Node>, AxonMindError> {
            let mut stmt = conn
                .prepare(
                    "SELECT id,kind,name,attrs,confidence,is_tainted,requires_human_review,
                        created_at,updated_at FROM nodes WHERE kind=?1",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let x = stmt
                .query_map([&kind_str], node_from_row)
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(x)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Fetch up to `limit` existing concept nodes (all kinds except Document) as `(id, name)`,
    /// most-recently-updated first. Feeds two cross-document features: the LLM entity extractor's
    /// "avoid duplicating" name hint, and the deterministic near-duplicate bridge.
    pub(crate) async fn fetch_concept_node_id_names(
        &self,
        limit: usize,
    ) -> Result<Vec<(NodeId, String)>, AxonMindError> {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(
            move |conn| -> Result<Vec<(NodeId, String)>, AxonMindError> {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, name FROM nodes WHERE kind != 'Document'
                 ORDER BY updated_at DESC LIMIT ?1",
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt
                    .query_map([limit as i64], |row| {
                        Ok((NodeId(row.get::<_, String>(0)?), row.get::<_, String>(1)?))
                    })
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                Ok(x)
            },
        )
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Concept node IDs this document points at via `MentionedIn` edges. Captured before a
    /// document is deleted so the caller can sweep any that become orphaned afterward.
    pub(crate) async fn fetch_mentioned_node_ids(
        &self,
        doc_node_id: &NodeId,
    ) -> Result<Vec<NodeId>, AxonMindError> {
        let id = doc_node_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<NodeId>, AxonMindError> {
            // DISTINCT: a document may have several MentionedIn edges to the same concept
            // (e.g. rule + LLM extraction); the caller must not try to delete it twice.
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT to_id FROM edges WHERE from_id = ?1 AND kind = 'MentionedIn'",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let x = stmt
                .query_map([&id], |row| Ok(NodeId(row.get::<_, String>(0)?)))
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(x)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// How many documents still reference a blob `sha256` (via `document_cache`). Used to decide
    /// whether a blob is safe to delete on document removal (don't delete a shared blob).
    pub(crate) async fn count_documents_with_sha(
        &self,
        sha256: &str,
    ) -> Result<usize, AxonMindError> {
        let sha = sha256.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<usize, AxonMindError> {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM document_cache WHERE sha256 = ?1",
                    [&sha],
                    |row| row.get(0),
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(n as usize)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// One row per processed document, with extraction counts, for the file-list UI.
    pub(crate) async fn list_document_summaries(
        &self,
    ) -> Result<Vec<DocumentSummary>, AxonMindError> {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<DocumentSummary>, AxonMindError> {
            let mut stmt = conn.prepare(
                "SELECT n.id, n.name, dc.path, dc.sha256, \
                        COALESCE(dc.indexed_at, n.created_at) AS indexed_at, \
                        (SELECT COUNT(*) FROM edges e WHERE e.from_id = n.id AND e.kind = 'MentionedIn') AS concept_count, \
                        (SELECT COUNT(*) FROM evidence ev WHERE ev.source_node_id = n.id) AS evidence_count \
                 FROM nodes n \
                 LEFT JOIN document_cache dc ON dc.node_id = n.id \
                 WHERE n.kind = 'Document' \
                 ORDER BY indexed_at DESC",
            ).map_err(|e| AxonMindError::Database(e.to_string()))?;
            let x = stmt.query_map([], |row| {
                Ok(DocumentSummary {
                    node_id: row.get(0)?,
                    name: row.get(1)?,
                    source_path: row.get(2)?,
                    sha256: row.get(3)?,
                    indexed_at: row.get(4)?,
                    concept_count: row.get::<_, i64>(5)? as usize,
                    evidence_count: row.get::<_, i64>(6)? as usize,
                })
            })
            .map_err(|e| AxonMindError::Database(e.to_string()))?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(x)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Count distinct Document nodes that have a MentionedIn edge pointing to `node_id`.
    pub(crate) async fn count_source_documents_for_node(
        &self,
        node_id: &NodeId,
    ) -> Result<usize, AxonMindError> {
        let id = node_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<usize, AxonMindError> {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(DISTINCT e.from_id)
                 FROM edges e
                 JOIN nodes n ON n.id = e.from_id
                 WHERE e.to_id = ?1 AND e.kind = 'MentionedIn' AND n.kind = 'Document'",
                    [&id],
                    |row| row.get(0),
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(n as usize)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Returns true if any non-rejected candidate exists with this exact name.
    pub(crate) async fn check_candidate_exists_by_name(
        &self,
        name: &str,
    ) -> Result<bool, AxonMindError> {
        let name = name.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<bool, AxonMindError> {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM kpi_candidates WHERE name=?1 AND status != 'rejected'",
                    [&name],
                    |row| row.get(0),
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(n > 0)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Fetch the most recent `limit` metric values for a KPI, newest first.
    pub(crate) async fn fetch_latest_metric_values(
        &self,
        kpi_node_id: &NodeId,
        limit: usize,
    ) -> Result<Vec<MetricValue>, AxonMindError> {
        let id = kpi_node_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<MetricValue>, AxonMindError> {
            let mut stmt = conn
                .prepare(
                    "SELECT id,kpi_node_id,metric_node_id,value,unit,
                        period_start,period_end,as_of,observed_at,evidence_id
                 FROM metric_values WHERE kpi_node_id=?1
                 ORDER BY COALESCE(as_of, observed_at) DESC, observed_at DESC LIMIT ?2",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let x = stmt
                .query_map(rusqlite::params![id, limit as i64], |r| {
                    let ps: Option<i64> = r.get(5)?;
                    let pe: Option<i64> = r.get(6)?;
                    let ao: Option<i64> = r.get(7)?;
                    let oa: i64 = r.get(8)?;
                    Ok(MetricValue {
                        id: r.get(0)?,
                        kpi_node_id: NodeId(r.get(1)?),
                        metric_node_id: NodeId(r.get(2)?),
                        value: r.get(3)?,
                        unit: r.get(4)?,
                        period_start: ps.and_then(|t| chrono::DateTime::from_timestamp(t, 0)),
                        period_end: pe.and_then(|t| chrono::DateTime::from_timestamp(t, 0)),
                        as_of: ao.and_then(|t| chrono::DateTime::from_timestamp(t, 0)),
                        observed_at: chrono::DateTime::from_timestamp(oa, 0).unwrap_or_default(),
                        evidence_id: EvidenceId(r.get(9)?),
                    })
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(x)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Bulk-fetch everything needed for `export_json`. Single pool checkout.
    pub(crate) async fn fetch_export(
        &self,
    ) -> Result<
        (
            Vec<Node>,
            Vec<Edge>,
            Vec<Evidence>,
            Vec<(EdgeId, EvidenceId)>,
            Vec<MetricValue>,
            Vec<KpiCandidate>,
        ),
        AxonMindError,
    > {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(|conn| -> Result<_, AxonMindError> {
            // Nodes — ORDER BY id for stable, diff-friendly export output.
            let nodes: Vec<Node> = {
                let mut stmt = conn.prepare(
                    "SELECT id,kind,name,attrs,confidence,is_tainted,requires_human_review,
                            created_at,updated_at FROM nodes ORDER BY id"
                ).map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt.query_map([], node_from_row)
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                x
            };

            // Edge evidence map (edge_id → evidence_ids) — ORDER BY for deterministic export.
            let edge_evidence_pairs: Vec<(EdgeId, EvidenceId)> = {
                let mut stmt = conn.prepare(
                    "SELECT edge_id, evidence_id FROM edge_evidence ORDER BY edge_id, evidence_id"
                ).map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt.query_map([], |r| Ok((
                    r.get::<_, String>(0).map(EdgeId)?,
                    r.get::<_, String>(1).map(EvidenceId)?,
                )))
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
                x
            };

            // Build evidence_ids per edge for Edge structs
            use std::collections::HashMap;
            let mut ev_by_edge: HashMap<String, Vec<EvidenceId>> = HashMap::new();
            for (eid, evid) in &edge_evidence_pairs {
                ev_by_edge.entry(eid.0.clone()).or_default().push(evid.clone());
            }

            // Edges — ORDER BY id for deterministic export.
            let edges: Vec<Edge> = {
                let mut stmt = conn.prepare(
                    "SELECT id,from_id,to_id,kind,confidence,created_by,
                            is_tainted,requires_human_review,created_at FROM edges ORDER BY id"
                ).map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, f64>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, bool>(6)?,
                        r.get::<_, bool>(7)?,
                        r.get::<_, i64>(8)?,
                    ))
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
                x.into_iter().map(|(eid, from, to, kind_str, conf, by_str, tainted, review, ts)| {
                    let evidence = ev_by_edge.get(&eid).cloned().unwrap_or_default();
                    Ok(Edge {
                        id: EdgeId(eid),
                        from: NodeId(from),
                        to: NodeId(to),
                        kind: from_db_str(&kind_str)?,
                        confidence: Confidence(conf as f32),
                        created_at: chrono::DateTime::from_timestamp(ts, 0).unwrap_or_default(),
                        created_by: from_db_str(&by_str)?,
                        evidence,
                        is_tainted: tainted,
                        requires_human_review: review,
                    })
                }).collect::<Result<Vec<_>, AxonMindError>>()?
            };

            // Evidence — ORDER BY id for deterministic export.
            let evidence: Vec<Evidence> = {
                let mut stmt = conn.prepare(
                    "SELECT id,source_node_id,source_type,quote,row_ref,blob_sha256,
                            timestamp,extractor,confidence,is_tainted,requires_human_review
                     FROM evidence ORDER BY id"
                ).map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt.query_map([], evidence_from_row)
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                x
            };

            // MetricValues — ORDER BY id for deterministic export.
            let metric_values: Vec<MetricValue> = {
                let mut stmt = conn.prepare(
                    "SELECT id,kpi_node_id,metric_node_id,value,unit,
                            period_start,period_end,as_of,observed_at,evidence_id FROM metric_values ORDER BY id"
                ).map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt.query_map([], |r| {
                    Ok(MetricValue {
                        id: r.get(0)?,
                        kpi_node_id: NodeId(r.get(1)?),
                        metric_node_id: NodeId(r.get(2)?),
                        value: r.get(3)?,
                        unit: r.get(4)?,
                        period_start: r.get::<_, Option<i64>>(5)?.map(|t| chrono::DateTime::from_timestamp(t, 0).unwrap_or_default()),
                        period_end: r.get::<_, Option<i64>>(6)?.map(|t| chrono::DateTime::from_timestamp(t, 0).unwrap_or_default()),
                        as_of: r.get::<_, Option<i64>>(7)?.map(|t| chrono::DateTime::from_timestamp(t, 0).unwrap_or_default()),
                        observed_at: chrono::DateTime::from_timestamp(r.get::<_, i64>(8)?, 0).unwrap_or_default(),
                        evidence_id: EvidenceId(r.get(9)?),
                    })
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
                x
            };

            // KpiCandidates — ORDER BY id for deterministic export.
            let kpi_candidates: Vec<KpiCandidate> = {
                let mut stmt = conn.prepare(
                    "SELECT id,name,definition,detected_in,confidence,proposed_at,status,merged_into
                     FROM kpi_candidates ORDER BY id"
                ).map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt.query_map([], |r| {
                    let detected_json: String = r.get(3)?;
                    let detected_in: Vec<NodeId> = serde_json::from_str(&detected_json)
                        .unwrap_or_default();
                    Ok(KpiCandidate {
                        id: CandidateId(r.get(0)?),
                        name: r.get(1)?,
                        definition: r.get(2)?,
                        detected_in,
                        confidence: Confidence(r.get::<_, f64>(4)? as f32),
                        proposed_at: chrono::DateTime::from_timestamp(r.get::<_, i64>(5)?, 0).unwrap_or_default(),
                        status: from_db_str(&r.get::<_, String>(6)?)
                            .unwrap_or(CandidateStatus::Pending),
                        merged_into: r.get::<_, Option<String>>(7)?.map(NodeId),
                    })
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
                x
            };

            Ok((nodes, edges, evidence, edge_evidence_pairs, metric_values, kpi_candidates))
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }
}

fn fetch_edge_inner(conn: &rusqlite::Connection, id: &str) -> Result<Option<Edge>, AxonMindError> {
    use rusqlite::OptionalExtension;
    let row: Option<(String, String, String, String, f64, String, bool, bool, i64)> = conn
        .query_row(
            "SELECT id,from_id,to_id,kind,confidence,created_by,
                    is_tainted,requires_human_review,created_at
             FROM edges WHERE id=?1",
            [id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                ))
            },
        )
        .optional()
        .map_err(|e| AxonMindError::Database(e.to_string()))?;

    let (eid, from, to, kind_str, conf, by_str, tainted, review, created_ts) = match row {
        None => return Ok(None),
        Some(r) => r,
    };

    let evidence_ids: Vec<EvidenceId> = {
        let mut stmt = conn
            .prepare("SELECT evidence_id FROM edge_evidence WHERE edge_id=?1")
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        let x = stmt
            .query_map([&eid], |r| r.get::<_, String>(0).map(EvidenceId))
            .map_err(|e| AxonMindError::Database(e.to_string()))?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        x
    };

    Ok(Some(Edge {
        id: EdgeId(eid),
        from: NodeId(from),
        to: NodeId(to),
        kind: from_db_str(&kind_str)?,
        confidence: Confidence(conf as f32),
        created_at: chrono::DateTime::from_timestamp(created_ts, 0).unwrap_or_default(),
        created_by: from_db_str(&by_str)?,
        evidence: evidence_ids,
        is_tainted: tainted,
        requires_human_review: review,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::sync::{RwLock, broadcast};

    // ── helpers ──────────────────────────────────────────────────────────────────

    async fn open_store(
        dir: &TempDir,
    ) -> (
        GraphStore,
        RwLock<GraphCache>,
        broadcast::Sender<crate::events::EngineEvent>,
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
        tx: &broadcast::Sender<crate::events::EngineEvent>,
        m: GraphMutation,
    ) -> Result<(), AxonMindError> {
        store.apply_mutation(m, cache, tx).await
    }

    fn node(id: &str) -> Node {
        let now = chrono::Utc::now();
        Node {
            id: NodeId(id.to_owned()),
            kind: NodeKind::Team,
            name: format!("Node {id}"),
            attrs: serde_json::Value::Null,
            confidence: Confidence(0.8),
            is_tainted: false,
            requires_human_review: false,
            created_at: now,
            updated_at: now,
        }
    }

    fn evidence(id: &str, source: &str) -> Evidence {
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

    fn edge(id: &str, from: &str, to: &str) -> Edge {
        Edge {
            id: EdgeId(id.to_owned()),
            from: NodeId(from.to_owned()),
            to: NodeId(to.to_owned()),
            kind: EdgeKind::Influences,
            evidence: vec![],
            confidence: Confidence(0.75),
            created_at: chrono::Utc::now(),
            created_by: ExtractorKind::Rule,
            is_tainted: false,
            requires_human_review: false,
        }
    }

    /// WHY: a document can hold multiple MentionedIn edges to the same concept (rule + LLM both
    /// emit one). If `fetch_mentioned_node_ids` returned duplicates, `remove_document` would try
    /// to delete that concept twice and fail with NodeNotFound — the bug behind "node not found"
    /// on Regenerate. The query must collapse them to one id.
    #[tokio::test]
    async fn fetch_mentioned_node_ids_dedupes_duplicate_edges() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode {
                node: Node {
                    kind: NodeKind::Document,
                    ..node("doc.x")
                },
            },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode {
                node: node("kpi.dr"),
            },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEvidence {
                evidence: evidence("ev1", "doc.x"),
            },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEvidence {
                evidence: evidence("ev2", "doc.x"),
            },
        )
        .await
        .unwrap();

        let mentioned_edge = |id: &str, ev: &str| Edge {
            kind: EdgeKind::MentionedIn,
            evidence: vec![EvidenceId(ev.to_owned())],
            ..edge(id, "doc.x", "kpi.dr")
        };
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEdge {
                edge: mentioned_edge("e1", "ev1"),
                evidence_ids: vec![EvidenceId("ev1".to_owned())],
            },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEdge {
                edge: mentioned_edge("e2", "ev2"),
                evidence_ids: vec![EvidenceId("ev2".to_owned())],
            },
        )
        .await
        .unwrap();

        let mentioned = store
            .fetch_mentioned_node_ids(&NodeId("doc.x".to_owned()))
            .await
            .unwrap();
        assert_eq!(
            mentioned,
            vec![NodeId("kpi.dr".to_owned())],
            "duplicate MentionedIn edges must collapse to one id"
        );
    }

    // ── UpsertNode ────────────────────────────────────────────────────────────────

    /// WHY: the only write path for nodes; if fields don't survive the roundtrip the graph is
    /// silently wrong with no detectable signal at query time.
    #[tokio::test]
    async fn upsert_node_roundtrip() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let n = node("team.eng");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: n.clone() },
        )
        .await
        .unwrap();

        let fetched = store
            .fetch_node(&n.id)
            .await
            .unwrap()
            .expect("node missing after upsert");
        assert_eq!(fetched.id, n.id);
        assert_eq!(fetched.name, n.name);
        assert_eq!(fetched.kind, n.kind);
    }

    /// WHY: upsert semantics must overwrite — a silent no-op on conflict would leave stale
    /// node metadata in the graph indefinitely.
    #[tokio::test]
    async fn upsert_node_update_overwrites_fields() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let mut n = node("team.eng");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: n.clone() },
        )
        .await
        .unwrap();

        n.name = "Updated Engineering".to_owned();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: n.clone() },
        )
        .await
        .unwrap();

        let fetched = store.fetch_node(&n.id).await.unwrap().unwrap();
        assert_eq!(fetched.name, "Updated Engineering");
    }

    /// WHY: FTS must be synced on every node write; if not, search results drift from the graph
    /// and search_fts returns IDs that fetch_node then returns None for.
    #[tokio::test]
    async fn upsert_node_populates_fts() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let mut n = node("team.eng");
        n.name = "AlphaOmegaTeamXYZ".to_owned();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: n.clone() },
        )
        .await
        .unwrap();

        let hits = store.search_fts("AlphaOmegaTeamXYZ", 10).await.unwrap();
        assert!(
            hits.contains(&n.id),
            "FTS did not index node name after upsert"
        );
    }

    /// WHY: graph traversal queries (impact_radius, focus_kpi) use the petgraph cache; a node
    /// absent from the cache after write is invisible to all traversal until cache rebuild.
    #[tokio::test]
    async fn upsert_node_patches_cache() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let n = node("team.eng");
        assert!(!cache.read().await.node_indices.contains_key(&n.id));
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: n.clone() },
        )
        .await
        .unwrap();
        assert!(cache.read().await.node_indices.contains_key(&n.id));
    }

    // ── DeleteNode ────────────────────────────────────────────────────────────────

    /// WHY: callers must know whether the delete had any effect; a silent success on a missing
    /// node masks bugs where the caller used the wrong ID.
    #[tokio::test]
    async fn delete_node_not_found() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let res = apply(
            &store,
            &cache,
            &tx,
            GraphMutation::DeleteNode {
                node_id: NodeId("ghost".into()),
            },
        )
        .await;
        assert!(matches!(res, Err(AxonMindError::NodeNotFound(_))));
    }

    /// WHY: FTS is a virtual table with no FK cascade; if not manually deleted, search_fts
    /// returns the dead node ID and all callers that trust those results silently corrupt output.
    #[tokio::test]
    async fn delete_node_clears_fts() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let mut n = node("team.eng");
        n.name = "DeleteMeNode77".to_owned();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: n.clone() },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::DeleteNode {
                node_id: n.id.clone(),
            },
        )
        .await
        .unwrap();

        let hits = store.search_fts("DeleteMeNode77", 10).await.unwrap();
        assert!(!hits.contains(&n.id), "FTS still contains deleted node");
    }

    /// WHY: a phantom node in the cache after delete creates ghost paths in petgraph traversal;
    /// impact_radius would traverse edges to a node that no longer exists in SQLite.
    #[tokio::test]
    async fn delete_node_removes_from_cache() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let n = node("team.eng");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: n.clone() },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::DeleteNode {
                node_id: n.id.clone(),
            },
        )
        .await
        .unwrap();
        assert!(!cache.read().await.node_indices.contains_key(&n.id));
    }

    // ── UpsertEvidence ────────────────────────────────────────────────────────────

    /// WHY: evidence is the provenance record for every edge; if fields don't roundtrip correctly,
    /// the audit trail is silently corrupted and focus_kpi returns wrong confidence scores.
    #[tokio::test]
    async fn upsert_evidence_roundtrip() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let n = node("doc.abc");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: n.clone() },
        )
        .await
        .unwrap();

        let ev = evidence("ev-1", "doc.abc");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEvidence {
                evidence: ev.clone(),
            },
        )
        .await
        .unwrap();

        let fetched = store.fetch_evidence_for_node(&n.id).await.unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].id, ev.id);
        assert_eq!(fetched[0].quote, ev.quote);
    }

    // ── UpsertEdge ────────────────────────────────────────────────────────────────

    /// WHY: this is THE critical invariant — every edge must have ≥1 evidence reference.
    /// Allowing empty evidence silently decouples graph structure from provenance, making
    /// confidence scores meaningless and the trust model unenforceable.
    #[tokio::test]
    async fn upsert_edge_empty_evidence_rejected() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: node("a") },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: node("b") },
        )
        .await
        .unwrap();

        let res = apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEdge {
                edge: edge("e1", "a", "b"),
                evidence_ids: vec![],
            },
        )
        .await;
        assert!(matches!(res, Err(AxonMindError::EvidenceMissing)));
    }

    /// WHY: referencing a non-existent evidence ID creates a dangling pointer; edge_evidence JOINs
    /// in fetch_evidence_for_edge silently return zero rows, hiding the missing provenance.
    #[tokio::test]
    async fn upsert_edge_missing_evidence_rejected() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: node("a") },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: node("b") },
        )
        .await
        .unwrap();

        let res = apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEdge {
                edge: edge("e1", "a", "b"),
                evidence_ids: vec![EvidenceId("nonexistent".into())],
            },
        )
        .await;
        assert!(matches!(res, Err(AxonMindError::EvidenceMissing)));
    }

    /// WHY: an edge to a non-existent endpoint creates an orphaned graph reference;
    /// traversal skips it silently, making impact_radius incomplete with no error signal.
    #[tokio::test]
    async fn upsert_edge_missing_endpoint_rejected() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: node("a") },
        )
        .await
        .unwrap();
        let ev = evidence("ev-1", "a");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEvidence { evidence: ev },
        )
        .await
        .unwrap();

        let res = apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEdge {
                edge: edge("e1", "a", "ghost"),
                evidence_ids: vec![EvidenceId("ev-1".into())],
            },
        )
        .await;
        assert!(matches!(res, Err(AxonMindError::NodeNotFound(_))));
    }

    /// WHY: edge rows and the edge_evidence junction must persist together; fetch_outgoing_edges
    /// is the backbone of impact_radius and focus_kpi — a missing edge or junction row means
    /// those queries return wrong results with no visible error.
    #[tokio::test]
    async fn upsert_edge_roundtrip() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let from = node("a");
        let to = node("b");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: from.clone() },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: to.clone() },
        )
        .await
        .unwrap();
        let ev = evidence("ev-1", "a");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEvidence {
                evidence: ev.clone(),
            },
        )
        .await
        .unwrap();

        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEdge {
                edge: edge("e1", "a", "b"),
                evidence_ids: vec![ev.id.clone()],
            },
        )
        .await
        .unwrap();

        let out = store.fetch_outgoing_edges(&from.id).await.unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, EdgeId("e1".into()));
        assert!(out[0].evidence.contains(&ev.id));

        let inc = store.fetch_incoming_edges(&to.id).await.unwrap();
        assert_eq!(inc.len(), 1);
        assert_eq!(inc[0].id, EdgeId("e1".into()));
    }

    /// WHY: edge endpoint FTS must include evidence quotes so that searching for quoted text
    /// surfaces the nodes connected by evidence; without this, full-graph search is incomplete.
    #[tokio::test]
    async fn upsert_edge_syncs_endpoint_fts_with_evidence_quotes() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let from = node("a");
        let to = node("b");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: from.clone() },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: to.clone() },
        )
        .await
        .unwrap();

        let mut ev = evidence("ev-1", "a");
        ev.quote = Some("ZetaUniqueQuote42".to_owned());
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEvidence {
                evidence: ev.clone(),
            },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEdge {
                edge: edge("e1", "a", "b"),
                evidence_ids: vec![ev.id],
            },
        )
        .await
        .unwrap();

        let hits = store.search_fts("ZetaUniqueQuote42", 10).await.unwrap();
        assert!(
            hits.contains(&from.id),
            "from-node missing from FTS after edge upsert"
        );
        assert!(
            hits.contains(&to.id),
            "to-node missing from FTS after edge upsert"
        );
    }

    /// WHY: same reasoning as upsert_node_patches_cache but for edges; a missing edge in the
    /// cache makes it invisible to petgraph traversal until the next cache rebuild.
    #[tokio::test]
    async fn upsert_edge_patches_cache() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: node("a") },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: node("b") },
        )
        .await
        .unwrap();
        let ev = evidence("ev-1", "a");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEvidence {
                evidence: ev.clone(),
            },
        )
        .await
        .unwrap();

        let eid = EdgeId("e1".into());
        assert!(!cache.read().await.edge_indices.contains_key(&eid));
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEdge {
                edge: edge("e1", "a", "b"),
                evidence_ids: vec![ev.id],
            },
        )
        .await
        .unwrap();
        assert!(cache.read().await.edge_indices.contains_key(&eid));
    }

    // ── DeleteEdge ────────────────────────────────────────────────────────────────

    /// WHY: a stale edge in SQLite causes phantom paths in impact_radius and false provenance
    /// chains in focus_kpi — both silent, no error returned to the caller.
    #[tokio::test]
    async fn delete_edge_roundtrip() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let from = node("a");
        let to = node("b");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: from.clone() },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: to.clone() },
        )
        .await
        .unwrap();
        let ev = evidence("ev-1", "a");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEvidence {
                evidence: ev.clone(),
            },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEdge {
                edge: edge("e1", "a", "b"),
                evidence_ids: vec![ev.id],
            },
        )
        .await
        .unwrap();

        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::DeleteEdge {
                edge_id: EdgeId("e1".into()),
            },
        )
        .await
        .unwrap();

        assert!(
            store
                .fetch_outgoing_edges(&from.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !cache
                .read()
                .await
                .edge_indices
                .contains_key(&EdgeId("e1".into()))
        );
    }

    #[tokio::test]
    async fn delete_edge_not_found() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let res = apply(
            &store,
            &cache,
            &tx,
            GraphMutation::DeleteEdge {
                edge_id: EdgeId("ghost".into()),
            },
        )
        .await;
        assert!(matches!(res, Err(AxonMindError::ValidationFailed { .. })));
    }

    // ── KPI candidates ────────────────────────────────────────────────────────────

    /// WHY: the KPI candidate state machine (propose → resolve) must enforce single-resolution;
    /// double-resolution or resolving a ghost candidate corrupts the human-review queue.
    #[tokio::test]
    async fn propose_and_resolve_kpi_candidate() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let candidate = KpiCandidate {
            id: CandidateId("cand-1".into()),
            name: "Churn Rate".to_owned(),
            definition: Some("Monthly churned / total customers".to_owned()),
            detected_in: vec![],
            confidence: Confidence(0.7),
            proposed_at: chrono::Utc::now(),
            status: CandidateStatus::Pending,
            merged_into: None,
        };
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::ProposeKpiCandidate { candidate },
        )
        .await
        .unwrap();

        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::ResolveKpiCandidate {
                candidate_id: CandidateId("cand-1".into()),
                resolution: CandidateResolution::Approve,
            },
        )
        .await
        .unwrap();

        // Second resolution must fail — candidate is no longer Pending
        let res = apply(
            &store,
            &cache,
            &tx,
            GraphMutation::ResolveKpiCandidate {
                candidate_id: CandidateId("cand-1".into()),
                resolution: CandidateResolution::Reject,
            },
        )
        .await;
        assert!(matches!(res, Err(AxonMindError::ValidationFailed { .. })));
    }

    #[tokio::test]
    async fn resolve_nonexistent_candidate_fails() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let res = apply(
            &store,
            &cache,
            &tx,
            GraphMutation::ResolveKpiCandidate {
                candidate_id: CandidateId("ghost".into()),
                resolution: CandidateResolution::Reject,
            },
        )
        .await;
        assert!(matches!(res, Err(AxonMindError::ValidationFailed { .. })));
    }

    // ── MetricValue ───────────────────────────────────────────────────────────────

    /// WHY: metric_values are the time-series input for kpi_recompute; if they don't persist
    /// correctly, trend computation silently returns wrong confidence scores with no error.
    #[tokio::test]
    async fn record_metric_value_roundtrip() {
        let dir = TempDir::new().unwrap();
        let (store, cache, tx) = open_store(&dir).await;

        let kpi = node("kpi.rev");
        let metric = node("metric.arr");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode { node: kpi.clone() },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode {
                node: metric.clone(),
            },
        )
        .await
        .unwrap();
        let ev = evidence("ev-1", "kpi.rev");
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertEvidence {
                evidence: ev.clone(),
            },
        )
        .await
        .unwrap();

        let mv = MetricValue {
            id: uuid::Uuid::new_v4().to_string(),
            kpi_node_id: kpi.id.clone(),
            metric_node_id: metric.id.clone(),
            value: 1_234_567.0,
            unit: "USD".to_owned(),
            period_start: None,
            period_end: None,
            as_of: None,
            observed_at: chrono::Utc::now(),
            evidence_id: ev.id,
        };
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::RecordMetricValue { value: mv },
        )
        .await
        .unwrap();

        let fetched = store.fetch_latest_metric_values(&kpi.id, 5).await.unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].value, 1_234_567.0);
        assert_eq!(fetched[0].unit, "USD");
    }
}

fn apply_graph_op(cache: &mut GraphCache, op: GraphOp) -> Result<(), AxonMindError> {
    match op {
        GraphOp::AddNode(node_id) => {
            if !cache.node_indices.contains_key(&node_id) {
                let idx = cache.graph.add_node(node_id.clone());
                cache.node_indices.insert(node_id, idx);
            }
        }
        GraphOp::RemoveNode(node_id) => {
            if let Some(&idx) = cache.node_indices.get(&node_id) {
                let to_remove: Vec<EdgeId> = cache
                    .edge_indices
                    .iter()
                    .filter_map(|(eid, &eidx)| {
                        cache
                            .graph
                            .edge_endpoints(eidx)
                            .filter(|(a, b)| *a == idx || *b == idx)
                            .map(|_| eid.clone())
                    })
                    .collect();
                for eid in to_remove {
                    cache.edge_indices.remove(&eid);
                }
                cache.graph.remove_node(idx);
                cache.node_indices.remove(&node_id);
            }
        }
        GraphOp::AddEdge {
            edge_id,
            from,
            to,
            kind,
        } => {
            let fi = cache.node_indices.get(&from).copied();
            let ti = cache.node_indices.get(&to).copied();
            if let (Some(fi), Some(ti)) = (fi, ti) {
                if !cache.edge_indices.contains_key(&edge_id) {
                    let eidx = cache.graph.add_edge(fi, ti, kind);
                    cache.edge_indices.insert(edge_id, eidx);
                }
            }
        }
        GraphOp::RemoveEdge(edge_id) => {
            if let Some(&eidx) = cache.edge_indices.get(&edge_id) {
                cache.graph.remove_edge(eidx);
                cache.edge_indices.remove(&edge_id);
            }
        }
        GraphOp::None => {}
    }
    Ok(())
}
