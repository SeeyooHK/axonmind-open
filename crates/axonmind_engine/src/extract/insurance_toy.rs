//! Phase 8 (open-repo): toy insurance table enrichment.
//!
//! This module is intentionally conservative and non-production. It only detects obvious
//! policy/claim-style tables and emits generic graph objects to demonstrate extension seams.
//! Production insurance extraction quality and coverage remain commercial-only.

use axonmind_core::{
    Confidence, Edge, EdgeId, EdgeKind, Evidence, EvidenceId, ExtractorKind, Node, NodeId,
    NodeKind, SourceType,
};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::ingest::{NormalizedDocument, NormalizedTable};
use crate::store::GraphMutation;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TableKind {
    Policy,
    Claim,
}

struct TableSchema {
    table_kind: TableKind,
    id_col: Option<usize>,
    policy_col: Option<usize>,
    customer_col: Option<usize>,
    product_col: Option<usize>,
    status_col: Option<usize>,
    premium_col: Option<usize>,
    incurred_col: Option<usize>,
    paid_col: Option<usize>,
    reserve_col: Option<usize>,
    as_of_col: Option<usize>,
}

pub fn extract(doc: &NormalizedDocument, doc_node: &Node) -> Vec<GraphMutation> {
    let mut out = Vec::<GraphMutation>::new();

    for (table_idx, table) in doc.tables.iter().enumerate() {
        let Some(schema) = detect_schema(table) else {
            continue;
        };
        for (row_idx, row) in table.rows.iter().enumerate() {
            if row.is_empty() || row.iter().all(|c| c.trim().is_empty()) {
                continue;
            }
            let row_mutations = extract_row(doc, doc_node, table_idx, row_idx, row, &schema);
            out.extend(row_mutations);
        }
    }

    out
}

