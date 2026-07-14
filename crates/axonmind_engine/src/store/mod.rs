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

use crate::query::document::DocumentIdentityReport;
use axonmind_core::{
    AxonMindError, Confidence, Edge, EdgeId, EdgeKind, Evidence, EvidenceId, ExtractorKind, Node,
    NodeId, NodeKind, SourceType,
};
use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
    /// Logical document this version belongs to (`ldoc.<uuid>`). Stable across versions.
    pub logical_doc_id: String,
    /// 1-based version number of this (HEAD) row within its logical document.
    pub version_no: i64,
    /// Total versions in this logical document (1 when never re-ingested).
    pub version_count: i64,
}

/// One row in the `document_versions` lineage log (read model). Newest→oldest when listed.
/// See `docs/document_versioning.md` §2.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocumentVersion {
    pub logical_doc_id: String,
    pub version_no: i64,
    pub node_id: String,
    pub sha256: String,
    pub structural_sha256: Option<String>,
    pub indexed_at: i64,
    pub source_path: Option<String>,
    pub previous_node_id: Option<String>,
    pub superseded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestStatusKind {
    Processing,
    Stalled,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestPhase {
    Reading,
    Copying,
    Parsing,
    Extracting,
    Indexing,
    ParseTimeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrashedItemKind {
    CompletedDocument,
    IngestRow,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IngestStatusRow {
    pub source_path: String,
    pub name: String,
    pub content_sha256: Option<String>,
    pub job_id: String,
    pub status: IngestStatusKind,
    pub phase: IngestPhase,
    pub error: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub trashed_at: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrashRow {
    pub source_path: String,
    pub name: String,
    pub trashed_at: i64,
    pub kind: TrashedItemKind,
    pub head_sha256: Option<String>,
    pub status: Option<IngestStatusKind>,
    pub error: Option<String>,
}

/// A new row to append to `document_versions`, written atomically with its graph mutations
/// by `apply_version_batch`. The document_cache HEAD pointer is derived from these fields
/// (path = `source_path`, node = `node_id`), so no separate cache args are needed.
#[derive(Debug, Clone)]
pub(crate) struct NewDocumentVersion {
    pub logical_doc_id: String,
    pub version_no: i64,
    pub node_id: NodeId,
    pub sha256: String,
    pub structural_sha256: Option<String>,
    pub indexed_at: i64,
    pub source_path: Option<String>,
    pub previous_node_id: Option<NodeId>,
    pub superseded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentAliasRecord {
    pub alias: String,
    pub alias_norm: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentIdentityRecord {
    pub doc_node_id: String,
    pub source_filename: String,
    pub source_path: Option<String>,
    pub raw_title: Option<String>,
    pub canonical_title: String,
    pub language: Option<String>,
    pub jurisdiction: Vec<String>,
    pub domain: Vec<String>,
    pub instrument_type: Option<String>,
    pub corpus: Vec<String>,
    pub confidence: f32,
    pub reviewed_at: Option<i64>,
    pub updated_at: i64,
    pub pinned_profile: Option<String>,
    pub aliases: Vec<DocumentAliasRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocUnitRecord {
    pub unit_id: String,
    pub doc_node_id: String,
    pub parent_unit_id: Option<String>,
    pub section_id: String,
    pub unit_kind: String,
    pub label: String,
    pub label_norm: String,
    pub title: Option<String>,
    pub ordinal: i64,
    pub level: i64,
    pub text: String,
    pub span_start: i64,
    pub span_end: i64,
    pub page_start: Option<i64>,
    pub page_end: Option<i64>,
    pub path: String,
    pub citation: String,
    pub package_name: String,
    pub profile_name: String,
    pub profile_version: i64,
    pub confidence: f32,
    /// Content sha256 of the document this unit was parsed from — the other half (with
    /// package_name/profile_name/profile_version above) of the staleness key that decides
    /// whether a re-parse is needed (see `AxonMindEngine::ensure_document_grounding`).
    pub doc_sha256: String,
    /// Arbitrary capture names -> values (e.g. `article`/`paragraph`, or a medical package's
    /// `dosage`/`frequency`). Stored normalized in `doc_unit_locators`, not a column here.
    pub locators: std::collections::BTreeMap<String, String>,
    /// Trust tier of the source document at ingest time (`user_upload`, `plugin_bundle`,
    /// `web_fetched`, `auto_captured` — see `ProvenanceTier`), copied here from the owning
    /// Document node so `document_search`'s hot path can gate `citation_safe` without a join
    /// back to `nodes.attrs` (retrieve_guarantee.md item 10).
    pub provenance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitRefRecord {
    pub from_unit_id: String,
    pub to_doc_node_id: Option<String>,
    pub target_label: String,
    pub target_label_norm: String,
    pub ref_text: String,
    /// The package-declared `[[ref]] target_kind` (e.g. "article", "recital") this reference
    /// resolves to. Lets ambiguous cross-reference resolution filter by kind generically instead
    /// of guessing it back from `target_label_norm`'s string prefix.
    pub target_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructurePackageRecord {
    pub package_name: String,
    pub version: i64,
    pub description: Option<String>,
    pub content_sha: String,
    pub imported_at: i64,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureProfileRecord {
    pub package_name: String,
    pub profile_name: String,
    pub version: i64,
    pub definition: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityRuleRecord {
    pub package_name: String,
    pub ordinal: i64,
    pub definition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusBindingRecord {
    pub package_name: String,
    pub ordinal: i64,
    pub kind: String,
    pub definition: String,
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

    /// Like `apply_batch`, but also — in the SAME transaction — appends `document_versions` rows
    /// and upserts the `document_cache` HEAD pointer. This is the atomicity guarantee from
    /// `docs/document_versioning.md`: the graph never holds a version's nodes without its version
    /// row, nor vice versa.
    ///
    /// `version` is the new HEAD row (drives the cache pointer). `retained` is the prior version's
    /// row, re-inserted (flagged superseded) on the supersede path: deleting+retaining the old
    /// node cascades its original `document_versions` row away (FK ON DELETE CASCADE), so it must
    /// be rewritten here rather than updated in place. The cache HEAD is derived from `version`
    /// (path = `source_path`, node = `node_id`); cache upsert is skipped when `source_path` is None
    /// (documents ingested from raw text have no backing path).
    pub(crate) async fn apply_version_batch(
        &self,
        mutations: Vec<GraphMutation>,
        version: NewDocumentVersion,
        retained: Option<NewDocumentVersion>,
        cache: &tokio::sync::RwLock<GraphCache>,
        event_tx: &tokio::sync::broadcast::Sender<crate::events::EngineEvent>,
    ) -> Result<(), AxonMindError> {
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

                    // Re-insert the retained (now superseded) prior row first, then the new HEAD.
                    // Both reference nodes (re)created by the mutations above, so FKs hold.
                    if let Some(r) = &retained {
                        insert_version_row(&tx, r)?;
                    }
                    insert_version_row(&tx, &version)?;

                    if let Some(path) = &version.source_path {
                        tx.execute(
                            "INSERT INTO document_cache (path, sha256, structural_sha256, indexed_at, node_id)
                             VALUES (?1,?2,?3,?4,?5)
                             ON CONFLICT(path) DO UPDATE SET sha256=excluded.sha256,
                               structural_sha256=excluded.structural_sha256,
                               indexed_at=excluded.indexed_at, node_id=excluded.node_id",
                            rusqlite::params![
                                path,
                                version.sha256,
                                version.structural_sha256.clone().unwrap_or_default(),
                                version.indexed_at,
                                version.node_id.0,
                            ],
                        )
                        .map_err(|e| AxonMindError::Database(e.to_string()))?;
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
                    tracing::warn!(
                        "cache patch failed after version batch commit, marked dirty: {e}"
                    );
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

fn vec_to_json(v: &[String]) -> Result<String, AxonMindError> {
    serde_json::to_string(v).map_err(|e| AxonMindError::Serialization(e.to_string()))
}

fn vec_from_json(s: &str) -> Result<Vec<String>, AxonMindError> {
    serde_json::from_str(s).map_err(|e| AxonMindError::Serialization(e.to_string()))
}

/// Insert one `document_versions` row inside an open transaction. Shared by the new-HEAD and
/// retained-superseded inserts in `apply_version_batch`.
fn insert_version_row(
    tx: &rusqlite::Transaction,
    row: &NewDocumentVersion,
) -> Result<(), AxonMindError> {
    tx.execute(
        "INSERT INTO document_versions
            (logical_doc_id, version_no, node_id, sha256, structural_sha256,
             indexed_at, source_path, previous_node_id, superseded)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        rusqlite::params![
            row.logical_doc_id,
            row.version_no,
            row.node_id.0,
            row.sha256,
            row.structural_sha256,
            row.indexed_at,
            row.source_path,
            row.previous_node_id.as_ref().map(|n| n.0.clone()),
            row.superseded as i64,
        ],
    )
    .map_err(|e| AxonMindError::Database(e.to_string()))?;
    Ok(())
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

    pub(crate) async fn fetch_all_edges(&self) -> Result<Vec<Edge>, AxonMindError> {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<Vec<Edge>, AxonMindError> {
            let edge_ids: Vec<String> = {
                let mut stmt = conn
                    .prepare("SELECT id FROM edges")
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt
                    .query_map([], |row| row.get(0))
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

    /// Content-addressable cache of `render_markdown(&NormalizedDocument)` output, keyed by the
    /// source blob's sha256. Lets `get_document_content` serve a preview without re-parsing.
    pub(crate) async fn upsert_document_markdown(
        &self,
        sha256: &str,
        markdown: &str,
    ) -> Result<(), AxonMindError> {
        let sha = sha256.to_owned();
        let md = markdown.to_owned();
        let now = Utc::now().timestamp();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            // Must overwrite, not ignore: the key is the source blob's sha256, which does NOT
            // change when converter code changes — this upsert on re-ingest is the only path a
            // converter fix has to already-cached documents (an earlier INSERT OR IGNORE left
            // every cache row frozen at its first-ever render; retrieve_guarantee.md 2026-07-13).
            conn.execute(
                "INSERT INTO document_markdown (sha256, markdown, built_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(sha256) DO UPDATE SET markdown=excluded.markdown, built_at=excluded.built_at",
                rusqlite::params![sha, md, now],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn get_document_markdown(
        &self,
        sha256: &str,
    ) -> Result<Option<String>, AxonMindError> {
        let sha = sha256.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Option<String>, AxonMindError> {
            conn.query_row(
                "SELECT markdown FROM document_markdown WHERE sha256 = ?1",
                [&sha],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| AxonMindError::Database(e.to_string()))
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn upsert_document_identity(
        &self,
        identity: &DocumentIdentityRecord,
    ) -> Result<(), AxonMindError> {
        let identity = identity.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            let tx = conn
                .transaction()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let jurisdiction = vec_to_json(&identity.jurisdiction)?;
            let domain = vec_to_json(&identity.domain)?;
            let corpus = vec_to_json(&identity.corpus)?;
            tx.execute(
                "INSERT INTO document_identity
                    (doc_node_id, source_filename, source_path, raw_title, canonical_title, language,
                     jurisdiction, domain, instrument_type, corpus, confidence, reviewed_at, updated_at,
                     pinned_profile)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
                 ON CONFLICT(doc_node_id) DO UPDATE SET
                    source_filename=excluded.source_filename,
                    source_path=excluded.source_path,
                    raw_title=excluded.raw_title,
                    canonical_title=excluded.canonical_title,
                    language=excluded.language,
                    jurisdiction=excluded.jurisdiction,
                    domain=excluded.domain,
                    instrument_type=excluded.instrument_type,
                    corpus=excluded.corpus,
                    confidence=excluded.confidence,
                    reviewed_at=excluded.reviewed_at,
                    updated_at=excluded.updated_at",
                rusqlite::params![
                    identity.doc_node_id,
                    identity.source_filename,
                    identity.source_path,
                    identity.raw_title,
                    identity.canonical_title,
                    identity.language,
                    jurisdiction,
                    domain,
                    identity.instrument_type,
                    corpus,
                    identity.confidence,
                    identity.reviewed_at,
                    identity.updated_at,
                    identity.pinned_profile,
                ],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            tx.execute(
                "DELETE FROM document_aliases WHERE doc_node_id = ?1",
                rusqlite::params![identity.doc_node_id],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            for alias in &identity.aliases {
                tx.execute(
                    "INSERT INTO document_aliases (doc_node_id, alias, alias_norm, source)
                     VALUES (?1,?2,?3,?4)",
                    rusqlite::params![
                        identity.doc_node_id,
                        alias.alias,
                        alias.alias_norm,
                        alias.source
                    ],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            }
            tx.execute(
                "DELETE FROM document_identity_fts WHERE doc_node_id = ?1",
                rusqlite::params![identity.doc_node_id],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let alias_blob = identity
                .aliases
                .iter()
                .map(|alias| alias.alias.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            tx.execute(
                "INSERT INTO document_identity_fts
                    (doc_node_id, canonical_title, aliases, source_filename, corpus, jurisdiction, domain, instrument_type)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![
                    identity.doc_node_id,
                    identity.canonical_title,
                    alias_blob,
                    identity.source_filename,
                    identity.corpus.join(" "),
                    identity.jurisdiction.join(" "),
                    identity.domain.join(" "),
                    identity.instrument_type,
                ],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            tx.commit()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Sets (or clears, with `None`) the explicit user pin on an existing `document_identity`
    /// row. Does not create a row — a document must already have been derived at least once
    /// (fresh ingest or the identity catalog pass) before it can be pinned.
    pub(crate) async fn set_pinned_profile(
        &self,
        doc_node_id: &str,
        profile_name: Option<&str>,
    ) -> Result<(), AxonMindError> {
        let doc_id = doc_node_id.to_owned();
        let profile_name = profile_name.map(str::to_owned);
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            let updated = conn
                .execute(
                    "UPDATE document_identity SET pinned_profile = ?1 WHERE doc_node_id = ?2",
                    rusqlite::params![profile_name, doc_id],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            if updated == 0 {
                return Err(AxonMindError::Preview {
                    message: "Document identity not found; ingest the document before pinning."
                        .to_string(),
                });
            }
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Sets the provenance tier on every existing `doc_units` row for `doc_node_id` (item 4c-style
    /// override). Pure `doc_units` update — the Document node's own `attrs.provenance` (the
    /// source of truth a future re-parse reads) is a `serde_json::Value` inside a msgpack blob,
    /// not JSON1-addressable SQL, so that half of the write goes through the normal
    /// `fetch_node`/`apply_mutation` path in `AxonMindEngine::set_document_provenance` instead of
    /// here — see that method for the full two-part write.
    pub(crate) async fn set_doc_units_provenance(
        &self,
        doc_node_id: &str,
        provenance: &str,
    ) -> Result<(), AxonMindError> {
        let doc_id = doc_node_id.to_owned();
        let provenance = provenance.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            conn.execute(
                "UPDATE doc_units SET provenance = ?1 WHERE doc_node_id = ?2",
                rusqlite::params![provenance, doc_id],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn fetch_document_identity(
        &self,
        doc_node_id: &str,
    ) -> Result<Option<DocumentIdentityRecord>, AxonMindError> {
        let doc_id = doc_node_id.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Option<DocumentIdentityRecord>, AxonMindError> {
            let row = conn
                .query_row(
                    "SELECT doc_node_id, source_filename, source_path, raw_title, canonical_title, language,
                            jurisdiction, domain, instrument_type, corpus, confidence, reviewed_at, updated_at,
                            pinned_profile
                     FROM document_identity
                     WHERE doc_node_id = ?1",
                    [&doc_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, String>(7)?,
                            row.get::<_, Option<String>>(8)?,
                            row.get::<_, String>(9)?,
                            row.get::<_, f64>(10)?,
                            row.get::<_, Option<i64>>(11)?,
                            row.get::<_, i64>(12)?,
                            row.get::<_, Option<String>>(13)?,
                        ))
                    },
                )
                .optional()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            let Some(row) = row else {
                return Ok(None);
            };

            let mut stmt = conn
                .prepare(
                    "SELECT alias, alias_norm, source
                     FROM document_aliases
                     WHERE doc_node_id = ?1
                     ORDER BY alias ASC",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let aliases = stmt
                .query_map([&doc_id], |alias_row| {
                    Ok(DocumentAliasRecord {
                        alias: alias_row.get(0)?,
                        alias_norm: alias_row.get(1)?,
                        source: alias_row.get(2)?,
                    })
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            Ok(Some(DocumentIdentityRecord {
                doc_node_id: row.0,
                source_filename: row.1,
                source_path: row.2,
                raw_title: row.3,
                canonical_title: row.4,
                language: row.5,
                jurisdiction: vec_from_json(&row.6)?,
                domain: vec_from_json(&row.7)?,
                instrument_type: row.8,
                corpus: vec_from_json(&row.9)?,
                confidence: row.10 as f32,
                reviewed_at: row.11,
                updated_at: row.12,
                aliases,
                pinned_profile: row.13,
            }))
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn search_document_identities(
        &self,
        query: Option<&str>,
        corpus: Option<&str>,
        limit: usize,
    ) -> Result<Vec<DocumentIdentityRecord>, AxonMindError> {
        let query = query.map(|q| {
            q.split_whitespace()
                .map(|part| format!("\"{}\"", part.replace('"', "")))
                .collect::<Vec<_>>()
                .join(" ")
        });
        let corpus = corpus.map(str::to_owned);
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<DocumentIdentityRecord>, AxonMindError> {
            let sql = match (query.as_ref(), corpus.as_ref()) {
                (Some(_), Some(_)) => {
                    "SELECT di.doc_node_id
                     FROM document_identity di
                     JOIN document_identity_fts fts ON fts.doc_node_id = di.doc_node_id
                     WHERE document_identity_fts MATCH ?1 AND di.corpus LIKE ?2
                     ORDER BY di.confidence DESC, di.updated_at DESC
                     LIMIT ?3"
                }
                (Some(_), None) => {
                    "SELECT di.doc_node_id
                     FROM document_identity di
                     JOIN document_identity_fts fts ON fts.doc_node_id = di.doc_node_id
                     WHERE document_identity_fts MATCH ?1
                     ORDER BY di.confidence DESC, di.updated_at DESC
                     LIMIT ?2"
                }
                (None, Some(_)) => {
                    "SELECT doc_node_id
                     FROM document_identity
                     WHERE corpus LIKE ?1
                     ORDER BY confidence DESC, updated_at DESC
                     LIMIT ?2"
                }
                (None, None) => {
                    "SELECT doc_node_id
                     FROM document_identity
                     ORDER BY confidence DESC, updated_at DESC
                     LIMIT ?1"
                }
            };

            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            let like_corpus = corpus.as_ref().map(|value| format!("%{value}%"));
            let doc_ids: Vec<String> = match (query.as_ref(), like_corpus.as_ref()) {
                (Some(query), Some(corpus)) => stmt
                    .query_map(rusqlite::params![query, corpus, limit as i64], |row| row.get(0))
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?,
                (Some(query), None) => stmt
                    .query_map(rusqlite::params![query, limit as i64], |row| row.get(0))
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?,
                (None, Some(corpus)) => stmt
                    .query_map(rusqlite::params![corpus, limit as i64], |row| row.get(0))
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?,
                (None, None) => stmt
                    .query_map(rusqlite::params![limit as i64], |row| row.get(0))
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?,
            };

            let mut out = Vec::with_capacity(doc_ids.len());
            for doc_id in doc_ids {
                if let Some(identity) = conn
                    .query_row(
                        "SELECT doc_node_id, source_filename, source_path, raw_title, canonical_title, language,
                                jurisdiction, domain, instrument_type, corpus, confidence, reviewed_at, updated_at,
                                pinned_profile
                         FROM document_identity
                         WHERE doc_node_id = ?1",
                        [&doc_id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, Option<String>>(2)?,
                                row.get::<_, Option<String>>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, Option<String>>(5)?,
                                row.get::<_, String>(6)?,
                                row.get::<_, String>(7)?,
                                row.get::<_, Option<String>>(8)?,
                                row.get::<_, String>(9)?,
                                row.get::<_, f64>(10)?,
                                row.get::<_, Option<i64>>(11)?,
                                row.get::<_, i64>(12)?,
                                row.get::<_, Option<String>>(13)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                {
                    let mut alias_stmt = conn
                        .prepare(
                            "SELECT alias, alias_norm, source
                             FROM document_aliases
                             WHERE doc_node_id = ?1
                             ORDER BY alias ASC",
                        )
                        .map_err(|e| AxonMindError::Database(e.to_string()))?;
                    let aliases = alias_stmt
                        .query_map([&doc_id], |row| {
                            Ok(DocumentAliasRecord {
                                alias: row.get(0)?,
                                alias_norm: row.get(1)?,
                                source: row.get(2)?,
                            })
                        })
                        .map_err(|e| AxonMindError::Database(e.to_string()))?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(|e| AxonMindError::Database(e.to_string()))?;
                    out.push(DocumentIdentityRecord {
                        doc_node_id: identity.0,
                        source_filename: identity.1,
                        source_path: identity.2,
                        raw_title: identity.3,
                        canonical_title: identity.4,
                        language: identity.5,
                        jurisdiction: vec_from_json(&identity.6)?,
                        domain: vec_from_json(&identity.7)?,
                        instrument_type: identity.8,
                        corpus: vec_from_json(&identity.9)?,
                        confidence: identity.10 as f32,
                        reviewed_at: identity.11,
                        updated_at: identity.12,
                        pinned_profile: identity.13,
                        aliases,
                    });
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// One row per `document_identity` row, joined against `doc_units` for the parsed-unit
    /// count and the profile that produced them (`MAX()` picks an arbitrary row's binding since
    /// every unit for one doc shares the same `package_name`/`profile_name`/`profile_version` —
    /// the same invariant `doc_unit_staleness_key` already relies on). Powers the Library review
    /// UI's identity list (`retrieve_guarantee.md` item 4a); no filtering here, the caller
    /// filters client-side (unclaimed / low-confidence).
    pub(crate) async fn list_document_identity_reports(
        &self,
    ) -> Result<Vec<DocumentIdentityReport>, AxonMindError> {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<DocumentIdentityReport>, AxonMindError> {
            let mut stmt = conn
                .prepare(
                    "SELECT di.doc_node_id, di.source_filename, di.source_path, di.canonical_title,
                            di.instrument_type, di.corpus, di.confidence, di.pinned_profile, di.updated_at,
                            MAX(du.profile_name), MAX(du.profile_version), COUNT(du.doc_node_id),
                            MAX(du.provenance)
                     FROM document_identity di
                     LEFT JOIN doc_units du ON du.doc_node_id = di.doc_node_id
                     GROUP BY di.doc_node_id
                     ORDER BY di.confidence ASC, di.updated_at DESC",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, f64>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, Option<String>>(9)?,
                        row.get::<_, Option<i64>>(10)?,
                        row.get::<_, i64>(11)?,
                        row.get::<_, Option<String>>(12)?,
                    ))
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                out.push(DocumentIdentityReport {
                    doc_node_id: row.0,
                    source_filename: row.1,
                    source_path: row.2,
                    canonical_title: row.3,
                    instrument_type: row.4,
                    corpus: vec_from_json(&row.5)?,
                    confidence: row.6 as f32,
                    pinned_profile: row.7,
                    updated_at: row.8,
                    profile_name: row.9,
                    profile_version: row.10,
                    unit_count: row.11,
                    provenance: row.12,
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn list_document_ids_by_corpus(
        &self,
        corpus: &str,
    ) -> Result<Vec<String>, AxonMindError> {
        let corpus = format!("%{corpus}%");
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<String>, AxonMindError> {
            let mut stmt = conn
                .prepare(
                    "SELECT doc_node_id
                     FROM document_identity
                     WHERE corpus LIKE ?1
                     ORDER BY confidence DESC, updated_at DESC",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            stmt.query_map([&corpus], |row| row.get(0))
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn install_structure_package(
        &self,
        pkg: &crate::structure::model::StructurePackage,
        source: &str,
    ) -> Result<StructurePackageRecord, AxonMindError> {
        let package_name = pkg.manifest.package.name.clone();
        let version = pkg.manifest.package.version;
        let description = pkg.manifest.package.description.clone();
        let content_sha = pkg.content_sha()?;
        let profiles = pkg.profiles.clone();
        let identity_rules = pkg.identity.rules.clone();
        let corpus = pkg.corpus.clone();
        let source = source.to_string();
        let imported_at = chrono::Utc::now().timestamp();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<StructurePackageRecord, AxonMindError> {
            let tx = conn
                .transaction()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            if let Some((existing_version, existing_sha)) = tx
                .query_row(
                    "SELECT version, content_sha FROM structure_packages WHERE package_name = ?1",
                    [&package_name],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|e| AxonMindError::Database(e.to_string()))?
            {
                if existing_version == version && existing_sha != content_sha {
                    return Err(AxonMindError::ValidationFailed {
                        message: format!(
                            "package {} version {} changed without version bump",
                            package_name, version
                        ),
                    });
                }
            }

            tx.execute(
                "INSERT INTO structure_packages (package_name, version, description, content_sha, imported_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(package_name) DO UPDATE SET
                    version=excluded.version,
                    description=excluded.description,
                    content_sha=excluded.content_sha,
                    imported_at=excluded.imported_at",
                rusqlite::params![package_name, version, description, content_sha, imported_at],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;

            tx.execute(
                "INSERT INTO structure_package_sources (package_name, source, imported_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(package_name, source) DO UPDATE SET imported_at=excluded.imported_at",
                rusqlite::params![package_name, source, imported_at],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;

            tx.execute(
                "DELETE FROM structure_profiles WHERE package_name = ?1",
                [&package_name],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            for profile in profiles {
                let definition = serde_json::to_string(&profile)
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                tx.execute(
                    "INSERT INTO structure_profiles (package_name, profile_name, version, definition, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![
                        package_name,
                        profile.profile.name,
                        profile.profile.version,
                        definition,
                        imported_at
                    ],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            }

            tx.execute("DELETE FROM identity_rules WHERE package_name = ?1", [&package_name])
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            for (ordinal, rule) in identity_rules.iter().enumerate() {
                let definition = serde_json::to_string(rule)
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                tx.execute(
                    "INSERT INTO identity_rules (package_name, ordinal, definition)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![package_name, ordinal as i64, definition],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            }

            tx.execute(
                "DELETE FROM corpus_bindings WHERE package_name = ?1",
                [&package_name],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            if let Some(corpus) = corpus {
                for (ordinal, binding) in corpus.binds.iter().enumerate() {
                    let definition = serde_json::to_string(binding)
                        .map_err(|e| AxonMindError::Database(e.to_string()))?;
                    tx.execute(
                        "INSERT INTO corpus_bindings (package_name, ordinal, kind, definition)
                         VALUES (?1, ?2, 'bind', ?3)",
                        rusqlite::params![package_name, ordinal as i64, definition],
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                }
                for (ordinal, binding) in corpus.term_maps.iter().enumerate() {
                    let definition = serde_json::to_string(binding)
                        .map_err(|e| AxonMindError::Database(e.to_string()))?;
                    tx.execute(
                        "INSERT INTO corpus_bindings (package_name, ordinal, kind, definition)
                         VALUES (?1, ?2, 'term_map', ?3)",
                        rusqlite::params![package_name, ordinal as i64, definition],
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                }
                for (ordinal, binding) in corpus.xrefs.iter().enumerate() {
                    let definition = serde_json::to_string(binding)
                        .map_err(|e| AxonMindError::Database(e.to_string()))?;
                    tx.execute(
                        "INSERT INTO corpus_bindings (package_name, ordinal, kind, definition)
                         VALUES (?1, ?2, 'xref', ?3)",
                        rusqlite::params![package_name, ordinal as i64, definition],
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                }
                if let Some(binding) = corpus.enrichment.as_ref() {
                    let definition = serde_json::to_string(binding)
                        .map_err(|e| AxonMindError::Database(e.to_string()))?;
                    tx.execute(
                        "INSERT INTO corpus_bindings (package_name, ordinal, kind, definition)
                         VALUES (?1, 0, 'enrichment', ?2)",
                        rusqlite::params![package_name, definition],
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                }
            }

            tx.commit()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(StructurePackageRecord {
                package_name,
                version,
                description,
                content_sha,
                imported_at,
                sources: vec![source],
            })
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn list_structure_packages(
        &self,
    ) -> Result<Vec<StructurePackageRecord>, AxonMindError> {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<StructurePackageRecord>, AxonMindError> {
            let mut stmt = conn
                .prepare(
                    "SELECT package_name, version, description, content_sha, imported_at
                     FROM structure_packages
                     ORDER BY package_name",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let base_rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            let mut out = Vec::new();
            for (package_name, version, description, content_sha, imported_at) in base_rows {
                let mut source_stmt = conn
                    .prepare(
                        "SELECT source FROM structure_package_sources WHERE package_name = ?1 ORDER BY source",
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let sources = source_stmt
                    .query_map([&package_name], |row| row.get::<_, String>(0))
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                out.push(StructurePackageRecord {
                    package_name,
                    version,
                    description,
                    content_sha,
                    imported_at,
                    sources,
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn remove_structure_package_source(
        &self,
        package_name: &str,
        source: &str,
    ) -> Result<bool, AxonMindError> {
        let package_name = package_name.to_string();
        let source = source.to_string();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<bool, AxonMindError> {
            let tx = conn
                .transaction()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            tx.execute(
                "DELETE FROM structure_package_sources WHERE package_name = ?1 AND source = ?2",
                rusqlite::params![package_name, source],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let remaining: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM structure_package_sources WHERE package_name = ?1",
                    [&package_name],
                    |row| row.get(0),
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            if remaining == 0 {
                tx.execute(
                    "DELETE FROM structure_packages WHERE package_name = ?1",
                    [&package_name],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
                tx.execute(
                    "DELETE FROM structure_profiles WHERE package_name = ?1",
                    [&package_name],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
                tx.execute(
                    "DELETE FROM identity_rules WHERE package_name = ?1",
                    [&package_name],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
                tx.execute(
                    "DELETE FROM corpus_bindings WHERE package_name = ?1",
                    [&package_name],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            }
            tx.commit()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(remaining == 0)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Distinct corpus names a package's `[[bind]]` rows resolve into, read straight from
    /// `corpus_bindings` (kind='bind'). The `[corpus] name` in a package's `corpus.toml` is
    /// never persisted itself — only the bind rows' `corpus: Vec<String>` are (see
    /// `install_structure_package`) — so this is the only source of truth for "what corpus
    /// name(s) does this package actually resolve documents into."
    pub(crate) async fn structure_package_corpora(
        &self,
        package_name: &str,
    ) -> Result<Vec<String>, AxonMindError> {
        let package_name = package_name.to_string();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<String>, AxonMindError> {
            let mut stmt = conn
                .prepare(
                    "SELECT definition FROM corpus_bindings
                     WHERE package_name = ?1 AND kind = 'bind'
                     ORDER BY ordinal",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let definitions = stmt
                .query_map([&package_name], |row| row.get::<_, String>(0))
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            let mut corpora = Vec::new();
            for definition in definitions {
                let binding: crate::structure::model::CorpusBinding =
                    serde_json::from_str(&definition)
                        .map_err(|e| AxonMindError::Database(e.to_string()))?;
                for name in binding.corpus {
                    if !corpora.contains(&name) {
                        corpora.push(name);
                    }
                }
            }
            Ok(corpora)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("pool interact: {e}")))?
    }

    pub(crate) async fn load_structure_packages(
        &self,
    ) -> Result<Vec<crate::structure::model::StructurePackage>, AxonMindError> {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<crate::structure::model::StructurePackage>, AxonMindError> {
            let mut stmt = conn
                .prepare(
                    "SELECT package_name, version, description
                     FROM structure_packages
                     ORDER BY package_name",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            let mut out = Vec::new();
            for (package_name, version, description) in rows {
                let mut profile_stmt = conn
                    .prepare(
                        "SELECT definition FROM structure_profiles WHERE package_name = ?1 ORDER BY profile_name",
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let profiles = profile_stmt
                    .query_map([&package_name], |row| row.get::<_, String>(0))
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .into_iter()
                    .map(|value| {
                        serde_json::from_str::<crate::structure::model::ProfileDefinition>(&value)
                            .map_err(|e| AxonMindError::Database(e.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                let mut rule_stmt = conn
                    .prepare(
                        "SELECT definition FROM identity_rules WHERE package_name = ?1 ORDER BY ordinal",
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let rules = rule_stmt
                    .query_map([&package_name], |row| row.get::<_, String>(0))
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .into_iter()
                    .map(|value| {
                        serde_json::from_str::<crate::structure::model::IdentityRule>(&value)
                            .map_err(|e| AxonMindError::Database(e.to_string()))
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                let mut bind_stmt = conn
                    .prepare(
                        "SELECT kind, definition FROM corpus_bindings WHERE package_name = ?1 ORDER BY kind, ordinal",
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let bindings = bind_stmt
                    .query_map([&package_name], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;

                let mut corpus = crate::structure::model::CorpusFile {
                    corpus: None,
                    binds: Vec::new(),
                    term_maps: Vec::new(),
                    xrefs: Vec::new(),
                    enrichment: None,
                };
                for (kind, definition) in bindings {
                    match kind.as_str() {
                        "bind" => corpus.binds.push(
                            serde_json::from_str(&definition)
                                .map_err(|e| AxonMindError::Database(e.to_string()))?,
                        ),
                        "term_map" => corpus.term_maps.push(
                            serde_json::from_str(&definition)
                                .map_err(|e| AxonMindError::Database(e.to_string()))?,
                        ),
                        "xref" => corpus.xrefs.push(
                            serde_json::from_str(&definition)
                                .map_err(|e| AxonMindError::Database(e.to_string()))?,
                        ),
                        "enrichment" => {
                            corpus.enrichment = Some(
                                serde_json::from_str(&definition)
                                    .map_err(|e| AxonMindError::Database(e.to_string()))?,
                            )
                        }
                        _ => {}
                    }
                }

                out.push(crate::structure::model::StructurePackage {
                    root_dir: PathBuf::new(),
                    manifest: crate::structure::model::PackageManifest {
                        package: crate::structure::model::PackageMeta {
                            name: package_name,
                            version,
                            description,
                            eval_policy: None,
                        },
                    },
                    profiles,
                    identity: crate::structure::model::IdentityRulesFile { rules },
                    corpus: if corpus.binds.is_empty()
                        && corpus.term_maps.is_empty()
                        && corpus.xrefs.is_empty()
                        && corpus.enrichment.is_none()
                    {
                        None
                    } else {
                        Some(corpus)
                    },
                    // Evals aren't persisted to the DB (retrieve_guarantee.md item 7 keeps them
                    // file-only, re-parsed from skill_files on every reconcile) — a package
                    // reconstructed from these tables is never the input to run_structure_evals.
                    evals: Vec::new(),
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Load `doc_unit_locators` rows for a batch of unit ids into a per-unit map. Kept as a
    /// free function (not a method) so it can run inside the sync `conn.interact` closures
    /// below without capturing `&self`.
    fn load_unit_locators(
        conn: &rusqlite::Connection,
        unit_ids: &[String],
    ) -> rusqlite::Result<
        std::collections::HashMap<String, std::collections::BTreeMap<String, String>>,
    > {
        if unit_ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let placeholders = unit_ids
            .iter()
            .enumerate()
            .map(|(idx, _)| format!("?{}", idx + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT unit_id, key, value FROM doc_unit_locators WHERE unit_id IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = unit_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        let mut out: std::collections::HashMap<String, std::collections::BTreeMap<String, String>> =
            std::collections::HashMap::new();
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (unit_id, key, value) = row?;
            out.entry(unit_id).or_default().insert(key, value);
        }
        Ok(out)
    }

    fn row_to_doc_unit(row: &rusqlite::Row<'_>) -> rusqlite::Result<DocUnitRecord> {
        Ok(DocUnitRecord {
            unit_id: row.get(0)?,
            doc_node_id: row.get(1)?,
            parent_unit_id: row.get(2)?,
            section_id: row.get(3)?,
            unit_kind: row.get(4)?,
            label: row.get(5)?,
            label_norm: row.get(6)?,
            title: row.get(7)?,
            ordinal: row.get(8)?,
            level: row.get(9)?,
            text: row.get(10)?,
            span_start: row.get(11)?,
            span_end: row.get(12)?,
            page_start: row.get(13)?,
            page_end: row.get(14)?,
            path: row.get(15)?,
            citation: row.get(16)?,
            package_name: row.get(17)?,
            profile_name: row.get(18)?,
            profile_version: row.get(19)?,
            confidence: row.get::<_, f64>(20)? as f32,
            doc_sha256: row.get(21)?,
            locators: std::collections::BTreeMap::new(),
            provenance: row.get(22)?,
        })
    }

    const DOC_UNIT_COLUMNS: &'static str = "unit_id, doc_node_id, parent_unit_id, section_id, unit_kind, label, label_norm, \
         title, ordinal, level, text, span_start, span_end, page_start, page_end, path, \
         citation, package_name, profile_name, profile_version, confidence, doc_sha256, provenance";

    /// The staleness key `(doc_sha256, package_name, profile_name, profile_version)` for a
    /// document's existing parse, read from any one of its units (they are always written as a
    /// single homogeneous batch by `replace_doc_units`, so any row carries the same key).
    /// `None` means the document has never been parsed (equivalent to the old "count == 0" gate).
    pub(crate) async fn doc_unit_staleness_key(
        &self,
        doc_node_id: &str,
    ) -> Result<Option<(String, String, String, i64)>, AxonMindError> {
        let doc_id = doc_node_id.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(
            move |conn| -> Result<Option<(String, String, String, i64)>, AxonMindError> {
                conn.query_row(
                    "SELECT doc_sha256, package_name, profile_name, profile_version \
                     FROM doc_units WHERE doc_node_id = ?1 LIMIT 1",
                    [&doc_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(|e| AxonMindError::Database(e.to_string()))
            },
        )
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn replace_doc_units(
        &self,
        doc_node_id: &str,
        units: &[DocUnitRecord],
        refs: &[UnitRefRecord],
    ) -> Result<(), AxonMindError> {
        let doc_id = doc_node_id.to_owned();
        let units = units.to_vec();
        let refs = refs.to_vec();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            let tx = conn
                .transaction()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            tx.execute(
                "DELETE FROM doc_unit_refs
                 WHERE from_unit_id IN (SELECT unit_id FROM doc_units WHERE doc_node_id = ?1)",
                rusqlite::params![doc_id],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            tx.execute(
                "DELETE FROM doc_unit_locators
                 WHERE doc_node_id = ?1",
                rusqlite::params![doc_id],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            tx.execute(
                "DELETE FROM doc_units WHERE doc_node_id = ?1",
                rusqlite::params![doc_id],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            for unit in &units {
                tx.execute(
                    "INSERT INTO doc_units
                        (unit_id, doc_node_id, parent_unit_id, section_id, unit_kind, label, label_norm,
                         title, ordinal, level, text, span_start, span_end, page_start, page_end, path,
                         citation, package_name, profile_name, profile_version, confidence, doc_sha256,
                         provenance)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
                    rusqlite::params![
                        unit.unit_id,
                        unit.doc_node_id,
                        unit.parent_unit_id,
                        unit.section_id,
                        unit.unit_kind,
                        unit.label,
                        unit.label_norm,
                        unit.title,
                        unit.ordinal,
                        unit.level,
                        unit.text,
                        unit.span_start,
                        unit.span_end,
                        unit.page_start,
                        unit.page_end,
                        unit.path,
                        unit.citation,
                        unit.package_name,
                        unit.profile_name,
                        unit.profile_version,
                        unit.confidence,
                        unit.doc_sha256,
                        unit.provenance,
                    ],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
                for (key, value) in &unit.locators {
                    tx.execute(
                        "INSERT INTO doc_unit_locators (unit_id, doc_node_id, key, value)
                         VALUES (?1,?2,?3,?4)",
                        rusqlite::params![unit.unit_id, unit.doc_node_id, key, value],
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                }
            }
            for reference in &refs {
                tx.execute(
                    "INSERT OR REPLACE INTO doc_unit_refs
                        (from_unit_id, to_doc_node_id, target_label, target_label_norm, ref_text, target_kind)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    rusqlite::params![
                        reference.from_unit_id,
                        reference.to_doc_node_id,
                        reference.target_label,
                        reference.target_label_norm,
                        reference.ref_text,
                        reference.target_kind,
                    ],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            }
            tx.commit()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn fetch_doc_units_by_section_ids(
        &self,
        section_ids: &[String],
    ) -> Result<Vec<DocUnitRecord>, AxonMindError> {
        if section_ids.is_empty() {
            return Ok(vec![]);
        }
        let ids = section_ids.to_vec();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<DocUnitRecord>, AxonMindError> {
            let placeholders = ids
                .iter()
                .enumerate()
                .map(|(idx, _)| format!("?{}", idx + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT {} FROM doc_units WHERE section_id IN ({placeholders})",
                Self::DOC_UNIT_COLUMNS
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let params: Vec<&dyn rusqlite::ToSql> =
                ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            let mut units = stmt
                .query_map(params.as_slice(), |row| Self::row_to_doc_unit(row))
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let unit_ids: Vec<String> = units.iter().map(|u| u.unit_id.clone()).collect();
            let mut locators = Self::load_unit_locators(conn, &unit_ids)
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            for unit in &mut units {
                unit.locators = locators.remove(&unit.unit_id).unwrap_or_default();
            }
            Ok(units)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn fetch_doc_unit_by_section(
        &self,
        doc_node_id: &str,
        section_id: &str,
    ) -> Result<Option<DocUnitRecord>, AxonMindError> {
        let doc_id = doc_node_id.to_owned();
        let section_id = section_id.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(
            move |conn| -> Result<Option<DocUnitRecord>, AxonMindError> {
                let sql = format!(
                    "SELECT {} FROM doc_units WHERE doc_node_id = ?1 AND section_id = ?2",
                    Self::DOC_UNIT_COLUMNS
                );
                let mut unit = conn
                    .query_row(&sql, rusqlite::params![doc_id, section_id], |row| {
                        Self::row_to_doc_unit(row)
                    })
                    .optional()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                if let Some(unit) = unit.as_mut() {
                    let mut locators =
                        Self::load_unit_locators(conn, std::slice::from_ref(&unit.unit_id))
                            .map_err(|e| AxonMindError::Database(e.to_string()))?;
                    unit.locators = locators.remove(&unit.unit_id).unwrap_or_default();
                }
                Ok(unit)
            },
        )
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Fetch units for `doc_node_id` whose locator map is a superset of `constraints` (every
    /// requested key must be present with a matching value). Filters in Rust rather than
    /// building dynamic `json_extract`/join SQL — per-document unit counts are small (tens to
    /// low hundreds), so this stays simple and avoids a dynamic-SQL surface entirely.
    pub(crate) async fn fetch_doc_units_by_locator(
        &self,
        doc_node_id: &str,
        constraints: &std::collections::BTreeMap<String, String>,
    ) -> Result<Vec<DocUnitRecord>, AxonMindError> {
        let doc_id = doc_node_id.to_owned();
        let constraints = constraints.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<DocUnitRecord>, AxonMindError> {
            let sql = format!(
                "SELECT {} FROM doc_units WHERE doc_node_id = ?1",
                Self::DOC_UNIT_COLUMNS
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let mut units = stmt
                .query_map(rusqlite::params![doc_id], |row| Self::row_to_doc_unit(row))
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let unit_ids: Vec<String> = units.iter().map(|u| u.unit_id.clone()).collect();
            let mut locators = Self::load_unit_locators(conn, &unit_ids)
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            for unit in &mut units {
                unit.locators = locators.remove(&unit.unit_id).unwrap_or_default();
            }
            units.retain(|unit| {
                constraints
                    .iter()
                    .all(|(key, value)| unit.locators.get(key) == Some(value))
            });
            Ok(units)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn fetch_doc_units_by_label_norm(
        &self,
        doc_node_id: &str,
        label_norm: &str,
    ) -> Result<Vec<DocUnitRecord>, AxonMindError> {
        let doc_id = doc_node_id.to_owned();
        let label_norm = label_norm.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<DocUnitRecord>, AxonMindError> {
            let sql = format!(
                "SELECT {} FROM doc_units WHERE doc_node_id = ?1 AND label_norm = ?2",
                Self::DOC_UNIT_COLUMNS
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let mut units = stmt
                .query_map(rusqlite::params![doc_id, label_norm], |row| {
                    Self::row_to_doc_unit(row)
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let unit_ids: Vec<String> = units.iter().map(|u| u.unit_id.clone()).collect();
            let mut locators = Self::load_unit_locators(conn, &unit_ids)
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            for unit in &mut units {
                unit.locators = locators.remove(&unit.unit_id).unwrap_or_default();
            }
            Ok(units)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn fetch_child_doc_units(
        &self,
        parent_unit_id: &str,
    ) -> Result<Vec<DocUnitRecord>, AxonMindError> {
        let parent_unit_id = parent_unit_id.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<DocUnitRecord>, AxonMindError> {
            let sql = format!(
                "SELECT {} FROM doc_units WHERE parent_unit_id = ?1 ORDER BY ordinal",
                Self::DOC_UNIT_COLUMNS
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let mut units = stmt
                .query_map([&parent_unit_id], |row| Self::row_to_doc_unit(row))
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let unit_ids: Vec<String> = units.iter().map(|u| u.unit_id.clone()).collect();
            let mut locators = Self::load_unit_locators(conn, &unit_ids)
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            for unit in &mut units {
                unit.locators = locators.remove(&unit.unit_id).unwrap_or_default();
            }
            Ok(units)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn fetch_doc_unit_refs(
        &self,
        from_unit_ids: &[String],
    ) -> Result<Vec<UnitRefRecord>, AxonMindError> {
        if from_unit_ids.is_empty() {
            return Ok(vec![]);
        }
        let ids = from_unit_ids.to_vec();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<UnitRefRecord>, AxonMindError> {
            let placeholders = ids
                .iter()
                .enumerate()
                .map(|(idx, _)| format!("?{}", idx + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "SELECT from_unit_id, to_doc_node_id, target_label, target_label_norm, ref_text, target_kind
                 FROM doc_unit_refs
                 WHERE from_unit_id IN ({placeholders})"
            );
            let mut stmt = conn
                .prepare(&sql)
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let params: Vec<&dyn rusqlite::ToSql> =
                ids.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            stmt.query_map(params.as_slice(), |row| {
                Ok(UnitRefRecord {
                    from_unit_id: row.get(0)?,
                    to_doc_node_id: row.get(1)?,
                    target_label: row.get(2)?,
                    target_label_norm: row.get(3)?,
                    ref_text: row.get(4)?,
                    target_kind: row.get(5)?,
                })
            })
            .map_err(|e| AxonMindError::Database(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| AxonMindError::Database(e.to_string()))
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// The Document node `document_cache` currently points at for `path` (the live HEAD), if the
    /// path is known. Feeds the ingest identity resolver (§1 matrix, path-first).
    pub(crate) async fn document_cache_node_for_path(
        &self,
        path_str: &str,
    ) -> Result<Option<NodeId>, AxonMindError> {
        let p = path_str.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<Option<NodeId>, AxonMindError> {
            conn.query_row(
                "SELECT node_id FROM document_cache WHERE path = ?1",
                [&p],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map(|opt| opt.map(NodeId))
            .map_err(|e| AxonMindError::Database(e.to_string()))
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

    /// Node IDs this document contributes to, either via `MentionedIn` or via evidence-backed
    /// relation edges. Captured before a document is deleted so the caller can sweep any nodes
    /// that become orphaned afterward.
    pub(crate) async fn fetch_document_related_node_ids(
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
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT node_id
                     FROM (
                         SELECT to_id AS node_id
                         FROM edges
                         WHERE from_id = ?1 AND kind = 'MentionedIn'
                         UNION
                         SELECT ed.from_id AS node_id
                         FROM evidence ev
                         JOIN edge_evidence ee ON ee.evidence_id = ev.id
                         JOIN edges ed ON ed.id = ee.edge_id
                         WHERE ev.source_node_id = ?1
                         UNION
                         SELECT ed.to_id AS node_id
                         FROM evidence ev
                         JOIN edge_evidence ee ON ee.evidence_id = ev.id
                         JOIN edges ed ON ed.id = ee.edge_id
                         WHERE ev.source_node_id = ?1
                     )
                     WHERE node_id <> ?1",
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

    /// How many documents still reference a blob `sha256`, counting BOTH the live
    /// `document_cache` and the retained `document_versions` log (§6). Used to decide whether a
    /// blob is safe to delete on document removal (don't delete a shared or still-versioned blob).
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
            // Count BOTH the live cache and the retained version log (§6): a blob is only
            // GC-eligible when no version — current or historical — still references its hash.
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM (
                         SELECT sha256 FROM document_cache    WHERE sha256 = ?1
                         UNION ALL
                         SELECT sha256 FROM document_versions WHERE sha256 = ?1
                     )",
                    [&sha],
                    |row| row.get(0),
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(n as usize)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn count_trash_with_sha(&self, sha256: &str) -> Result<usize, AxonMindError> {
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
                    "SELECT COUNT(*) FROM document_trash_blob WHERE sha256 = ?1",
                    [&sha],
                    |row| row.get(0),
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(n as usize)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn reconcile_ingest_status_on_startup(&self) -> Result<(), AxonMindError> {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            conn.execute(
                "UPDATE document_ingest_status
                 SET status = 'interrupted', updated_at = strftime('%s','now')
                 WHERE status IN ('processing', 'stalled')",
                [],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn upsert_ingest_status(
        &self,
        row: &IngestStatusRow,
    ) -> Result<(), AxonMindError> {
        let row = row.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            let status = to_db_str(&row.status)?;
            let phase = to_db_str(&row.phase)?;
            conn.execute(
                "INSERT INTO document_ingest_status
                    (source_path, name, content_sha256, job_id, status, phase, error, started_at, updated_at, trashed_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                 ON CONFLICT(source_path) DO UPDATE SET
                    name=excluded.name,
                    content_sha256=excluded.content_sha256,
                    job_id=excluded.job_id,
                    status=excluded.status,
                    phase=excluded.phase,
                    error=excluded.error,
                    started_at=excluded.started_at,
                    updated_at=excluded.updated_at,
                    trashed_at=excluded.trashed_at",
                rusqlite::params![
                    row.source_path,
                    row.name,
                    row.content_sha256,
                    row.job_id,
                    status,
                    phase,
                    row.error,
                    row.started_at,
                    row.updated_at,
                    row.trashed_at,
                ],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn delete_ingest_status(
        &self,
        source_path: &str,
    ) -> Result<(), AxonMindError> {
        let path = source_path.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            conn.execute(
                "DELETE FROM document_ingest_status WHERE source_path = ?1",
                [&path],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn list_ingest_statuses(&self) -> Result<Vec<IngestStatusRow>, AxonMindError> {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<IngestStatusRow>, AxonMindError> {
            let mut stmt = conn
                .prepare(
                    "SELECT source_path, name, content_sha256, job_id, status, phase, error,
                            started_at, updated_at, trashed_at
                     FROM document_ingest_status
                     WHERE trashed_at IS NULL
                     ORDER BY updated_at DESC, source_path ASC",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let rows = stmt
                .query_map([], |row| {
                    let status: String = row.get(4)?;
                    let phase: String = row.get(5)?;
                    Ok(IngestStatusRow {
                        source_path: row.get(0)?,
                        name: row.get(1)?,
                        content_sha256: row.get(2)?,
                        job_id: row.get(3)?,
                        status: from_db_str(&status).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                4,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?,
                        phase: from_db_str(&phase).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                5,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?,
                        error: row.get(6)?,
                        started_at: row.get(7)?,
                        updated_at: row.get(8)?,
                        trashed_at: row.get(9)?,
                    })
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(rows)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn fetch_ingest_status(
        &self,
        source_path: &str,
    ) -> Result<Option<IngestStatusRow>, AxonMindError> {
        let path = source_path.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(
            move |conn| -> Result<Option<IngestStatusRow>, AxonMindError> {
                conn.query_row(
                    "SELECT source_path, name, content_sha256, job_id, status, phase, error,
                        started_at, updated_at, trashed_at
                 FROM document_ingest_status WHERE source_path = ?1",
                    [&path],
                    |row| {
                        let status: String = row.get(4)?;
                        let phase: String = row.get(5)?;
                        Ok(IngestStatusRow {
                            source_path: row.get(0)?,
                            name: row.get(1)?,
                            content_sha256: row.get(2)?,
                            job_id: row.get(3)?,
                            status: from_db_str(&status).map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?,
                            phase: from_db_str(&phase).map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    5,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?,
                            error: row.get(6)?,
                            started_at: row.get(7)?,
                            updated_at: row.get(8)?,
                            trashed_at: row.get(9)?,
                        })
                    },
                )
                .optional()
                .map_err(|e| AxonMindError::Database(e.to_string()))
            },
        )
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn trash_ingest_status(
        &self,
        source_path: &str,
        trashed_at: i64,
    ) -> Result<(), AxonMindError> {
        let path = source_path.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            conn.execute(
                "UPDATE document_ingest_status SET trashed_at = ?2, updated_at = ?2 WHERE source_path = ?1",
                rusqlite::params![path, trashed_at],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn restore_ingest_status(
        &self,
        source_path: &str,
    ) -> Result<(), AxonMindError> {
        let path = source_path.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            conn.execute(
                "UPDATE document_ingest_status
                 SET trashed_at = NULL, updated_at = strftime('%s','now')
                 WHERE source_path = ?1",
                [&path],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn delete_trashed_ingest_status(
        &self,
        source_path: &str,
    ) -> Result<(), AxonMindError> {
        let path = source_path.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            conn.execute(
                "DELETE FROM document_ingest_status WHERE source_path = ?1 AND trashed_at IS NOT NULL",
                [&path],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn upsert_document_trash(
        &self,
        source_path: &str,
        name: &str,
        head_sha256: &str,
        trashed_at: i64,
        shas: &[String],
    ) -> Result<(), AxonMindError> {
        let path = source_path.to_owned();
        let name = name.to_owned();
        let head = head_sha256.to_owned();
        let shas = shas.to_vec();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            let tx = conn
                .transaction()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            tx.execute(
                "INSERT INTO document_trash (source_path, name, head_sha256, trashed_at)
                 VALUES (?1,?2,?3,?4)
                 ON CONFLICT(source_path) DO UPDATE SET
                    name=excluded.name,
                    head_sha256=excluded.head_sha256,
                    trashed_at=excluded.trashed_at",
                rusqlite::params![path, name, head, trashed_at],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            tx.execute(
                "DELETE FROM document_trash_blob WHERE source_path = ?1",
                rusqlite::params![path],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            for sha in shas {
                tx.execute(
                    "INSERT INTO document_trash_blob (source_path, sha256) VALUES (?1,?2)",
                    rusqlite::params![path, sha],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            }
            tx.commit()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn fetch_document_trash(
        &self,
        source_path: &str,
    ) -> Result<Option<(TrashRow, Vec<String>)>, AxonMindError> {
        let path = source_path.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Option<(TrashRow, Vec<String>)>, AxonMindError> {
            let row = conn
                .query_row(
                    "SELECT source_path, name, trashed_at, head_sha256
                     FROM document_trash WHERE source_path = ?1",
                    [&path],
                    |row| {
                        Ok(TrashRow {
                            source_path: row.get(0)?,
                            name: row.get(1)?,
                            trashed_at: row.get(2)?,
                            kind: TrashedItemKind::CompletedDocument,
                            head_sha256: Some(row.get(3)?),
                            status: None,
                            error: None,
                        })
                    },
                )
                .optional()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let Some(row) = row else {
                return Ok(None);
            };
            let mut stmt = conn
                .prepare(
                    "SELECT sha256 FROM document_trash_blob WHERE source_path = ?1 ORDER BY sha256 ASC",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let shas = stmt
                .query_map([&path], |row| row.get(0))
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<String>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(Some((row, shas)))
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn delete_document_trash(
        &self,
        source_path: &str,
    ) -> Result<(), AxonMindError> {
        let path = source_path.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            conn.execute("DELETE FROM document_trash WHERE source_path = ?1", [&path])
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn list_trash_rows(&self) -> Result<Vec<TrashRow>, AxonMindError> {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<TrashRow>, AxonMindError> {
            let mut rows = Vec::new();

            let mut completed = conn
                .prepare(
                    "SELECT source_path, name, trashed_at, head_sha256
                     FROM document_trash
                     ORDER BY trashed_at DESC, source_path ASC",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let completed_rows = completed
                .query_map([], |row| {
                    Ok(TrashRow {
                        source_path: row.get(0)?,
                        name: row.get(1)?,
                        trashed_at: row.get(2)?,
                        kind: TrashedItemKind::CompletedDocument,
                        head_sha256: Some(row.get(3)?),
                        status: None,
                        error: None,
                    })
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            rows.extend(completed_rows);

            let mut ingest = conn
                .prepare(
                    "SELECT source_path, name, trashed_at, status, error
                     FROM document_ingest_status
                     WHERE trashed_at IS NOT NULL
                     ORDER BY trashed_at DESC, source_path ASC",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let ingest_rows = ingest
                .query_map([], |row| {
                    let status: String = row.get(3)?;
                    Ok(TrashRow {
                        source_path: row.get(0)?,
                        name: row.get(1)?,
                        trashed_at: row.get(2)?,
                        kind: TrashedItemKind::IngestRow,
                        head_sha256: None,
                        status: Some(from_db_str(&status).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                3,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?),
                        error: row.get(4)?,
                    })
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            rows.extend(ingest_rows);

            rows.sort_by(|a, b| {
                b.trashed_at
                    .cmp(&a.trashed_at)
                    .then_with(|| a.source_path.cmp(&b.source_path))
            });
            Ok(rows)
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    pub(crate) async fn fetch_live_document_by_source_path(
        &self,
        source_path: &str,
    ) -> Result<Option<DocumentSummary>, AxonMindError> {
        let path = source_path.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Option<DocumentSummary>, AxonMindError> {
            conn.query_row(
                "SELECT n.id, n.name, dv.source_path, dv.sha256, dv.indexed_at, \
                        (SELECT COUNT(*) FROM edges e WHERE e.from_id = n.id AND e.kind = 'MentionedIn') AS concept_count, \
                        (SELECT COUNT(*) FROM evidence ev WHERE ev.source_node_id = n.id) AS evidence_count, \
                        dv.logical_doc_id, dv.version_no, \
                        (SELECT COUNT(*) FROM document_versions dv2 WHERE dv2.logical_doc_id = dv.logical_doc_id) AS version_count \
                 FROM document_versions dv
                 JOIN nodes n ON n.id = dv.node_id
                 WHERE dv.superseded = 0 AND n.kind = 'Document' AND dv.source_path = ?1",
                [&path],
                |row| {
                    Ok(DocumentSummary {
                        node_id: row.get(0)?,
                        name: row.get(1)?,
                        source_path: row.get(2)?,
                        sha256: row.get(3)?,
                        indexed_at: row.get(4)?,
                        concept_count: row.get::<_, i64>(5)? as usize,
                        evidence_count: row.get::<_, i64>(6)? as usize,
                        logical_doc_id: row.get(7)?,
                        version_no: row.get(8)?,
                        version_count: row.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(|e| AxonMindError::Database(e.to_string()))
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// One row per *logical* document — HEAD only — with extraction counts and version metadata,
    /// for the file-list UI (§ list_documents). Driven by `document_versions` (the authoritative
    /// log): the HEAD is the single `superseded = 0` row per logical doc, so superseded versions
    /// are excluded automatically (live-graph filter, §3). `version_count` is the total across the
    /// chain. Every Document node has a HEAD row after backfill, so none are dropped.
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
                "SELECT n.id, n.name, dv.source_path, dv.sha256, dv.indexed_at, \
                        (SELECT COUNT(*) FROM edges e WHERE e.from_id = n.id AND e.kind = 'MentionedIn') AS concept_count, \
                        (SELECT COUNT(*) FROM evidence ev WHERE ev.source_node_id = n.id) AS evidence_count, \
                        dv.logical_doc_id, dv.version_no, \
                        (SELECT COUNT(*) FROM document_versions dv2 WHERE dv2.logical_doc_id = dv.logical_doc_id) AS version_count \
                 FROM document_versions dv \
                 JOIN nodes n ON n.id = dv.node_id \
                 WHERE dv.superseded = 0 AND n.kind = 'Document' \
                 ORDER BY dv.indexed_at DESC",
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
                    logical_doc_id: row.get(7)?,
                    version_no: row.get(8)?,
                    version_count: row.get(9)?,
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

    /// All versions of a logical document, newest→oldest (§ list_document_versions).
    pub(crate) async fn list_document_versions(
        &self,
        logical_doc_id: &str,
    ) -> Result<Vec<DocumentVersion>, AxonMindError> {
        let ldoc = logical_doc_id.to_owned();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<Vec<DocumentVersion>, AxonMindError> {
            let mut stmt = conn
                .prepare(
                    "SELECT logical_doc_id, version_no, node_id, sha256, structural_sha256, \
                            indexed_at, source_path, previous_node_id, superseded \
                     FROM document_versions WHERE logical_doc_id = ?1 \
                     ORDER BY version_no DESC",
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            let x = stmt
                .query_map([&ldoc], |row| {
                    Ok(DocumentVersion {
                        logical_doc_id: row.get(0)?,
                        version_no: row.get(1)?,
                        node_id: row.get(2)?,
                        sha256: row.get(3)?,
                        structural_sha256: row.get(4)?,
                        indexed_at: row.get(5)?,
                        source_path: row.get(6)?,
                        previous_node_id: row.get(7)?,
                        superseded: row.get::<_, i64>(8)? != 0,
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

    /// The full `document_versions` row for `node_id`, if any. Used by the supersede path to
    /// re-insert the prior HEAD's row (flagged superseded) after its node is deleted+retained —
    /// deletion cascades the original row away (FK ON DELETE CASCADE), so it must be rewritten.
    pub(crate) async fn fetch_version(
        &self,
        node_id: &NodeId,
    ) -> Result<Option<DocumentVersion>, AxonMindError> {
        let id = node_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(
            move |conn| -> Result<Option<DocumentVersion>, AxonMindError> {
                conn.query_row(
                    "SELECT logical_doc_id, version_no, node_id, sha256, structural_sha256, \
                        indexed_at, source_path, previous_node_id, superseded \
                 FROM document_versions WHERE node_id = ?1",
                    [&id],
                    |row| {
                        Ok(DocumentVersion {
                            logical_doc_id: row.get(0)?,
                            version_no: row.get(1)?,
                            node_id: row.get(2)?,
                            sha256: row.get(3)?,
                            structural_sha256: row.get(4)?,
                            indexed_at: row.get(5)?,
                            source_path: row.get(6)?,
                            previous_node_id: row.get(7)?,
                            superseded: row.get::<_, i64>(8)? != 0,
                        })
                    },
                )
                .optional()
                .map_err(|e| AxonMindError::Database(e.to_string()))
            },
        )
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// The `(logical_doc_id, version_no)` of the version row for `node_id`, if any. Used by the
    /// ingest identity resolver to inherit a logical id and compute the next version number from
    /// the current HEAD node (the node `document_cache` points at).
    pub(crate) async fn fetch_version_for_node(
        &self,
        node_id: &NodeId,
    ) -> Result<Option<(String, i64)>, AxonMindError> {
        let id = node_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(
            move |conn| -> Result<Option<(String, i64)>, AxonMindError> {
                conn.query_row(
                    "SELECT logical_doc_id, version_no FROM document_versions WHERE node_id = ?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|e| AxonMindError::Database(e.to_string()))
            },
        )
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Backfill `document_versions` for any pre-existing Document node that has no version row:
    /// each becomes a fresh logical doc at v1 (§ Migration/existing data). Returns
    /// `(backfilled, total_documents)`. Past lineage is unrecoverable and never fabricated; the
    /// caller logs the count (Rule 12).
    pub(crate) async fn backfill_document_versions(&self) -> Result<(usize, usize), AxonMindError> {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(move |conn| -> Result<(usize, usize), AxonMindError> {
            let tx = conn
                .transaction()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            // Document nodes lacking a version row, with their cache path/fingerprint if present.
            let rows: Vec<(String, Option<String>, Option<String>, Option<String>, Option<i64>, i64)> = {
                let mut stmt = tx
                    .prepare(
                        "SELECT n.id, dc.path, dc.sha256, dc.structural_sha256, dc.indexed_at, n.created_at \
                         FROM nodes n \
                         LEFT JOIN document_cache dc ON dc.node_id = n.id \
                         WHERE n.kind = 'Document' \
                           AND NOT EXISTS (SELECT 1 FROM document_versions dv WHERE dv.node_id = n.id)",
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                stmt.query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?
            };

            let total: i64 = tx
                .query_row("SELECT COUNT(*) FROM nodes WHERE kind = 'Document'", [], |r| r.get(0))
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            let backfilled = rows.len();
            for (node_id, path, sha, structural, indexed_at, created_at) in rows {
                // Orphans (not in cache) have no recorded sha; fall back to the node's stored attr
                // (parsed in Rust — attrs is JSON TEXT — to avoid a JSON1 SQLite dependency).
                let sha = match sha {
                    Some(s) => s,
                    None => tx
                        .query_row(
                            "SELECT attrs FROM nodes WHERE id = ?1",
                            [&node_id],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()
                        .map_err(|e| AxonMindError::Database(e.to_string()))?
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                        .and_then(|v| {
                            v.get("sha256").and_then(|x| x.as_str().map(str::to_owned))
                        })
                        .unwrap_or_default(),
                };
                let logical = format!("ldoc.{}", uuid::Uuid::new_v4());
                tx.execute(
                    "INSERT INTO document_versions
                        (logical_doc_id, version_no, node_id, sha256, structural_sha256,
                         indexed_at, source_path, previous_node_id, superseded)
                     VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, NULL, 0)",
                    rusqlite::params![
                        logical,
                        node_id,
                        sha,
                        structural,
                        indexed_at.unwrap_or(created_at),
                        path,
                    ],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            }
            tx.commit()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok((backfilled, total as usize))
        })
        .await
        .map_err(|e| AxonMindError::Database(format!("interact: {e}")))?
    }

    /// Count distinct Document nodes that contribute evidence or a `MentionedIn` edge to
    /// `node_id`.
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
                    "SELECT COUNT(DISTINCT doc_id)
                     FROM (
                         SELECT e.from_id AS doc_id
                         FROM edges e
                         JOIN nodes n ON n.id = e.from_id
                         WHERE e.to_id = ?1
                           AND e.kind = 'MentionedIn'
                           AND n.kind = 'Document'
                         UNION
                         SELECT ev.source_node_id AS doc_id
                         FROM evidence ev
                         JOIN edge_evidence ee ON ee.evidence_id = ev.id
                         JOIN edges ed ON ed.id = ee.edge_id
                         JOIN nodes n ON n.id = ev.source_node_id
                         WHERE (ed.from_id = ?1 OR ed.to_id = ?1)
                           AND n.kind = 'Document'
                     )",
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
            // Nodes — ORDER BY id for stable, diff-friendly export output. Superseded version
            // nodes are excluded so the export reflects only the live (HEAD) graph (§3); their
            // bytes/lineage are retained in blobs + document_versions for audit, not here.
            let nodes: Vec<Node> = {
                let mut stmt = conn.prepare(
                    "SELECT id,kind,name,attrs,confidence,is_tainted,requires_human_review,
                            created_at,updated_at FROM nodes
                     WHERE id NOT IN (SELECT node_id FROM document_versions WHERE superseded = 1)
                     ORDER BY id"
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
    /// emit one). If `fetch_document_related_node_ids` returned duplicates, `remove_document`
    /// would try to delete that concept twice and fail with NodeNotFound.
    #[tokio::test]
    async fn fetch_document_related_node_ids_dedupes_duplicate_edges() {
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
            .fetch_document_related_node_ids(&NodeId("doc.x".to_owned()))
            .await
            .unwrap();
        assert_eq!(
            mentioned,
            vec![NodeId("kpi.dr".to_owned())],
            "duplicate MentionedIn edges must collapse to one id"
        );
    }

    /// WHY: some document-derived nodes may only remain discoverable through evidence-backed
    /// relation edges. Removal must still sweep them, or deleting and re-adding the same file can
    /// stack duplicate concepts/evidence onto a reused content-hash document id.
    #[tokio::test]
    async fn fetch_document_related_node_ids_includes_relation_endpoints() {
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
                node: node("kpi.alpha"),
            },
        )
        .await
        .unwrap();
        apply(
            &store,
            &cache,
            &tx,
            GraphMutation::UpsertNode {
                node: node("risk.beta"),
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
            GraphMutation::UpsertEdge {
                edge: Edge {
                    evidence: vec![EvidenceId("ev1".to_owned())],
                    ..edge("rel1", "kpi.alpha", "risk.beta")
                },
                evidence_ids: vec![EvidenceId("ev1".to_owned())],
            },
        )
        .await
        .unwrap();

        let mut related = store
            .fetch_document_related_node_ids(&NodeId("doc.x".to_owned()))
            .await
            .unwrap();
        related.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            related,
            vec![
                NodeId("kpi.alpha".to_owned()),
                NodeId("risk.beta".to_owned())
            ]
        );
        assert_eq!(
            store
                .count_source_documents_for_node(&NodeId("kpi.alpha".to_owned()))
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .count_source_documents_for_node(&NodeId("risk.beta".to_owned()))
                .await
                .unwrap(),
            1
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

    #[tokio::test]
    async fn reingest_upsert_does_not_clobber_an_existing_pinned_profile() {
        // pinned_profile is a user pin that must "survive re-ingestion"
        // (docs/structure_packages.md). Automated re-ingest always upserts with
        // pinned_profile: None, so the upsert itself must leave an existing pin
        // untouched rather than overwrite it with the incoming NULL.
        let dir = TempDir::new().unwrap();
        let (store, _cache, _tx) = open_store(&dir).await;

        let identity = DocumentIdentityRecord {
            doc_node_id: "doc.gdpr".to_string(),
            source_filename: "gdpr.pdf".to_string(),
            source_path: None,
            raw_title: None,
            canonical_title: "Regulation (EU) 2016/679".to_string(),
            language: Some("en".to_string()),
            jurisdiction: vec!["EU".to_string()],
            domain: vec![],
            instrument_type: Some("regulation".to_string()),
            corpus: vec!["gdpr".to_string()],
            confidence: 0.98,
            reviewed_at: None,
            updated_at: chrono::Utc::now().timestamp(),
            pinned_profile: None,
            aliases: vec![],
        };
        store.upsert_document_identity(&identity).await.unwrap();

        // Simulate a user pinning the profile out-of-band (the future pin API).
        let conn = store.db.0.get().await.unwrap();
        conn.interact(|conn| {
            conn.execute(
                "UPDATE document_identity SET pinned_profile = ?1 WHERE doc_node_id = ?2",
                rusqlite::params!["legal-eu-privacy/eu-regulation-en", "doc.gdpr"],
            )
        })
        .await
        .unwrap()
        .unwrap();

        // Re-ingestion re-derives identity automatically and upserts again with
        // pinned_profile: None — this must not erase the pin set above.
        store.upsert_document_identity(&identity).await.unwrap();

        let fetched = store
            .fetch_document_identity("doc.gdpr")
            .await
            .unwrap()
            .expect("identity row");
        assert_eq!(
            fetched.pinned_profile.as_deref(),
            Some("legal-eu-privacy/eu-regulation-en")
        );
    }

    #[tokio::test]
    async fn reingest_replaces_a_stale_document_markdown_cache_row() {
        // document_markdown is keyed by the source blob's sha256, which does NOT change when
        // converter code changes — a re-ingest's upsert is the ONLY path a converter fix has
        // to an already-cached document. An earlier INSERT OR IGNORE froze every row at its
        // first-ever render: live, doc.b515d17b's cache stayed at its 2026-07-04 pre-fix
        // bullet render through two remove+re-adds, which would have made retro_apply wipe
        // the correctly-parsed paragraph units (retrieve_guarantee.md, 2026-07-13 findings).
        let dir = TempDir::new().unwrap();
        let (store, _cache, _tx) = open_store(&dir).await;

        store
            .upsert_document_markdown("sha-blob", "- In the case of a personal data breach")
            .await
            .unwrap();
        store
            .upsert_document_markdown("sha-blob", "1. In the case of a personal data breach")
            .await
            .unwrap();

        let cached = store.get_document_markdown("sha-blob").await.unwrap();
        assert_eq!(
            cached.as_deref(),
            Some("1. In the case of a personal data breach"),
            "second render for the same blob sha must replace the cached row, not be ignored"
        );
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
