//! Phase 3 (E v2): LLM-assisted cross-document semantic linking.
//!
//! Where the deterministic bridge ([`super::bridge`]) only connects near-duplicate *names*, this
//! pass asks the LLM for *meaningful* relationships between concepts from the document just
//! ingested and concepts already in the graph — e.g. an agreement constraint that `Blocks` a
//! deck capability. One batched call per ingest (not per concept-pair), with both concept lists
//! capped to bound cost. The LLM refers to concepts by index; we map indices back to node IDs.

use super::llm::{LlmProvider, SemanticLink, SemanticLinkInput};
use super::normalize::{normalize_edge_kind, sanitize_confidence};
use crate::store::GraphMutation;
use axonmind_core::{
    AxonMindError, Edge, EdgeId, EdgeKind, Evidence, EvidenceId, ExtractorKind, Node, NodeId,
    SourceType,
};
use chrono::Utc;
use uuid::Uuid;

/// Max concepts sent per side. Bounds prompt + output size and keeps the call cost-stable.
const SEMANTIC_LINK_LIMIT: usize = 40;
const REVIEW_CONFIDENCE_THRESHOLD: f32 = 0.60;

/// Run one cross-document semantic-linking call and convert the result to mutations. Caps each
/// concept list to [`SEMANTIC_LINK_LIMIT`]; returns no mutations (and makes no call) when either
/// side is empty. Returns `Err` on LLM failure so the caller can surface it.
pub async fn run_semantic_linking(
    llm: &dyn LlmProvider,
    doc_node: &Node,
    doc_sha256: &str,
    new_concepts: &[(NodeId, String)],
    existing_concepts: &[(NodeId, String)],
) -> Result<Vec<GraphMutation>, AxonMindError> {
    let new_capped = &new_concepts[..new_concepts.len().min(SEMANTIC_LINK_LIMIT)];
    let existing_capped = &existing_concepts[..existing_concepts.len().min(SEMANTIC_LINK_LIMIT)];
    if new_capped.is_empty() || existing_capped.is_empty() {
        return Ok(Vec::new());
    }

    let output = llm
        .link_concepts(SemanticLinkInput {
            new_concepts: new_capped.iter().map(|(_, n)| n.clone()).collect(),
            existing_concepts: existing_capped.iter().map(|(_, n)| n.clone()).collect(),
        })
        .await?;

    // The LLM indexes into the *capped* lists, so the builder must see those same slices.
    Ok(build_semantic_link_mutations(
        doc_node,
        doc_sha256,
        new_capped,
        existing_capped,
        output.links,
    ))
}

/// Convert LLM-returned links into evidence+edge mutations. Pure (no I/O) so it is unit-testable.
/// Drops links whose indices are out of range, whose `from`/`to` resolve to the same node, whose
/// edge kind is unmappable, or that are `MentionedIn` (structural provenance, not a relation).
pub fn build_semantic_link_mutations(
    doc_node: &Node,
    doc_sha256: &str,
    new_concepts: &[(NodeId, String)],
    existing_concepts: &[(NodeId, String)],
    links: Vec<SemanticLink>,
) -> Vec<GraphMutation> {
    let mut mutations = Vec::new();

    for link in links {
        let (Some((from_id, _)), Some((to_id, _))) = (
            new_concepts.get(link.from_new),
            existing_concepts.get(link.to_existing),
        ) else {
            continue;
        };
        if from_id == to_id {
            continue;
        }
        let Some(kind) = normalize_edge_kind(&link.edge_kind) else {
            continue;
        };
        if kind == EdgeKind::MentionedIn {
            continue;
        }

        let confidence = sanitize_confidence(link.confidence);
        let requires_human_review = confidence.0 < REVIEW_CONFIDENCE_THRESHOLD;
        let now = Utc::now();
        let ev_id = EvidenceId(Uuid::new_v4().to_string());

        mutations.push(GraphMutation::UpsertEvidence {
            evidence: Evidence {
                id: ev_id.clone(),
                source_node_id: doc_node.id.clone(),
                source_type: SourceType::Document,
                quote: Some(link.rationale),
                row_ref: None,
                blob_sha256: Some(doc_sha256.to_string()),
                timestamp: Some(now),
                extractor: ExtractorKind::Llm,
                confidence,
                is_tainted: true,
                requires_human_review,
            },
        });
        mutations.push(GraphMutation::UpsertEdge {
            edge: Edge {
                id: EdgeId(Uuid::new_v4().to_string()),
                from: from_id.clone(),
                to: to_id.clone(),
                kind,
                evidence: vec![ev_id.clone()],
                confidence,
                created_at: now,
                created_by: ExtractorKind::Llm,
                is_tainted: true,
                requires_human_review,
            },
            evidence_ids: vec![ev_id],
        });
    }

    mutations
}

#[cfg(test)]
mod tests {
    use super::*;
    use axonmind_core::{Confidence, NodeKind};

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

    fn link(from_new: usize, to_existing: usize, kind: &str) -> SemanticLink {
        SemanticLink {
            from_new,
            to_existing,
            edge_kind: kind.into(),
            confidence: 0.8,
            rationale: "because".into(),
        }
    }

    #[test]
    fn maps_valid_link_to_edge_and_drops_invalid_ones() {
        // Intent: a well-formed link becomes one evidence-backed edge with indices resolved to
        // node IDs; malformed links never reach the graph.
        let new = vec![(
            NodeId("cap.auto_quoting".into()),
            "Autonomous Quoting".into(),
        )];
        let existing = vec![(NodeId("ctl.hitl".into()), "Human-in-the-Loop".into())];

        let links = vec![
            link(0, 0, "Blocks"),      // valid → kept
            link(9, 0, "Blocks"),      // from index out of range → dropped
            link(0, 0, "frobnicate"),  // unmappable edge kind → dropped
            link(0, 0, "MentionedIn"), // structural provenance → dropped
        ];

        let muts = build_semantic_link_mutations(&doc_node(), "sha", &new, &existing, links);

        let edges: Vec<&Edge> = muts
            .iter()
            .filter_map(|m| match m {
                GraphMutation::UpsertEdge { edge, .. } => Some(edge),
                _ => None,
            })
            .collect();
        assert_eq!(
            edges.len(),
            1,
            "only the one valid link should produce an edge"
        );
        assert_eq!(edges[0].kind, EdgeKind::Blocks);
        assert_eq!(edges[0].from.0, "cap.auto_quoting");
        assert_eq!(edges[0].to.0, "ctl.hitl");
        assert!(!edges[0].evidence.is_empty(), "edge must carry evidence");
    }

    #[test]
    fn never_links_a_node_to_itself() {
        let same = vec![(NodeId("x".into()), "X".into())];
        let muts = build_semantic_link_mutations(
            &doc_node(),
            "sha",
            &same,
            &same,
            vec![link(0, 0, "Blocks")],
        );
        assert!(muts.is_empty());
    }
}