fn extract_row(
    doc: &NormalizedDocument,
    doc_node: &Node,
    table_idx: usize,
    row_idx: usize,
    row: &[String],
    schema: &TableSchema,
) -> Vec<GraphMutation> {
    use super::value_parse::parse_metric_cell;

    let now = Utc::now();
    let mut out = Vec::<GraphMutation>::new();

    let external_id = schema
        .id_col
        .and_then(|i| row.get(i))
        .map(String::as_str)
        .unwrap_or_default()
        .trim();
    let stable_id_part = if external_id.is_empty() {
        format!("t{table_idx}r{row_idx}")
    } else {
        slugify(external_id)
    };
    let kind_prefix = match schema.table_kind {
        TableKind::Policy => "policy_row",
        TableKind::Claim => "claim_row",
    };
    let row_node_id = NodeId(format!(
        "process.{kind_prefix}.{}.{}",
        slugify(&doc.id),
        stable_id_part
    ));
    let row_label = if external_id.is_empty() {
        format!(
            "{} {}",
            if schema.table_kind == TableKind::Policy {
                "Policy Row"
            } else {
                "Claim Row"
            },
            row_idx + 1
        )
    } else if schema.table_kind == TableKind::Policy {
        format!("Policy {}", external_id)
    } else {
        format!("Claim {}", external_id)
    };

    let premium = schema
        .premium_col
        .and_then(|i| row.get(i))
        .and_then(|v| parse_metric_cell(v))
        .map(|m| json!({"value": m.value, "unit": m.unit}));
    let incurred = schema
        .incurred_col
        .and_then(|i| row.get(i))
        .and_then(|v| parse_metric_cell(v))
        .map(|m| json!({"value": m.value, "unit": m.unit}));
    let paid = schema
        .paid_col
        .and_then(|i| row.get(i))
        .and_then(|v| parse_metric_cell(v))
        .map(|m| json!({"value": m.value, "unit": m.unit}));
    let reserve = schema
        .reserve_col
        .and_then(|i| row.get(i))
        .and_then(|v| parse_metric_cell(v))
        .map(|m| json!({"value": m.value, "unit": m.unit}));

    let status = schema
        .status_col
        .and_then(|i| row.get(i))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let as_of = schema
        .as_of_col
        .and_then(|i| row.get(i))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    let evidence_id = EvidenceId(Uuid::new_v4().to_string());
    let row_ref = format!("table:{table_idx} row:{row_idx}");
    let quote = row.join(" | ");
    out.push(GraphMutation::UpsertEvidence {
        evidence: Evidence {
            id: evidence_id.clone(),
            source_node_id: doc_node.id.clone(),
            source_type: SourceType::Table,
            quote: Some(quote),
            row_ref: Some(row_ref),
            blob_sha256: Some(doc.sha256.clone()),
            timestamp: Some(now),
            extractor: ExtractorKind::Rule,
            confidence: Confidence::RULE,
            is_tainted: false,
            requires_human_review: false,
        },
    });

    out.push(GraphMutation::UpsertNode {
        node: Node {
            id: row_node_id.clone(),
            kind: NodeKind::Process,
            name: row_label,
            attrs: json!({
                "object_type": kind_prefix,
                "external_id": if external_id.is_empty() { serde_json::Value::Null } else { json!(external_id) },
                "status": status,
                "as_of": as_of,
                "policy_ref": schema.policy_col.and_then(|i| row.get(i)).map(|s| s.trim()).filter(|s| !s.is_empty()),
                "premium": premium,
                "incurred": incurred,
                "paid": paid,
                "reserve": reserve,
            }),
            confidence: Confidence::RULE,
            created_at: now,
            updated_at: now,
            is_tainted: false,
            requires_human_review: true,
        },
    });
    out.push(GraphMutation::UpsertEdge {
        edge: Edge {
            id: EdgeId(Uuid::new_v4().to_string()),
            from: doc_node.id.clone(),
            to: row_node_id.clone(),
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

    if let Some(product_name) = schema
        .product_col
        .and_then(|i| row.get(i))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let product_node_id = NodeId(format!("product.{}", slugify(product_name)));
        out.push(GraphMutation::UpsertNode {
            node: Node {
                id: product_node_id.clone(),
                kind: NodeKind::Product,
                name: product_name.to_string(),
                attrs: json!({}),
                confidence: Confidence::RULE,
                created_at: now,
                updated_at: now,
                is_tainted: false,
                requires_human_review: true,
            },
        });
        out.push(GraphMutation::UpsertEdge {
            edge: Edge {
                id: EdgeId(Uuid::new_v4().to_string()),
                from: row_node_id.clone(),
                to: product_node_id,
                kind: EdgeKind::ForProduct,
                evidence: vec![evidence_id.clone()],
                confidence: Confidence::RULE,
                created_at: now,
                created_by: ExtractorKind::Rule,
                is_tainted: false,
                requires_human_review: false,
            },
            evidence_ids: vec![evidence_id.clone()],
        });
    }

    if let Some(customer_name) = schema
        .customer_col
        .and_then(|i| row.get(i))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let customer_node_id = NodeId(format!("customer.{}", slugify(customer_name)));
        out.push(GraphMutation::UpsertNode {
            node: Node {
                id: customer_node_id.clone(),
                kind: NodeKind::Customer,
                name: customer_name.to_string(),
                attrs: json!({}),
                confidence: Confidence::RULE,
                created_at: now,
                updated_at: now,
                is_tainted: false,
                requires_human_review: true,
            },
        });
        out.push(GraphMutation::UpsertEdge {
            edge: Edge {
                id: EdgeId(Uuid::new_v4().to_string()),
                from: row_node_id,
                to: customer_node_id,
                kind: EdgeKind::OwnedBy,
                evidence: vec![evidence_id.clone()],
                confidence: Confidence::RULE,
                created_at: now,
                created_by: ExtractorKind::Rule,
                is_tainted: false,
                requires_human_review: false,
            },
            evidence_ids: vec![evidence_id],
        });
    }

    out
}

fn detect_schema(table: &NormalizedTable) -> Option<TableSchema> {
    if table.headers.is_empty() {
        return None;
    }

    let normalized = table
        .headers
        .iter()
        .map(|h| normalize_header(h))
        .collect::<Vec<_>>();

    let find = |aliases: &[&str]| -> Option<usize> {
        normalized
            .iter()
            .position(|h| aliases.iter().any(|a| h.contains(a)))
    };

    let claim_marker = find(&["claim", "claimid", "claimnumber"]);
    let policy_marker = find(&["policy", "policyid", "policynumber"]);
    let premium_col = find(&["premium", "writtenpremium", "earnedpremium"]);
    let incurred_col = find(&["incurred", "lossincurred"]);
    let paid_col = find(&["paid", "losspaid"]);
    let reserve_col = find(&["reserve", "outstanding"]);
    let customer_col = find(&["customer", "client", "insured", "policyholder"]);
    let product_col = find(&["product", "lob", "lineofbusiness"]);
    let status_col = find(&["status", "claimstatus", "policystatus"]);
    let as_of_col = find(&["asof", "reportdate", "valuationdate", "snapshotdate"]);

    let table_kind = if claim_marker.is_some()
        && (incurred_col.is_some() || paid_col.is_some() || reserve_col.is_some())
    {
        Some(TableKind::Claim)
    } else if policy_marker.is_some()
        && (premium_col.is_some() || customer_col.is_some() || product_col.is_some())
    {
        Some(TableKind::Policy)
    } else {
        None
    }?;

    Some(TableSchema {
        table_kind,
        id_col: if table_kind == TableKind::Claim {
            claim_marker
        } else {
            policy_marker
        },
        policy_col: policy_marker,
        customer_col,
        product_col,
        status_col,
        premium_col,
        incurred_col,
        paid_col,
        reserve_col,
        as_of_col,
    })
}

fn normalize_header(raw: &str) -> String {
    raw.to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

fn slugify(name: &str) -> String {
    name.to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::SourceSpan;

    fn sample_doc(table: NormalizedTable) -> (NormalizedDocument, Node) {
        let doc = NormalizedDocument {
            id: "doc.test1234".to_string(),
            source_path: None,
            sha256: "abc".to_string(),
            title: None,
            blocks: vec![],
            tables: vec![table],
        };
        let now = Utc::now();
        let doc_node = Node {
            id: NodeId(doc.id.clone()),
            kind: NodeKind::Document,
            name: "Doc".to_string(),
            created_at: now,
            updated_at: now,
            attrs: json!({}),
            confidence: Confidence::RULE,
            is_tainted: false,
            requires_human_review: false,
        };
        (doc, doc_node)
    }

    #[test]
    fn extracts_policy_rows_into_generic_objects() {
        let table = NormalizedTable {
            headers: vec![
                "Policy Number".to_string(),
                "Policyholder".to_string(),
                "Line of Business".to_string(),
                "Written Premium".to_string(),
                "Status".to_string(),
                "As Of".to_string(),
            ],
            rows: vec![vec![
                "POL-1".to_string(),
                "Acme Ltd".to_string(),
                "Auto".to_string(),
                "$120,000".to_string(),
                "In Force".to_string(),
                "2026-05-01".to_string(),
            ]],
            span: SourceSpan::default(),
        };
        let (doc, doc_node) = sample_doc(table);
        let muts = extract(&doc, &doc_node);

        let has_policy_obj = muts.iter().any(|m| {
            matches!(
                m,
                GraphMutation::UpsertNode { node }
                    if node.kind == NodeKind::Process
                    && node.attrs.get("object_type").and_then(|v| v.as_str()) == Some("policy_row")
            )
        });
        let has_product_link = muts.iter().any(|m| {
            matches!(
                m,
                GraphMutation::UpsertEdge { edge, .. } if edge.kind == EdgeKind::ForProduct
            )
        });
        let has_customer_link = muts.iter().any(|m| {
            matches!(
                m,
                GraphMutation::UpsertEdge { edge, .. } if edge.kind == EdgeKind::OwnedBy
            )
        });

        assert!(has_policy_obj);
        assert!(has_product_link);
        assert!(has_customer_link);
    }

    #[test]
    fn extracts_claim_rows_with_loss_fields() {
        let table = NormalizedTable {
            headers: vec![
                "Claim ID".to_string(),
                "Policy Number".to_string(),
                "Incurred".to_string(),
                "Paid".to_string(),
                "Reserve".to_string(),
                "Status".to_string(),
            ],
            rows: vec![vec![
                "CLM-1".to_string(),
                "POL-1".to_string(),
                "$10,000".to_string(),
                "$4,000".to_string(),
                "$6,000".to_string(),
                "Open".to_string(),
            ]],
            span: SourceSpan::default(),
        };
        let (doc, doc_node) = sample_doc(table);
        let muts = extract(&doc, &doc_node);

        let claim_attrs = muts.iter().find_map(|m| match m {
            GraphMutation::UpsertNode { node }
                if node.kind == NodeKind::Process
                    && node.attrs.get("object_type").and_then(|v| v.as_str())
                        == Some("claim_row") =>
            {
                Some(node.attrs.clone())
            }
            _ => None,
        });
        let attrs = claim_attrs.expect("claim row attrs should exist");
        assert!(attrs.get("incurred").is_some());
        assert!(attrs.get("paid").is_some());
        assert!(attrs.get("reserve").is_some());
    }
}
