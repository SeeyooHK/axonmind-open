//! Phase 3: LLM-assisted entity and relation extraction.
//!
//! Runs after rule extraction. Novel entities (not already created by rules) get
//! is_tainted=true and Confidence::LLM. Existing nodes are tracked for co-occurrence
//! detection but are not re-upserted (avoids confidence downgrade).
use super::llm::{EntityExtractionInput, LlmProvider, RelationExtractionInput};
use super::normalize::{
    customer_lifecycle, normalize_edge_kind, normalize_node_kind, sanitize_confidence,
    strip_leading_clause_number,
};
use crate::ingest::{DocumentBlock, NormalizedDocument};
use crate::store::GraphMutation;
use axonmind_core::{
    Confidence, Edge, EdgeId, EdgeKind, Evidence, EvidenceId, ExtractorKind, Node, NodeId,
    NodeKind, SourceType,
};
use chrono::Utc;
use std::collections::HashSet;
use uuid::Uuid;

const REVIEW_CONFIDENCE_THRESHOLD: f32 = 0.60;

/// Run LLM extraction for a single document.
///
/// `existing_ids` — node IDs already created by rule extraction for this document.
/// `existing_graph_names` — names of concept nodes already in the graph (from other
///   documents). Merged into the entity prompt's "avoid duplicating" hint so the LLM
///   reuses canonical names across documents instead of minting near-duplicates.
/// `rule_edge_pairs` — `(from_id, to_id)` pairs already linked by rule extraction.
///   The LLM relation call is skipped for any pair already covered (either direction).
/// Mutations are returned in application order: evidence → nodes → edges.
/// Returns `Err` on the first LLM API failure so callers can surface the error to the user.
pub async fn run_llm_extraction(
    llm: &dyn LlmProvider,
    doc: &NormalizedDocument,
    doc_node: &Node,
    existing_ids: HashSet<String>,
    existing_graph_names: Vec<String>,
    rule_edge_pairs: HashSet<(String, String)>,
) -> Result<Vec<GraphMutation>, axonmind_core::AxonMindError> {
    let mut mutations: Vec<GraphMutation> = Vec::new();

    let full_text = collect_text(doc);
    if full_text.trim().is_empty() {
        return Ok(mutations);
    }
    // Stay within ~3 000 tokens.
    let text = truncate(&full_text, 12_000);

    // Dedup case-insensitively across this doc's rule-node names and the graph-wide concept
    // names, preserving first-seen order. The graph names are what enable cross-document reuse.
    let mut seen: HashSet<String> = HashSet::new();
    let mut existing_names: Vec<String> = Vec::new();
    for name in existing_ids
        .iter()
        .map(|id| id.split('.').last().unwrap_or("").replace('_', " "))
        .chain(existing_graph_names)
    {
        let key = name.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        if seen.insert(key) {
            existing_names.push(name.trim().to_string());
        }
    }

    let entities = llm
        .extract_entities(EntityExtractionInput {
            document_text: text.to_string(),
            existing_node_names: existing_names,
        })
        .await?
        .entities;

    // Phase 1 — evidence + node + MentionedIn edges for novel entities.
    // For existing nodes we record the ID for co-occurrence but emit no mutations.
    let mut extracted: Vec<(NodeId, EvidenceId)> = Vec::new();

    for (kind_str, raw_name, quote) in &entities {
        let Some(node_kind) = normalize_node_kind(kind_str) else {
            continue;
        };
        // Drop any leading clause/section number ("22.3 Platform Warranties" → "Platform
        // Warranties") so headings collapse to their concept and duplicates merge on upsert.
        let name = strip_leading_clause_number(raw_name).trim();
        let slug = slugify(name);
        if slug.is_empty() {
            continue;
        }
        let node_id = NodeId(format!("{}.{slug}", kind_prefix(node_kind)));

        if existing_ids.contains(&node_id.0) {
            // Node exists from rule extraction — register without emitting mutations.
            // We still need a real evidence ID so relation edges can reference it.
            let ev_id = EvidenceId(Uuid::new_v4().to_string());
            let now = Utc::now();
            mutations.push(GraphMutation::UpsertEvidence {
                evidence: Evidence {
                    id: ev_id.clone(),
                    source_node_id: doc_node.id.clone(),
                    source_type: SourceType::Document,
                    quote: Some(quote.clone()),
                    row_ref: None,
                    blob_sha256: Some(doc.sha256.clone()),
                    timestamp: Some(now),
                    extractor: ExtractorKind::Llm,
                    confidence: Confidence::LLM,
                    is_tainted: true,
                    requires_human_review: false,
                },
            });
            extracted.push((node_id, ev_id));
            continue;
        }

        let ev_id = EvidenceId(Uuid::new_v4().to_string());
        let now = Utc::now();

        mutations.push(GraphMutation::UpsertEvidence {
            evidence: Evidence {
                id: ev_id.clone(),
                source_node_id: doc_node.id.clone(),
                source_type: SourceType::Document,
                quote: Some(quote.clone()),
                row_ref: None,
                blob_sha256: Some(doc.sha256.clone()),
                timestamp: Some(now),
                extractor: ExtractorKind::Llm,
                confidence: Confidence::LLM,
                is_tainted: true,
                requires_human_review: false,
            },
        });
        // Customer nodes carry their lifecycle stage (prospect vs active) as an attribute,
        // since prospect/lead and customer/client/account are the same kind at different stages.
        let attrs = match customer_lifecycle(kind_str) {
            Some(stage) if node_kind == NodeKind::Customer => {
                serde_json::json!({ "lifecycle": stage })
            }
            _ => serde_json::Value::Null,
        };

        mutations.push(GraphMutation::UpsertNode {
            node: Node {
                id: node_id.clone(),
                kind: node_kind,
                name: name.to_string(),
                attrs,
                confidence: Confidence::LLM,
                created_at: now,
                updated_at: now,
                is_tainted: true,
                requires_human_review: false,
            },
        });
        mutations.push(GraphMutation::UpsertEdge {
            edge: Edge {
                id: EdgeId(Uuid::new_v4().to_string()),
                from: doc_node.id.clone(),
                to: node_id.clone(),
                kind: EdgeKind::MentionedIn,
                evidence: vec![ev_id.clone()],
                confidence: Confidence::LLM,
                created_at: now,
                created_by: ExtractorKind::Llm,
                is_tainted: true,
                requires_human_review: false,
            },
            evidence_ids: vec![ev_id.clone()],
        });

        extracted.push((node_id, ev_id));
    }

    // Phase 2 — relation extraction for co-occurring entity pairs per paragraph.
    for block in &doc.blocks {
        let para = match block {
            DocumentBlock::Paragraph { text, .. } => text.as_str(),
            _ => continue,
        };
        let lower = para.to_lowercase();

        let mentioned: Vec<&(NodeId, EvidenceId)> = extracted
            .iter()
            .filter(|(id, _)| {
                let name = entity_display_name(id);
                !name.is_empty() && lower.contains(&name)
            })
            .collect();

        if mentioned.len() < 2 {
            continue;
        }

        for i in 0..mentioned.len() {
            for j in (i + 1)..mentioned.len() {
                let (a_id, _) = mentioned[i];
                let (b_id, _) = mentioned[j];

                // Skip the LLM call when rules already produced an edge for this pair.
                // Checking both directions because rule edges are directed but the coverage
                // intent is symmetric: if A→B exists, we don't need the LLM to re-confirm it.
                if rule_edge_pairs.contains(&(a_id.0.clone(), b_id.0.clone()))
                    || rule_edge_pairs.contains(&(b_id.0.clone(), a_id.0.clone()))
                {
                    continue;
                }

                let rel = llm
                    .extract_relations(RelationExtractionInput {
                        entity_a: entity_display_name(a_id),
                        entity_b: entity_display_name(b_id),
                        context_paragraph: para.to_string(),
                    })
                    .await?;

                let Some(edge_kind) = normalize_edge_kind(&rel.edge_kind) else {
                    tracing::warn!("LLM returned unmappable edge kind: {}", rel.edge_kind);
                    continue;
                };
                let confidence = sanitize_confidence(rel.confidence);
                let requires_human_review = confidence.0 < REVIEW_CONFIDENCE_THRESHOLD;

                let now = Utc::now();
                let rel_ev_id = EvidenceId(Uuid::new_v4().to_string());
                mutations.push(GraphMutation::UpsertEvidence {
                    evidence: Evidence {
                        id: rel_ev_id.clone(),
                        source_node_id: doc_node.id.clone(),
                        source_type: SourceType::Document,
                        quote: Some(rel.quote),
                        row_ref: None,
                        blob_sha256: Some(doc.sha256.clone()),
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
                        from: a_id.clone(),
                        to: b_id.clone(),
                        kind: edge_kind,
                        evidence: vec![rel_ev_id.clone()],
                        confidence,
                        created_at: now,
                        created_by: ExtractorKind::Llm,
                        is_tainted: true,
                        requires_human_review,
                    },
                    evidence_ids: vec![rel_ev_id],
                });
            }
        }
    }

    Ok(mutations)
}

