use super::{
    AffectedNode, ImpactRadiusInput, ImpactRadiusOutput, SuggestActionsInput, SuggestActionsOutput,
    TraceDecisionInput, TraceDecisionOutput,
};
use crate::store::{GraphCache, GraphStore};
use axonmind_core::{AxonMindError, NodeId};
use petgraph::Direction;
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::{HashMap, HashSet, VecDeque};

pub async fn impact_radius(
    input: ImpactRadiusInput,
    store: &GraphStore,
    cache: &tokio::sync::RwLock<GraphCache>,
) -> Result<ImpactRadiusOutput, AxonMindError> {
    let max_depth = input.max_depth.unwrap_or(3);

    let cache = cache.read().await;
    if cache.is_dirty() {
        return Err(AxonMindError::CacheDirty);
    }

    let start_idx = cache
        .node_indices
        .get(&input.node_id)
        .copied()
        .ok_or_else(|| AxonMindError::NodeNotFound(input.node_id.clone()))?;

    // BFS with depth and path tracking
    let mut visited: HashSet<NodeIndex> = HashSet::new();
    let mut queue: VecDeque<(NodeIndex, u32, Vec<NodeId>)> = VecDeque::new();
    visited.insert(start_idx);
    queue.push_back((start_idx, 0, vec![input.node_id.clone()]));

    let mut reached: Vec<(NodeId, u32, Vec<NodeId>)> = Vec::new();

    while let Some((nx, depth, path)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        for edge_ref in cache.graph.edges_directed(nx, Direction::Outgoing) {
            let neighbor = edge_ref.target();
            if visited.insert(neighbor) {
                let neighbor_id = node_index_to_id(&cache, neighbor);
                let mut new_path = path.clone();
                if let Some(ref nid) = neighbor_id {
                    new_path.push(nid.clone());
                }
                if let Some(nid) = neighbor_id {
                    reached.push((nid, depth + 1, new_path.clone()));
                }
                queue.push_back((neighbor, depth + 1, new_path));
            }
        }
    }
    drop(cache);

    // Fetch full Node structs for all reached nodes
    let node_ids: Vec<axonmind_core::NodeId> =
        reached.iter().map(|(id, _, _)| id.clone()).collect();
    let nodes_by_id: HashMap<NodeId, axonmind_core::Node> = store
        .fetch_nodes_by_ids(&node_ids)
        .await?
        .into_iter()
        .map(|n| (n.id.clone(), n))
        .collect();

    let affected = reached
        .into_iter()
        .filter_map(|(nid, depth, path)| {
            nodes_by_id.get(&nid).map(|node| AffectedNode {
                node: node.clone(),
                depth,
                path,
            })
        })
        .collect();

    Ok(ImpactRadiusOutput { affected })
}

pub async fn trace_decision(
    input: TraceDecisionInput,
    store: &GraphStore,
    _cache: &tokio::sync::RwLock<GraphCache>,
) -> Result<TraceDecisionOutput, AxonMindError> {
    use super::EdgeWithNodes;
    use axonmind_core::EdgeKind;

    let decision = store
        .fetch_node(&input.decision_node_id)
        .await?
        .ok_or_else(|| AxonMindError::NodeNotFound(input.decision_node_id.clone()))?;

    let incoming = store.fetch_incoming_edges(&input.decision_node_id).await?;
    let outgoing = store.fetch_outgoing_edges(&input.decision_node_id).await?;

    let mut caused_by = Vec::new();
    for edge in &incoming {
        if matches!(edge.kind, EdgeKind::DecidedBy | EdgeKind::Causes) {
            if let Some(from_node) = store.fetch_node(&edge.from).await? {
                caused_by.push(EdgeWithNodes {
                    edge: edge.clone(),
                    from: from_node,
                    to: decision.clone(),
                });
            }
        }
    }

    let evidenced_by = store
        .fetch_evidence_for_node(&input.decision_node_id)
        .await?;

    let mut next_actions = Vec::new();
    for edge in &outgoing {
        if edge.kind == EdgeKind::NextAction {
            if let Some(to_node) = store.fetch_node(&edge.to).await? {
                next_actions.push(EdgeWithNodes {
                    edge: edge.clone(),
                    from: decision.clone(),
                    to: to_node,
                });
            }
        }
    }

    Ok(TraceDecisionOutput {
        decision,
        caused_by,
        evidenced_by,
        next_actions,
    })
}

pub async fn suggest_actions(
    input: SuggestActionsInput,
    store: &GraphStore,
    _cache: &tokio::sync::RwLock<GraphCache>,
) -> Result<SuggestActionsOutput, AxonMindError> {
    use axonmind_core::NodeKind;

    let outgoing = store.fetch_outgoing_edges(&input.kpi_id).await?;
    let include_unreviewed = input.include_unreviewed.unwrap_or(false);

    let mut actions = Vec::new();
    for edge in &outgoing {
        if !include_unreviewed && edge.requires_human_review {
            continue;
        }
        if let Some(to_node) = store.fetch_node(&edge.to).await? {
            if to_node.kind == NodeKind::Action {
                if let Some(ref status_filter) = input.status_filter {
                    if let Some(kpi_status) = to_node.attrs.get("status").and_then(|v| {
                        serde_json::from_value::<axonmind_core::KpiStatus>(v.clone()).ok()
                    }) {
                        if !status_filter.contains(&kpi_status) {
                            continue;
                        }
                    }
                }
                actions.push(to_node);
            }
        }
    }

    Ok(SuggestActionsOutput { actions })
}

fn node_index_to_id(cache: &GraphCache, idx: NodeIndex) -> Option<NodeId> {
    // O(n) reverse lookup — acceptable for Phase 1 graph sizes.
    cache
        .node_indices
        .iter()
        .find(|&(_, &i)| i == idx)
        .map(|(id, _)| id.clone())
}
