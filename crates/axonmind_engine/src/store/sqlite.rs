use axonmind_core::AxonMindError;
/// Blocker D: FTS5 search_index synchronization rules.
///
/// `search_index` is a FTS5 virtual table that mirrors text fields from nodes and evidence.
/// It CANNOT be updated by SQLite triggers — it must be manually synced in Rust.
/// Every write path in `GraphStore::apply_mutation` must call the appropriate sync function below.
///
/// Sync rules (exhaustive — no exceptions):
///
/// On `GraphMutation::UpsertNode`:
///   - DELETE from search_index WHERE node_id = node.id
///   - INSERT into search_index (node_id, kind, name, definition, evidence_quotes)
///     where `definition` comes from KpiAttrs.definition if kind=Kpi, else ""
///     and `evidence_quotes` is rebuilt by querying all evidence linked to this node
///
/// On `GraphMutation::UpsertEvidence`:
///   - Recompute `evidence_quotes` for the evidence's source_node_id
///   - Also recompute for every node at either end of any edge linked to this evidence
///
/// On `GraphMutation::UpsertEdge` (after edge_evidence rows are inserted):
///   - Recompute `evidence_quotes` for edge.from and edge.to
///
/// On `GraphMutation::DeleteNode`:
///   - DELETE from search_index WHERE node_id = node_id (cascades from SQLite FK)
///
/// On `GraphMutation::DeleteEdge`:
///   - Recompute `evidence_quotes` for both endpoint nodes
///
/// On bulk import (import-json CLI command):
///   - DELETE from search_index (full wipe)
///   - Rebuild entire table by iterating all nodes + their evidence
///   - Also exposed as CLI: `axonmind rebuild-search-index --workspace <path>`
use deadpool_sqlite::{Config, Pool, Runtime};
use rusqlite::Connection;
use std::path::Path;

/// Async connection pool for the axonmind SQLite database.
/// Mirrors soverex-open's `AgentDb(pub Pool)` pattern.
#[derive(Clone)]
pub struct GraphDb(pub Pool);

impl GraphDb {
    pub async fn new(db_path: &Path) -> Result<Self, AxonMindError> {
        let config = Config::new(db_path);
        let pool = config
            .builder(Runtime::Tokio1)
            .map_err(|e| AxonMindError::Database(format!("pool builder: {e}")))?
            .post_create(deadpool_sqlite::Hook::async_fn(|conn, _| {
                Box::pin(async move {
                    conn.interact(|c| c.execute_batch("PRAGMA foreign_keys = ON;"))
                        .await
                        .map_err(|e| deadpool_sqlite::HookError::Message(e.to_string().into()))?
                        .map_err(|e| deadpool_sqlite::HookError::Message(e.to_string().into()))?;
                    Ok(())
                })
            }))
            .build()
            .map_err(|e| AxonMindError::Database(format!("pool build: {e}")))?;

        let conn = pool
            .get()
            .await
            .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;
        conn.interact(|conn| super::migrations::run_migrations(conn))
            .await
            .map_err(|e| AxonMindError::Database(format!("interact: {e}")))??;

        Ok(Self(pool))
    }
}

// ── FTS5 sync helpers (all sync — called inside conn.interact closures) ────────

/// Gather all evidence quotes associated with a node for FTS5 indexing.
/// Includes direct evidence (source_node_id) and indirect evidence via edges.
pub(super) fn gather_evidence_quotes(conn: &Connection, node_id: &str) -> rusqlite::Result<String> {
    let mut stmt = conn.prepare(
        "SELECT e.quote FROM evidence e
         WHERE e.source_node_id = ?1 AND e.quote IS NOT NULL
         UNION
         SELECT e.quote FROM evidence e
         JOIN edge_evidence ee ON e.id = ee.evidence_id
         JOIN edges ed ON ee.edge_id = ed.id
         WHERE (ed.from_id = ?1 OR ed.to_id = ?1) AND e.quote IS NOT NULL",
    )?;
    let quotes: Vec<String> = stmt
        .query_map([node_id], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(quotes.join(" | "))
}

/// Sync FTS5 entry for a single node: delete existing row, reinsert with fresh data.
/// If the node no longer exists in the nodes table, only the delete step runs.
pub(super) fn sync_node_fts(conn: &Connection, node_id: &str) -> Result<(), AxonMindError> {
    use rusqlite::OptionalExtension;
    let row: Option<(String, String, Vec<u8>)> = conn
        .query_row(
            "SELECT kind, name, attrs FROM nodes WHERE id = ?1",
            [node_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, Vec<u8>>(2)?)),
        )
        .optional()
        .map_err(|e| AxonMindError::Database(e.to_string()))?;

    conn.execute("DELETE FROM search_index WHERE node_id = ?1", [node_id])
        .map_err(|e| AxonMindError::Database(e.to_string()))?;

    let (kind, name, attrs_blob) = match row {
        None => return Ok(()),
        Some(r) => r,
    };

    let definition = if kind == "Kpi" {
        rmp_serde::from_slice::<serde_json::Value>(&attrs_blob)
            .ok()
            .and_then(|v| v.get("definition")?.as_str().map(|s| s.to_owned()))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let quotes = gather_evidence_quotes(conn, node_id)
        .map_err(|e| AxonMindError::Database(e.to_string()))?;

    conn.execute(
        "INSERT INTO search_index (node_id, kind, name, definition, evidence_quotes)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![node_id, kind, name, definition, quotes],
    )
    .map_err(|e| AxonMindError::Database(e.to_string()))?;
    Ok(())
}

/// Rebuild the entire FTS5 search_index from scratch (used by bulk import + CLI rebuild command).
pub fn rebuild_full_fts(conn: &Connection) -> Result<(), AxonMindError> {
    conn.execute_batch("DELETE FROM search_index")
        .map_err(|e| AxonMindError::Database(e.to_string()))?;
    let ids: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT id FROM nodes")
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        let x = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| AxonMindError::Database(e.to_string()))?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| AxonMindError::Database(e.to_string()))?;
        x
    };
    for id in &ids {
        sync_node_fts(conn, id)?;
    }
    Ok(())
}
