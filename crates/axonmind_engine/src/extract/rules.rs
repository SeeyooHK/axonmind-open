//! Phase 1: Rule-based entity and relation extraction from `NormalizedDocument`.
//!
//! KPI patterns are detected by case-insensitive substring match in headings.
//! Relation rules detect linking verbs in paragraph text.
use std::collections::HashSet;

use axonmind_core::{
    Confidence, Edge, EdgeId, EdgeKind, Evidence, EvidenceId, ExtractorKind, Node, NodeId,
    NodeKind, SourceType,
};
use chrono::Utc;
use uuid::Uuid;

use crate::ingest::{DocumentBlock, NormalizedDocument, NormalizedTable};
use crate::store::{GraphMutation, MetricValue};

/// KPI term patterns — case-insensitive substring matches against heading text.
static KPI_PATTERNS: &[&str] = &[
    "revenue",
    "arr",
    "mrr",
    "churn",
    "cac",
    "ltv",
    "nps",
    "clv",
    "aov",
    "gross margin",
    "net revenue",
    "conversion rate",
    "retention",
    "burn rate",
    "runway",
    "daily active",
    "monthly active",
    "average order",
    "customer acquisition",
    "customer lifetime",
    "operating margin",
    "ebitda",
    "net promoter",
];

/// Linking verbs that imply an Influences edge between co-occurring entities in a paragraph.
static INFLUENCE_VERBS: &[&str] = &[
    "drives",
    "drive",
    "impacts",
    "impact",
    "affects",
    "affect",
    "influences",
    "influence",
    "improves",
    "improve",
    "increases",
    "increase",
];
static BLOCK_VERBS: &[&str] = &["blocks", "block", "limits", "limit", "reduces", "reduce"];

/// Extract `GraphMutation`s from a parsed document.
///
/// Returns mutations in the correct application order:
/// 1. UpsertNode for the document node itself
/// 2. UpsertEvidence for each piece of evidence
/// 3. UpsertNode for each extracted entity
/// 4. UpsertEdge for each extracted relation (requires evidence to exist first)
pub fn extract(doc: &NormalizedDocument, doc_node: &Node) -> Vec<GraphMutation> {
    let mut mutations: Vec<GraphMutation> = Vec::new();
    let mut kpi_ids: Vec<(NodeId, EvidenceId)> = Vec::new();

    // ── Heading-based KPI detection ───────────────────────────────────────────
    for block in &doc.blocks {
        let (level, text) = match block {
            DocumentBlock::Heading { level, text, .. } => (*level, text),
            _ => continue,
        };

        let lower = text.to_lowercase();
        let is_kpi = KPI_PATTERNS.iter().any(|pat| lower.contains(pat));
        if !is_kpi {
            continue;
        }

        let slug = slugify(text);
        let kpi_id = NodeId(format!("kpi.{slug}"));
        let evidence_id = EvidenceId(Uuid::new_v4().to_string());
        let now = Utc::now();

        // Evidence: the heading text in this document
        mutations.push(GraphMutation::UpsertEvidence {
            evidence: Evidence {
                id: evidence_id.clone(),
                source_node_id: doc_node.id.clone(),
                source_type: SourceType::Document,
                quote: Some(text.clone()),
                row_ref: Some(format!("h{level}")),
                blob_sha256: doc.sha256.clone().into(),
                timestamp: Some(now),
                extractor: ExtractorKind::Rule,
                confidence: Confidence::RULE,
                is_tainted: false,
                requires_human_review: false,
            },
        });

        // KPI node (upsert — multiple documents may mention same KPI)
        let kpi_attrs = serde_json::json!({
            "value": null,
            "unit": "Count",
            "period": "Month",
            "status": "Unknown",
            "trend": "Unknown",
            "target": null,
            "owner_node_id": null,
            "definition": text,
            "source_refs": [doc_node.id.0],
            "explanation": null,
            "last_recomputed_at": null,
        });
        mutations.push(GraphMutation::UpsertNode {
            node: Node {
                id: kpi_id.clone(),
                kind: NodeKind::Kpi,
                name: text.clone(),
                attrs: kpi_attrs,
                confidence: Confidence::RULE,
                created_at: now,
                updated_at: now,
                is_tainted: false,
                requires_human_review: true, // KPI candidates need human review
            },
        });

        // Edge: Document → KPI (MentionedIn)
        let edge_id = EdgeId(Uuid::new_v4().to_string());
        mutations.push(GraphMutation::UpsertEdge {
            edge: Edge {
                id: edge_id,
                from: doc_node.id.clone(),
                to: kpi_id.clone(),
                kind: EdgeKind::MentionedIn,
                evidence: vec![evidence_id.clone()],
                confidence: Confidence::RULE,
                created_at: now,
                created_by: ExtractorKind::Rule,
                is_tainted: false,
                requires_human_review: false,
            },
            evidence_ids: vec![evidence_id.clone()],
        });

        kpi_ids.push((kpi_id, evidence_id));
    }

    // ── Table-based Metric detection ──────────────────────────────────────────
    for (table_idx, table) in doc.tables.iter().enumerate() {
        extract_table_metrics(table, table_idx, doc_node, doc, &kpi_ids, &mut mutations);
    }

    // ── Paragraph linking-verb relation extraction ────────────────────────────
    if kpi_ids.len() >= 2 {
        extract_paragraph_relations(&doc.blocks, &kpi_ids, doc_node, &mut mutations);
    }

    mutations
}

