use std::collections::HashMap;

use axonmind_core::{Edge, Node};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::GraphExportV1;
use crate::util::slugify;

// ── Public output types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NodeChange {
    /// "(kind):(slugified_name)" — the stable identity key.
    pub logical_key: String,
    pub before: Option<Node>,
    pub after: Option<Node>,
    /// Which fields differ; populated only for Modified entries.
    pub changed_fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EdgeChange {
    /// "(from_key)->(to_key):(edge_kind)" — the stable identity key.
    pub logical_key: String,
    pub before: Option<Edge>,
    pub after: Option<Edge>,
    pub changed_fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffSection<T> {
    pub added: Vec<T>,
    pub removed: Vec<T>,
    pub modified: Vec<T>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiffCounts {
    pub nodes_added: usize,
    pub nodes_removed: usize,
    pub nodes_modified: usize,
    pub edges_added: usize,
    pub edges_removed: usize,
    pub edges_modified: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GraphDiff {
    pub before_exported_at: DateTime<Utc>,
    pub after_exported_at: DateTime<Utc>,
    pub nodes: DiffSection<NodeChange>,
    pub edges: DiffSection<EdgeChange>,
    pub summary: DiffCounts,
    /// Non-empty when the inputs were not cleanly diffable: logical-key collisions
    /// (two distinct objects map to one key) or edges whose endpoints are absent from
    /// the same export. These entries are dropped from the diff, so surface them rather
    /// than silently hiding them.
    pub warnings: Vec<String>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn diff_exports(before: &GraphExportV1, after: &GraphExportV1) -> GraphDiff {
    let mut warnings = Vec::new();

    // Build node_id → logical_key indexes for both sides (needed for edge keys).
    let before_node_idx = node_id_to_key(&before.nodes);
    let after_node_idx = node_id_to_key(&after.nodes);

    // Build logical_key → Node maps, surfacing collisions instead of silently dropping.
    let before_nodes = keyed_nodes(&before.nodes, "before", &mut warnings);
    let after_nodes = keyed_nodes(&after.nodes, "after", &mut warnings);

    // Build logical_key → Edge maps, surfacing missing endpoints and collisions.
    let before_edges = keyed_edges(&before.edges, &before_node_idx, "before", &mut warnings);
    let after_edges = keyed_edges(&after.edges, &after_node_idx, "after", &mut warnings);

    let nodes = diff_nodes(before_nodes, after_nodes);
    let edges = diff_edges(before_edges, after_edges);

    let summary = DiffCounts {
        nodes_added: nodes.added.len(),
        nodes_removed: nodes.removed.len(),
        nodes_modified: nodes.modified.len(),
        edges_added: edges.added.len(),
        edges_removed: edges.removed.len(),
        edges_modified: edges.modified.len(),
    };

    GraphDiff {
        before_exported_at: before.exported_at,
        after_exported_at: after.exported_at,
        nodes,
        edges,
        summary,
        warnings,
    }
}

// ── Identity keys ─────────────────────────────────────────────────────────────

fn node_logical_key(node: &Node) -> String {
    format!("{:?}:{}", node.kind, slugify(&node.name))
}

/// Returns `{node_id → logical_key}` used to compute edge keys.
fn node_id_to_key(nodes: &[Node]) -> HashMap<String, String> {
    nodes
        .iter()
        .map(|n| (n.id.0.clone(), node_logical_key(n)))
        .collect()
}

/// Returns `{logical_key → Node}`. Two distinct nodes mapping to the same logical key
/// is a collision: the first wins and a warning is recorded (fail loud, don't silently drop).
fn keyed_nodes(nodes: &[Node], side: &str, warnings: &mut Vec<String>) -> HashMap<String, Node> {
    let mut map = HashMap::new();
    for node in nodes {
        let key = node_logical_key(node);
        if let Some(existing) = map.get(&key) {
            let existing: &Node = existing;
            warnings.push(format!(
                "{side}: node logical-key collision on '{key}' — kept id '{}', dropped id '{}'",
                existing.id.0, node.id.0
            ));
            continue;
        }
        map.insert(key, node.clone());
    }
    map
}

/// Returns `{logical_key → Edge}`. Edges whose endpoints are absent from the node index, or
/// whose logical key collides, are dropped and recorded as warnings.
fn keyed_edges(
    edges: &[Edge],
    node_idx: &HashMap<String, String>,
    side: &str,
    warnings: &mut Vec<String>,
) -> HashMap<String, Edge> {
    let mut map = HashMap::new();
    for edge in edges {
        let from_key = match node_idx.get(&edge.from.0) {
            Some(k) => k,
            None => {
                warnings.push(format!(
                    "{side}: edge '{}' dropped — 'from' node '{}' absent from export",
                    edge.id.0, edge.from.0
                ));
                continue;
            }
        };
        let to_key = match node_idx.get(&edge.to.0) {
            Some(k) => k,
            None => {
                warnings.push(format!(
                    "{side}: edge '{}' dropped — 'to' node '{}' absent from export",
                    edge.id.0, edge.to.0
                ));
                continue;
            }
        };
        let key = format!("{from_key}->{to_key}:{:?}", edge.kind);
        if let Some(existing) = map.get(&key) {
            let existing: &Edge = existing;
            warnings.push(format!(
                "{side}: edge logical-key collision on '{key}' — kept id '{}', dropped id '{}'",
                existing.id.0, edge.id.0
            ));
            continue;
        }
        map.insert(key, edge.clone());
    }
    map
}

// ── Content fingerprinting ────────────────────────────────────────────────────

/// Quantize confidence to 3 decimal places to suppress float noise.
fn q(v: f32) -> i64 {
    (v * 1000.0).round() as i64
}

/// Attrs keys that are provenance/bookkeeping, not semantic content. They churn whenever a
/// document is re-indexed (source doc id changes) or KPIs are recomputed — comparing them would
/// report a "modified" node on every re-index even though nothing meaningful changed. Excluded
/// from the fingerprint and the field-level diff, for the same reason `created_at`/`updated_at`
/// are excluded.
const VOLATILE_ATTR_KEYS: &[&str] = &["source_refs", "last_recomputed_at"];

fn node_fingerprint(node: &Node) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", node.kind).hash(&mut h);
    node.name.hash(&mut h);
    q(node.confidence.0).hash(&mut h);
    node.is_tainted.hash(&mut h);
    node.requires_human_review.hash(&mut h);
    stable_attrs_string(&node.attrs).hash(&mut h);
    h.finish()
}

fn edge_fingerprint(edge: &Edge) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    q(edge.confidence.0).hash(&mut h);
    edge.is_tainted.hash(&mut h);
    edge.requires_human_review.hash(&mut h);
    format!("{:?}", edge.created_by).hash(&mut h);
    edge.evidence.len().hash(&mut h);
    h.finish()
}

/// Canonical (key-sorted) JSON string of `attrs` with volatile provenance keys removed,
/// so that re-index/recompute churn in `source_refs`/`last_recomputed_at` is not seen as a change.
fn stable_attrs_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut pairs: Vec<_> = map
                .iter()
                .filter(|(k, _)| !VOLATILE_ATTR_KEYS.contains(&k.as_str()))
                .collect();
            pairs.sort_by_key(|(k, _)| k.as_str());
            let rebuilt: serde_json::Map<_, _> = pairs
                .into_iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            serde_json::to_string(&serde_json::Value::Object(rebuilt)).unwrap_or_default()
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

// ── Field-level change reporting ──────────────────────────────────────────────

fn node_changed_fields(before: &Node, after: &Node) -> Vec<String> {
    let mut fields = Vec::new();
    if before.name != after.name {
        fields.push("name".into());
    }
    if format!("{:?}", before.kind) != format!("{:?}", after.kind) {
        fields.push("kind".into());
    }
    if q(before.confidence.0) != q(after.confidence.0) {
        fields.push("confidence".into());
    }
    if before.is_tainted != after.is_tainted {
        fields.push("is_tainted".into());
    }
    if before.requires_human_review != after.requires_human_review {
        fields.push("requires_human_review".into());
    }
    if stable_attrs_string(&before.attrs) != stable_attrs_string(&after.attrs) {
        fields.push("attrs".into());
    }
    fields
}

fn edge_changed_fields(before: &Edge, after: &Edge) -> Vec<String> {
    let mut fields = Vec::new();
    if q(before.confidence.0) != q(after.confidence.0) {
        fields.push("confidence".into());
    }
    if before.is_tainted != after.is_tainted {
        fields.push("is_tainted".into());
    }
    if before.requires_human_review != after.requires_human_review {
        fields.push("requires_human_review".into());
    }
    if format!("{:?}", before.created_by) != format!("{:?}", after.created_by) {
        fields.push("created_by".into());
    }
    if before.evidence.len() != after.evidence.len() {
        fields.push("evidence_count".into());
    }
    fields
}

// ── Set diff helpers ──────────────────────────────────────────────────────────

fn diff_nodes(
    before: HashMap<String, Node>,
    mut after: HashMap<String, Node>,
) -> DiffSection<NodeChange> {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();

    for (key, bv) in before {
        if let Some(av) = after.remove(&key) {
            if node_fingerprint(&bv) != node_fingerprint(&av) {
                let fields = node_changed_fields(&bv, &av);
                modified.push(NodeChange {
                    logical_key: key,
                    before: Some(bv),
                    after: Some(av),
                    changed_fields: fields,
                });
            }
        } else {
            removed.push(NodeChange {
                logical_key: key,
                before: Some(bv),
                after: None,
                changed_fields: vec![],
            });
        }
    }
    for (key, av) in after {
        added.push(NodeChange {
            logical_key: key,
            before: None,
            after: Some(av),
            changed_fields: vec![],
        });
    }

    DiffSection {
        added,
        removed,
        modified,
    }
}

fn diff_edges(
    before: HashMap<String, Edge>,
    mut after: HashMap<String, Edge>,
) -> DiffSection<EdgeChange> {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut modified = Vec::new();

    for (key, bv) in before {
        if let Some(av) = after.remove(&key) {
            if edge_fingerprint(&bv) != edge_fingerprint(&av) {
                let fields = edge_changed_fields(&bv, &av);
                modified.push(EdgeChange {
                    logical_key: key,
                    before: Some(bv),
                    after: Some(av),
                    changed_fields: fields,
                });
            }
        } else {
            removed.push(EdgeChange {
                logical_key: key,
                before: Some(bv),
                after: None,
                changed_fields: vec![],
            });
        }
    }
    for (key, av) in after {
        added.push(EdgeChange {
            logical_key: key,
            before: None,
            after: Some(av),
            changed_fields: vec![],
        });
    }

    DiffSection {
        added,
        removed,
        modified,
    }
}
