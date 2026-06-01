//! Phase 3 (E v1): deterministic cross-document concept bridging.
//!
//! After a document is extracted, its concept nodes are compared by name against concept nodes
//! already in the graph (from other documents). Near-duplicate names that the exact-slug merge
//! missed (e.g. "Risk Evaluation" vs "AI-Assisted Risk Evaluation") are linked with a
//! `CorrelatesWith` edge, so the per-document clusters knit together instead of sitting as
//! disconnected blobs.
//!
//! Matching is high-precision and deterministic — see [`names_near_match`]. Semantically
//! different relations (a deck capability ↔ an agreement constraint) are intentionally out of
//! scope here; those require the LLM semantic linker (E v2).

use super::normalize::names_near_match;
use crate::store::GraphMutation;
use axonmind_core::{
    Confidence, Edge, EdgeId, EdgeKind, Evidence, EvidenceId, ExtractorKind, Node, NodeId,
    SourceType,
};
use chrono::Utc;
use uuid::Uuid;

/// Confidence for a name-similarity bridge — heuristic, so modest and review-flagged.
const BRIDGE_CONFIDENCE: Confidence = Confidence(0.5);

/// Build `CorrelatesWith` edges (each with backing evidence) linking concept nodes from the
/// document just ingested (`new_concepts`) to near-duplicate concept nodes already in the graph
/// (`existing_concepts`). Returns mutations in application order (evidence before its edge).
///
/// `new_concepts`/`existing_concepts` are `(node_id, name)` pairs, excluding Document nodes.
/// Nodes with the same id (e.g. a re-ingested concept) are never self-linked.
pub fn build_cross_document_bridges(
    doc_node: &Node,
    doc_sha256: &str,
    new_concepts: &[(NodeId, String)],
    existing_concepts: &[(NodeId, String)],
) -> Vec<GraphMutation> {
    let mut mutations = Vec::new();

    for (new_id, new_name) in new_concepts {
        for (existing_id, existing_name) in existing_concepts {
            if new_id == existing_id || !names_near_match(new_name, existing_name) {
                continue;
            }

            let now = Utc::now();
            let ev_id = EvidenceId(Uuid::new_v4().to_string());
            mutations.push(GraphMutation::UpsertEvidence {
                evidence: Evidence {
                    id: ev_id.clone(),
                    source_node_id: doc_node.id.clone(),
                    source_type: SourceType::Document,
                    quote: Some(format!(
                        "Cross-document concept match: \"{new_name}\" ~ \"{existing_name}\""
                    )),
                    row_ref: None,
                    blob_sha256: Some(doc_sha256.to_string()),
                    timestamp: Some(now),
                    extractor: ExtractorKind::Rule,
                    confidence: BRIDGE_CONFIDENCE,
                    is_tainted: true,
                    requires_human_review: true,
                },
            });
            mutations.push(GraphMutation::UpsertEdge {
                edge: Edge {
                    id: EdgeId(Uuid::new_v4().to_string()),
                    from: new_id.clone(),
                    to: existing_id.clone(),
                    kind: EdgeKind::CorrelatesWith,
                    evidence: vec![ev_id.clone()],
                    confidence: BRIDGE_CONFIDENCE,
                    created_at: now,
                    created_by: ExtractorKind::Rule,
                    is_tainted: true,
                    requires_human_review: true,
                },
                evidence_ids: vec![ev_id],
            });
        }
    }

    mutations
}

#[cfg(test)]
mod tests {
    use super::*;
    use axonmind_core::NodeKind;

    fn doc_node() -> Node {
        let now = Utc::now();
        Node {
            id: NodeId("doc.abc".into()),
            kind: NodeKind::Document,
            name: "Doc".into(),
            attrs: serde_json::Value::Null,
            confidence: Confidence::RULE,
            created_at: now,
            updated_at: now,
            is_tainted: false,
            requires_human_review: false,
        }
    }

    #[test]
    fn links_near_duplicate_concept_and_skips_unrelated_sibling() {
        // Intent: a concept the slug-merge missed gets bridged; a same-word-but-different concept
        // does not. This is what stops the two document clusters from being disconnected blobs.
        let new = vec![(
            NodeId("insight.ai_assisted_risk_evaluation".into()),
            "AI-Assisted Risk Evaluation".to_string(),
        )];
        let existing = vec![
            (
                NodeId("insight.risk_evaluation".into()),
                "Risk Evaluation".to_string(),
            ),
            (
                NodeId("insight.broker_warranties".into()),
                "Broker Warranties".to_string(),
            ),
        ];

        let muts = build_cross_document_bridges(&doc_node(), "sha", &new, &existing);

        let edges: Vec<&Edge> = muts
            .iter()
            .filter_map(|m| match m {
                GraphMutation::UpsertEdge { edge, .. } => Some(edge),
                _ => None,
            })
            .collect();
        assert_eq!(edges.len(), 1, "exactly one near-match should bridge");
        assert_eq!(edges[0].kind, EdgeKind::CorrelatesWith);
        assert_eq!(edges[0].from.0, "insight.ai_assisted_risk_evaluation");
        assert_eq!(edges[0].to.0, "insight.risk_evaluation");
        // The edge must carry evidence (hard store invariant) — verify it was emitted.
        assert!(!edges[0].evidence.is_empty());
        assert!(
            muts.iter()
                .any(|m| matches!(m, GraphMutation::UpsertEvidence { .. }))
        );
    }

    #[test]
    fn never_self_links_the_same_node() {
        // On re-ingest the same concept appears on both sides; it must not link to itself.
        let same = vec![(
            NodeId("insight.risk_evaluation".into()),
            "Risk Evaluation".to_string(),
        )];
        let muts = build_cross_document_bridges(&doc_node(), "sha", &same, &same);
        assert!(muts.is_empty());
    }
}
