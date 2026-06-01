use super::{EdgeWithNodes, FocusKpiInput, FocusKpiOutput};
use crate::store::{GraphCache, GraphStore};
use axonmind_core::{AxonMindError, EdgeKind, NodeKind};

pub async fn focus_kpi(
    input: FocusKpiInput,
    store: &GraphStore,
    _cache: &tokio::sync::RwLock<GraphCache>,
) -> Result<FocusKpiOutput, AxonMindError> {
    let kpi = store
        .fetch_node(&input.kpi_id)
        .await?
        .ok_or_else(|| AxonMindError::NodeNotFound(input.kpi_id.clone()))?;

    if kpi.kind != NodeKind::Kpi {
        return Err(AxonMindError::NotAKpi(input.kpi_id.clone()));
    }

    let incoming = store.fetch_incoming_edges(&input.kpi_id).await?;
    let outgoing = store.fetch_outgoing_edges(&input.kpi_id).await?;

    let mut drivers = Vec::new();
    let mut blockers = Vec::new();
    let mut owner = None;

    for edge in &incoming {
        match edge.kind {
            EdgeKind::Influences | EdgeKind::Improves | EdgeKind::Causes => {
                if let Some(from_node) = store.fetch_node(&edge.from).await? {
                    drivers.push(EdgeWithNodes {
                        edge: edge.clone(),
                        from: from_node,
                        to: kpi.clone(),
                    });
                }
            }
            EdgeKind::Blocks | EdgeKind::Degrades => {
                if let Some(from_node) = store.fetch_node(&edge.from).await? {
                    blockers.push(EdgeWithNodes {
                        edge: edge.clone(),
                        from: from_node,
                        to: kpi.clone(),
                    });
                }
            }
            _ => {}
        }
    }

    let mut risks = Vec::new();
    for edge in &outgoing {
        if edge.kind == EdgeKind::OwnedBy {
            owner = store.fetch_node(&edge.to).await?;
            continue;
        }
        if let Some(to_node) = store.fetch_node(&edge.to).await? {
            if to_node.kind == NodeKind::Risk {
                risks.push(EdgeWithNodes {
                    edge: edge.clone(),
                    from: kpi.clone(),
                    to: to_node,
                });
            }
        }
    }

    let evidence_count = store.count_evidence_for_node(&input.kpi_id).await?;

    Ok(FocusKpiOutput {
        kpi,
        drivers,
        blockers,
        risks,
        owner,
        evidence_count,
    })
}
