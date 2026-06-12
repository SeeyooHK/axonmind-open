use std::collections::HashMap;

use axonmind_core::{AxonMindError, Edge, EdgeKind, Evidence, Node, NodeId};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::store::{GraphCache, GraphStore};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FindConflictsInput {
    /// Restrict to conflicts touching this node. Default: scan the whole graph.
    pub node_id: Option<NodeId>,
    /// Max conflict pairs to return. Default: 50.
    pub limit: Option<usize>,
}

/// An edge together with all of its backing evidence citations.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EdgeWithEvidence {
    pub edge: Edge,
    pub evidence: Vec<Evidence>,
}

/// A pair of nodes where the graph holds contradictory claims about their relationship.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConflictPair {
    pub node_a: Node,
    pub node_b: Node,
    /// Edges with positive polarity (Improves, Corroborates) between the pair.
    pub positive: Vec<EdgeWithEvidence>,
    /// Edges with negative polarity (Degrades, Blocks, Contradicts) between the pair.
    pub negative: Vec<EdgeWithEvidence>,
    /// Max confidence across all edges in the pair — used for sorting.
    pub max_confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FindConflictsOutput {
    pub conflicts: Vec<ConflictPair>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Polarity {
    Positive,
    Negative,
}

fn polarity(kind: EdgeKind) -> Option<Polarity> {
    match kind {
        EdgeKind::Improves | EdgeKind::Corroborates => Some(Polarity::Positive),
        EdgeKind::Degrades | EdgeKind::Blocks | EdgeKind::Contradicts => Some(Polarity::Negative),
        _ => None,
    }
}

/// Canonical unordered pair key: always (lexicographically smaller id, larger id).
fn pair_key(a: &NodeId, b: &NodeId) -> (String, String) {
    if a.0 <= b.0 {
        (a.0.clone(), b.0.clone())
    } else {
        (b.0.clone(), a.0.clone())
    }
}

pub async fn find_conflicts(
    input: FindConflictsInput,
    store: &GraphStore,
    _cache: &tokio::sync::RwLock<GraphCache>,
) -> Result<FindConflictsOutput, AxonMindError> {
    let limit = input.limit.unwrap_or(50);

    // Collect candidate edges — all for the node, or whole graph.
    let edges: Vec<Edge> = match &input.node_id {
        Some(nid) => {
            // Verify the node exists.
            store
                .fetch_node(nid)
                .await?
                .ok_or_else(|| AxonMindError::NodeNotFound(nid.clone()))?;
            let mut v = store.fetch_incoming_edges(nid).await?;
            v.extend(store.fetch_outgoing_edges(nid).await?);
            v
        }
        None => store.fetch_all_edges().await?,
    };

    // Group by unordered pair; only keep edges that have a polarity.
    let mut groups: HashMap<(String, String), Vec<Edge>> = HashMap::new();
    for edge in edges {
        if polarity(edge.kind).is_some() {
            let key = pair_key(&edge.from, &edge.to);
            groups.entry(key).or_default().push(edge);
        }
    }

    // Filter to conflicted groups (Rule 1 OR Rule 2 from the plan).
    let mut conflicts = Vec::new();
    for (_key, group) in groups {
        let has_positive = group.iter().any(|e| polarity(e.kind) == Some(Polarity::Positive));
        let has_negative = group.iter().any(|e| polarity(e.kind) == Some(Polarity::Negative));
        let has_contradicts = group.iter().any(|e| e.kind == EdgeKind::Contradicts);

        if !(has_positive && has_negative) && !has_contradicts {
            continue;
        }

        // Attach evidence to each edge.
        let mut positive = Vec::new();
        let mut negative = Vec::new();
        let mut max_conf: f32 = 0.0;

        for edge in group {
            let evidence = store.fetch_evidence_for_edge(&edge.id).await?;
            max_conf = max_conf.max(edge.confidence.0);
            let ewe = EdgeWithEvidence { edge: edge.clone(), evidence };
            match polarity(edge.kind) {
                Some(Polarity::Positive) => positive.push(ewe),
                Some(Polarity::Negative) => negative.push(ewe),
                None => {}
            }
        }

        // Fetch the two node records. If a node was deleted after the edge was written, skip.
        let (a_id, b_id) = {
            let first = positive
                .first()
                .or(negative.first())
                .map(|e| (&e.edge.from, &e.edge.to))
                .unwrap();
            (first.0.clone(), first.1.clone())
        };
        // Always report as (min, max) so callers get a stable order.
        let (a_id, b_id) = if a_id.0 <= b_id.0 {
            (a_id, b_id)
        } else {
            (b_id, a_id)
        };

        let node_a = match store.fetch_node(&a_id).await? {
            Some(n) => n,
            None => continue,
        };
        let node_b = match store.fetch_node(&b_id).await? {
            Some(n) => n,
            None => continue,
        };

        conflicts.push(ConflictPair {
            node_a,
            node_b,
            positive,
            negative,
            max_confidence: max_conf,
        });
    }

    // Sort by max_confidence descending so the most certain contradictions surface first.
    conflicts.sort_by(|a, b| b.max_confidence.partial_cmp(&a.max_confidence).unwrap_or(std::cmp::Ordering::Equal));
    conflicts.truncate(limit);

    Ok(FindConflictsOutput { conflicts })
}
