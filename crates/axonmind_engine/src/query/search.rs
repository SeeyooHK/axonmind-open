use super::{GraphSearchInput, GraphSearchOutput, SearchMatchKind};
use crate::store::GraphStore;
use axonmind_core::AxonMindError;

pub async fn graph_search(
    input: GraphSearchInput,
    store: &GraphStore,
) -> Result<GraphSearchOutput, AxonMindError> {
    let limit = input.limit.unwrap_or(20);

    // FTS5 doesn't support special characters well — sanitize the query
    let query = sanitize_fts_query(&input.query);
    if query.is_empty() {
        return Ok(GraphSearchOutput {
            nodes: vec![],
            matched_via: vec![],
        });
    }

    let node_ids = store.search_fts(&query, limit).await?;
    if node_ids.is_empty() {
        return Ok(GraphSearchOutput {
            nodes: vec![],
            matched_via: vec![],
        });
    }

    let mut nodes = store.fetch_nodes_by_ids(&node_ids).await?;

    // Filter by kind if requested
    if let Some(ref kinds) = input.kinds {
        nodes.retain(|n| kinds.contains(&n.kind));
    }

    // Phase 1: conservative — mark all three columns as potential match sources.
    // Phase 2+: use FTS5 highlight() to narrow down per result.
    let matched_via = nodes
        .iter()
        .map(|_| {
            vec![
                SearchMatchKind::Name,
                SearchMatchKind::Definition,
                SearchMatchKind::EvidenceQuote,
            ]
        })
        .collect();

    Ok(GraphSearchOutput { nodes, matched_via })
}

/// Escape characters that confuse FTS5 MATCH syntax.
fn sanitize_fts_query(q: &str) -> String {
    // Wrap each word in double quotes so FTS5 treats them as phrase literals.
    // This avoids special chars (*, :, ^, -) being interpreted as operators.
    q.split_whitespace()
        .map(|w| format!("\"{}\"", w.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}
