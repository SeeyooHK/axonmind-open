use axonmind_core::{AxonMindError, Confidence, Edge, EdgeId, Evidence, EvidenceId, Node, NodeId};
/// Phase 4: generation management and scoped export.
///
/// Generations are pure metadata — they never modify nodes/edges/evidence.
/// All writes here bypass `apply_mutation` intentionally: no FTS sync, no
/// cache patch, no event emit. The generation tables are bookkeeping only.
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use super::{evidence_from_row, from_db_str, node_from_row};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GenerationId(pub String);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationSummary {
    pub id: GenerationId,
    pub name: String,
    pub created_at: i64,
    pub file_count: usize,
}

// ── GraphStore generation methods ─────────────────────────────────────────────

impl super::GraphStore {
    /// Create a new named generation. Returns its ID.
    pub async fn create_generation(&self, name: String) -> Result<GenerationId, AxonMindError> {
        let id = uuid::Uuid::new_v4().to_string();
        let id_clone = id.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<(), AxonMindError> {
            let now = chrono::Utc::now().timestamp();
            conn.execute(
                "INSERT INTO generation (id, name, created_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![id_clone, name, now],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))??;
        Ok(GenerationId(id))
    }

    /// List all generations, newest first.
    pub async fn list_generations(&self) -> Result<Vec<GenerationSummary>, AxonMindError> {
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(|conn| -> Result<Vec<GenerationSummary>, AxonMindError> {
            let mut stmt = conn.prepare(
                "SELECT g.id, g.name, g.created_at,
                        (SELECT COUNT(*) FROM generation_source gs WHERE gs.generation_id = g.id) AS file_count
                 FROM generation g ORDER BY g.created_at DESC",
            ).map_err(|e| AxonMindError::Database(e.to_string()))?;
            let x = stmt.query_map([], |r| Ok(GenerationSummary {
                id: GenerationId(r.get(0)?),
                name: r.get(1)?,
                created_at: r.get(2)?,
                file_count: r.get::<_, i64>(3)? as usize,
            }))
            .map_err(|e| AxonMindError::Database(e.to_string()))?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(x)
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    /// Record `(generation_id, path, sha256)` into generation_source and assign/look up
    /// a source_version for this `(path, sha256)` pair. Called after ingest succeeds.
    ///
    /// If `(path, sha256)` was seen before, the existing version number is reused (D6).
    /// If the path is new or the content changed, `version = max(existing for path) + 1`.
    pub async fn record_generation_source(
        &self,
        gen_id: &GenerationId,
        path: String,
        sha256: String,
    ) -> Result<i64, AxonMindError> {
        let gid = gen_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<i64, AxonMindError> {
            let tx = conn
                .transaction()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            // Determine version for (path, sha256) — D6
            let existing_version: Option<i64> = tx
                .query_row(
                    "SELECT version FROM source_version WHERE path=?1 AND sha256=?2",
                    rusqlite::params![path, sha256],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;

            let version = if let Some(v) = existing_version {
                v // reuse — covers revert case
            } else {
                let max: i64 = tx
                    .query_row(
                        "SELECT COALESCE(MAX(version), 0) FROM source_version WHERE path=?1",
                        rusqlite::params![path],
                        |r| r.get(0),
                    )
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let new_v = max + 1;
                let now = chrono::Utc::now().timestamp();
                tx.execute(
                    "INSERT INTO source_version (path, sha256, version, first_seen_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![path, sha256, new_v, now],
                )
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
                new_v
            };

            // Record in generation_source (idempotent)
            tx.execute(
                "INSERT OR IGNORE INTO generation_source (generation_id, path, sha256)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![gid, path, sha256],
            )
            .map_err(|e| AxonMindError::Database(e.to_string()))?;

            tx.commit()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
            Ok(version)
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }

    /// Scoped export for a generation (algorithm A.3 from the design doc).
    ///
    /// 1. generation_source → sha256 set
    /// 2. evidence WHERE blob_sha256 ∈ sha_set
    /// 3. edges backed by those evidence rows (via edge_evidence)
    /// 4. nodes: edge endpoints + evidence.source_node_id
    ///
    /// Nodes whose evidence has no blob_sha256 (manual/derived) only appear in the
    /// big (exportJson) view, not here — this is by design (A.3 guardrail).
    pub async fn fetch_export_for_generation(
        &self,
        gen_id: &GenerationId,
    ) -> Result<
        (
            Vec<Node>,
            Vec<Edge>,
            Vec<Evidence>,
            Vec<(EdgeId, EvidenceId)>,
        ),
        AxonMindError,
    > {
        let gid = gen_id.0.clone();
        let conn = self
            .db
            .0
            .get()
            .await
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        conn.interact(move |conn| -> Result<_, AxonMindError> {
            // Evidence in this generation
            let evidence: Vec<Evidence> = {
                let mut stmt = conn.prepare(
                    "SELECT ev.id, ev.source_node_id, ev.source_type, ev.quote, ev.row_ref,
                            ev.blob_sha256, ev.timestamp, ev.extractor, ev.confidence,
                            ev.is_tainted, ev.requires_human_review
                     FROM evidence ev
                     WHERE ev.blob_sha256 IN (
                         SELECT sha256 FROM generation_source WHERE generation_id = ?1
                     )
                     ORDER BY ev.id",
                ).map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt.query_map(rusqlite::params![gid], evidence_from_row)
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                x
            };

            let evidence_id_set: std::collections::HashSet<String> =
                evidence.iter().map(|e| e.id.0.clone()).collect();

            // edge_evidence pairs for this evidence set
            let edge_evidence_pairs: Vec<(EdgeId, EvidenceId)> = {
                let mut stmt = conn.prepare(
                    "SELECT DISTINCT ee.edge_id, ee.evidence_id
                     FROM edge_evidence ee
                     WHERE ee.evidence_id IN (
                         SELECT ev.id FROM evidence ev
                         WHERE ev.blob_sha256 IN (
                             SELECT sha256 FROM generation_source WHERE generation_id = ?1
                         )
                     )
                     ORDER BY ee.edge_id, ee.evidence_id",
                ).map_err(|e| AxonMindError::Database(e.to_string()))?;
                let x = stmt.query_map(rusqlite::params![gid], |r| {
                    Ok((
                        r.get::<_, String>(0).map(EdgeId)?,
                        r.get::<_, String>(1).map(EvidenceId)?,
                    ))
                })
                .map_err(|e| AxonMindError::Database(e.to_string()))?
                .collect::<rusqlite::Result<_>>()
                .map_err(|e| AxonMindError::Database(e.to_string()))?;
                x
            };

            let gen_edge_ids: std::collections::HashSet<String> =
                edge_evidence_pairs.iter().map(|(eid, _)| eid.0.clone()).collect();

            // Edges
            use std::collections::HashMap;
            let mut ev_by_edge: HashMap<String, Vec<EvidenceId>> = HashMap::new();
            for (eid, evid) in &edge_evidence_pairs {
                // only include evidence that's in this generation
                if evidence_id_set.contains(&evid.0) {
                    ev_by_edge.entry(eid.0.clone()).or_default().push(evid.clone());
                }
            }

            let edges: Vec<Edge> = if gen_edge_ids.is_empty() {
                vec![]
            } else {
                let sql2 = format!(
                    "SELECT id,from_id,to_id,kind,confidence,created_by,
                            is_tainted,requires_human_review,created_at
                     FROM edges WHERE id IN ({})  ORDER BY id",
                    gen_edge_ids.iter().enumerate()
                        .map(|(i, _)| format!("?{}", i + 1))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                let mut stmt = conn.prepare(&sql2)
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let edge_id_vec: Vec<String> = gen_edge_ids.into_iter().collect();
                let params2: Vec<&dyn rusqlite::ToSql> =
                    edge_id_vec.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
                let x = stmt.query_map(params2.as_slice(), |r| {
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
                    let evidence_ids = ev_by_edge.get(&eid).cloned().unwrap_or_default();
                    Ok(Edge {
                        id: EdgeId(eid),
                        from: NodeId(from),
                        to: NodeId(to),
                        kind: from_db_str(&kind_str)?,
                        confidence: Confidence(conf as f32),
                        created_at: chrono::DateTime::from_timestamp(ts, 0).unwrap_or_default(),
                        created_by: from_db_str(&by_str)?,
                        evidence: evidence_ids,
                        is_tainted: tainted,
                        requires_human_review: review,
                    })
                }).collect::<Result<Vec<_>, AxonMindError>>()?
            };

            // Node IDs: edge endpoints + evidence source nodes
            let mut node_id_set: std::collections::HashSet<String> = std::collections::HashSet::new();
            for e in &edges {
                node_id_set.insert(e.from.0.clone());
                node_id_set.insert(e.to.0.clone());
            }
            for ev in &evidence {
                node_id_set.insert(ev.source_node_id.0.clone());
            }

            // Nodes
            let nodes: Vec<Node> = if node_id_set.is_empty() {
                vec![]
            } else {
                let nids: Vec<String> = node_id_set.into_iter().collect();
                let placeholders = nids.iter().enumerate()
                    .map(|(i, _)| format!("?{}", i + 1))
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!(
                    "SELECT id,kind,name,attrs,confidence,is_tainted,requires_human_review,
                            created_at,updated_at FROM nodes WHERE id IN ({placeholders}) ORDER BY id"
                );
                let mut stmt = conn.prepare(&sql)
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                let params: Vec<&dyn rusqlite::ToSql> =
                    nids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
                let x = stmt.query_map(params.as_slice(), node_from_row)
                    .map_err(|e| AxonMindError::Database(e.to_string()))?
                    .collect::<rusqlite::Result<_>>()
                    .map_err(|e| AxonMindError::Database(e.to_string()))?;
                x
            };

            Ok((nodes, edges, evidence, edge_evidence_pairs))
        })
        .await
        .map_err(|e| AxonMindError::Database(e.to_string()))?
    }
}