fn extract_table_metrics(
    table: &NormalizedTable,
    table_idx: usize,
    doc_node: &Node,
    doc: &NormalizedDocument,
    kpi_ids: &[(NodeId, EvidenceId)],
    mutations: &mut Vec<GraphMutation>,
) {
    use super::value_parse::parse_metric_cell;

    // Build a set of known KPI node IDs for O(1) lookup when linking metric values.
    let kpi_set: HashSet<&str> = kpi_ids.iter().map(|(id, _)| id.0.as_str()).collect();

    // Detect metric tables: first column is a label, remaining columns have numeric values.
    for (row_idx, row) in table.rows.iter().enumerate() {
        if row.is_empty() {
            continue;
        }
        let label = &row[0];
        if label.trim().is_empty() {
            continue;
        }

        // First parseable numeric cell wins.
        let Some(parsed) = row.iter().skip(1).find_map(|c| parse_metric_cell(c)) else {
            continue;
        };

        let metric_id = NodeId(format!("metric.{}.t{table_idx}r{row_idx}", slugify(label)));
        let evidence_id = EvidenceId(Uuid::new_v4().to_string());
        let now = Utc::now();
        let row_ref = format!("table:{table_idx} row:{row_idx}");

        mutations.push(GraphMutation::UpsertEvidence {
            evidence: Evidence {
                id: evidence_id.clone(),
                source_node_id: doc_node.id.clone(),
                source_type: SourceType::Table,
                quote: Some(format!("{}: {}", label, row[1..].join(", "))),
                row_ref: Some(row_ref),
                blob_sha256: doc.sha256.clone().into(),
                timestamp: Some(now),
                extractor: ExtractorKind::Rule,
                confidence: Confidence::RULE,
                is_tainted: false,
                requires_human_review: false,
            },
        });

        // Metric node with parsed value in attrs — no longer Null.
        let attrs = serde_json::json!({
            "value": parsed.value,
            "unit": parsed.unit,
        });
        mutations.push(GraphMutation::UpsertNode {
            node: Node {
                id: metric_id.clone(),
                kind: NodeKind::Metric,
                name: label.clone(),
                attrs,
                confidence: Confidence::RULE,
                created_at: now,
                updated_at: now,
                is_tainted: false,
                requires_human_review: true,
            },
        });

        let edge_id = EdgeId(Uuid::new_v4().to_string());
        mutations.push(GraphMutation::UpsertEdge {
            edge: Edge {
                id: edge_id,
                from: doc_node.id.clone(),
                to: metric_id.clone(),
                kind: EdgeKind::MentionedIn,
                evidence: vec![evidence_id.clone()],
                confidence: Confidence::RULE,
                created_at: now,
                created_by: ExtractorKind::Rule,
                is_tainted: false,
                requires_human_review: false,
            },
            evidence_ids: vec![evidence_id.clone()],
        });

        // Link to a KPI if this row's label matches a KPI detected from a heading in the
        // same document. Only emit RecordMetricValue when the FK target is guaranteed to
        // exist (it was added to mutations earlier in this same batch).
        let candidate_kpi_id = NodeId(format!("kpi.{}", slugify(label)));
        if kpi_set.contains(candidate_kpi_id.0.as_str()) {
            mutations.push(GraphMutation::RecordMetricValue {
                value: MetricValue {
                    id: Uuid::new_v4().to_string(),
                    kpi_node_id: candidate_kpi_id,
                    metric_node_id: metric_id,
                    value: parsed.value,
                    unit: parsed.unit,
                    period_start: None,
                    period_end: None,
                    as_of: None,
                    observed_at: now,
                    evidence_id: evidence_id,
                },
            });
        }
    }
}

fn extract_paragraph_relations(
    blocks: &[DocumentBlock],
    kpi_ids: &[(NodeId, EvidenceId)],
    _doc_node: &Node,
    mutations: &mut Vec<GraphMutation>,
) {
    for block in blocks {
        let text = match block {
            DocumentBlock::Paragraph { text, .. } => text,
            _ => continue,
        };
        let lower = text.to_lowercase();

        // Find which KPIs are mentioned in this paragraph
        let mentioned: Vec<&(NodeId, EvidenceId)> = kpi_ids
            .iter()
            .filter(|(id, _)| {
                let name = id.0.trim_start_matches("kpi.").replace('_', " ");
                lower.contains(&name)
            })
            .collect();
        if mentioned.len() < 2 {
            continue;
        }

        // Check for linking verbs and emit edges between mentioned KPIs
        let now = Utc::now();
        for i in 0..mentioned.len() {
            for j in (i + 1)..mentioned.len() {
                let (from_id, from_ev) = &mentioned[i];
                let (to_id, _to_ev) = &mentioned[j];

                let kind = if INFLUENCE_VERBS.iter().any(|v| lower.contains(v)) {
                    EdgeKind::Influences
                } else if BLOCK_VERBS.iter().any(|v| lower.contains(v)) {
                    EdgeKind::Blocks
                } else {
                    continue; // no recognizable verb, skip
                };

                // Reuse the existing evidence from the heading extractions
                let ev_id = from_ev.clone();
                let edge_id = EdgeId(Uuid::new_v4().to_string());
                mutations.push(GraphMutation::UpsertEdge {
                    edge: Edge {
                        id: edge_id,
                        from: from_id.clone(),
                        to: to_id.clone(),
                        kind,
                        evidence: vec![ev_id.clone()],
                        confidence: Confidence(0.60), // lower confidence for co-occurrence
                        created_at: now,
                        created_by: ExtractorKind::Rule,
                        is_tainted: false,
                        requires_human_review: true,
                    },
                    evidence_ids: vec![ev_id],
                });
            }
        }
    }
}

/// Convert a free-form name to a lowercase underscore slug, e.g. "Revenue Growth" → "revenue_growth"
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
