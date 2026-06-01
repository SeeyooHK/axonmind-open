use axonmind_core::{AxonMindError, EdgeId, EdgeKind, NodeId};
use petgraph::stable_graph::{EdgeIndex, NodeIndex, StableGraph};
use std::collections::HashMap;

/// In-memory graph cache rebuilt from SQLite on engine start.
/// All read queries (`focus_kpi`, `impact_radius`, etc.) hit this cache.
/// Writes go through `GraphStore::apply_mutation`, which patches the cache after DB commit.
///
/// If `dirty` is true, the cache must be rebuilt before serving graph queries.
/// Set by `mark_dirty()` when a cache patch fails after a successful DB commit.
pub struct GraphCache {
    pub graph: StableGraph<NodeId, EdgeKind>,
    pub node_indices: HashMap<NodeId, NodeIndex>,
    pub edge_indices: HashMap<EdgeId, EdgeIndex>,
    dirty: bool,
}

impl GraphCache {
    pub fn new() -> Self {
        Self {
            graph: StableGraph::new(),
            node_indices: HashMap::new(),
            edge_indices: HashMap::new(),
            dirty: false,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Rebuild from all nodes and edges currently in SQLite.
    /// Called on engine start and after dirty-flag recovery.
    pub async fn rebuild_from_db(&mut self, db: &super::GraphDb) -> Result<(), AxonMindError> {
        let conn =
            db.0.get()
                .await
                .map_err(|e| AxonMindError::Database(format!("get conn: {e}")))?;

        let (node_ids, edge_rows) =
            conn
                .interact(
                    |conn| -> Result<
                        (Vec<String>, Vec<(String, String, String, String)>),
                        AxonMindError,
                    > {
                        let mut stmt = conn
                            .prepare("SELECT id FROM nodes")
                            .map_err(|e| AxonMindError::Database(e.to_string()))?;
                        let node_ids: Vec<String> = stmt
                            .query_map([], |row| row.get(0))
                            .map_err(|e| AxonMindError::Database(e.to_string()))?
                            .collect::<rusqlite::Result<_>>()
                            .map_err(|e| AxonMindError::Database(e.to_string()))?;

                        let mut stmt = conn
                            .prepare("SELECT id, from_id, to_id, kind FROM edges")
                            .map_err(|e| AxonMindError::Database(e.to_string()))?;
                        let edge_rows: Vec<(String, String, String, String)> = stmt
                            .query_map([], |row| {
                                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                            })
                            .map_err(|e| AxonMindError::Database(e.to_string()))?
                            .collect::<rusqlite::Result<_>>()
                            .map_err(|e| AxonMindError::Database(e.to_string()))?;

                        Ok((node_ids, edge_rows))
                    },
                )
                .await
                .map_err(|e| AxonMindError::Database(format!("interact: {e}")))??;

        self.graph.clear();
        self.node_indices.clear();
        self.edge_indices.clear();

        for id in &node_ids {
            let node_id = NodeId(id.clone());
            let idx = self.graph.add_node(node_id.clone());
            self.node_indices.insert(node_id, idx);
        }

        for (id, from_id, to_id, kind_str) in &edge_rows {
            let edge_kind: EdgeKind =
                serde_json::from_value(serde_json::Value::String(kind_str.clone()))
                    .unwrap_or(EdgeKind::Influences);
            let from_idx = self.node_indices.get(&NodeId(from_id.clone())).copied();
            let to_idx = self.node_indices.get(&NodeId(to_id.clone())).copied();
            if let (Some(fi), Some(ti)) = (from_idx, to_idx) {
                let eidx = self.graph.add_edge(fi, ti, edge_kind);
                self.edge_indices.insert(EdgeId(id.clone()), eidx);
            }
        }

        self.dirty = false;
        Ok(())
    }
}

impl Default for GraphCache {
    fn default() -> Self {
        Self::new()
    }
}