fn collect_text(doc: &NormalizedDocument) -> String {
    doc.blocks
        .iter()
        .filter_map(|b| match b {
            DocumentBlock::Heading { text, .. } => Some(text.as_str()),
            DocumentBlock::Paragraph { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn truncate(text: &str, max_chars: usize) -> &str {
    if text.len() <= max_chars {
        text
    } else {
        let slice = &text[..max_chars];
        slice.rfind('\n').map(|pos| &text[..pos]).unwrap_or(slice)
    }
}

fn entity_display_name(id: &NodeId) -> String {
    id.0.split('.').last().unwrap_or("").replace('_', " ")
}

fn kind_prefix(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Kpi => "kpi",
        NodeKind::Metric => "metric",
        NodeKind::Objective => "objective",
        NodeKind::Initiative => "initiative",
        NodeKind::Risk => "risk",
        NodeKind::Opportunity => "opportunity",
        NodeKind::Decision => "decision",
        NodeKind::Insight => "insight",
        NodeKind::Document => "doc",
        NodeKind::Person => "person",
        NodeKind::Team => "team",
        NodeKind::Customer => "customer",
        NodeKind::Function => "function",
        NodeKind::Product => "product",
        NodeKind::Market => "market",
        NodeKind::Process => "process",
        NodeKind::System => "system",
        NodeKind::Action => "action",
    }
}

fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}
