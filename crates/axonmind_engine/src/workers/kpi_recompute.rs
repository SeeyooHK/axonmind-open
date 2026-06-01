//! Phase 4: KPI recomputation worker.
//!
//! For each KPI node, every recompute_interval:
//!   1. Aggregate confidence via noisy-OR across all evidence records.
//!   2. Compute trend from the two most recent metric_values (Up/Down/Flat/Unknown).
//!   3. Update latest value and last_recomputed_at in KpiAttrs.
//!   4. Apply UpsertNode with the updated attrs and confidence.
//!
//! Historical metric_values are never deleted.
//! Runs every WorkerConfig::recompute_interval (default: 5 minutes).
use crate::config::EngineConfig;
use crate::events::EngineEvent;
use crate::store::{GraphCache, GraphMutation, GraphStore};
use axonmind_core::{Confidence, EdgeKind, KpiAttrs, KpiTrend, NodeKind};
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};

#[cfg(test)]
pub(crate) async fn run_once(
    store: &GraphStore,
    cache: &RwLock<GraphCache>,
    event_tx: &broadcast::Sender<EngineEvent>,
) {
    _run_once(store, cache, event_tx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{GraphCache, GraphMutation, GraphStore};
    use axonmind_core::{
        Confidence, Edge, EdgeId, Evidence, EvidenceId, ExtractorKind, KpiAttrs, KpiStatus,
        KpiTrend, KpiUnit, Node, NodeId, NodeKind, Period, SourceType,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    async fn open_store(dir: &TempDir) -> GraphStore {
        GraphStore::open(&dir.path().join("test.db")).await.unwrap()
    }

    fn kpi_node(id: &str, confidence: Confidence) -> Node {
        let attrs = KpiAttrs {
            value: None,
            unit: KpiUnit::Count,
            period: Period::Month,
            status: KpiStatus::Unknown,
            trend: KpiTrend::Unknown,
            target: None,
            owner_node_id: None,
            definition: String::new(),
            source_refs: vec![],
            explanation: None,
            last_recomputed_at: None,
        };
        let now = chrono::Utc::now();
        Node {
            id: NodeId(id.into()),
            kind: NodeKind::Kpi,
            name: id.into(),
            attrs: serde_json::to_value(&attrs).unwrap(),
            confidence,
            created_at: now,
            updated_at: now,
            is_tainted: false,
            requires_human_review: false,
        }
    }

    fn metric_node(id: &str) -> Node {
        let now = chrono::Utc::now();
        Node {
            id: NodeId(id.into()),
            kind: NodeKind::Metric,
            name: id.into(),
            attrs: serde_json::Value::Null,
            confidence: Confidence::RULE,
            created_at: now,
            updated_at: now,
            is_tainted: false,
            requires_human_review: false,
        }
    }

    fn evidence(source_node_id: &str) -> Evidence {
        Evidence {
            id: EvidenceId(Uuid::new_v4().to_string()),
            source_node_id: NodeId(source_node_id.into()),
            source_type: SourceType::Document,
            quote: Some("contradicts the kpi claim".into()),
            row_ref: None,
            blob_sha256: None,
            timestamp: None,
            extractor: ExtractorKind::Rule,
            confidence: Confidence(0.8),
            is_tainted: false,
            requires_human_review: false,
        }
    }

    /// WHY: a KPI with a Contradicts incoming edge must be flagged for human review
    /// and have its confidence dampened below the support-only level.
    /// This is the core invariant of contradiction-aware confidence (Feature 3).
    #[tokio::test]
    async fn contradicts_edge_sets_review_flag_and_dampens_confidence() {
        let dir = TempDir::new().unwrap();
        let store = open_store(&dir).await;
        let cache = RwLock::new(GraphCache::new());
        let (event_tx, _rx) = broadcast::channel(16);

        let kpi_id = "kpi.test_metric";
        let source_id = "doc.source";
        let initial_confidence = Confidence(0.9);

        // Insert KPI node and a source node.
        for mutation in [
            GraphMutation::UpsertNode {
                node: kpi_node(kpi_id, initial_confidence),
            },
            GraphMutation::UpsertNode {
                node: metric_node(source_id),
            },
        ] {
            store
                .apply_mutation(mutation, &cache, &event_tx)
                .await
                .unwrap();
        }

        // Insert evidence, then the Contradicts edge referencing it.
        let ev = evidence(source_id);
        let ev_id = ev.id.clone();
        store
            .apply_mutation(
                GraphMutation::UpsertEvidence { evidence: ev },
                &cache,
                &event_tx,
            )
            .await
            .unwrap();

        let now = chrono::Utc::now();
        let edge = Edge {
            id: EdgeId(Uuid::new_v4().to_string()),
            from: NodeId(source_id.into()),
            to: NodeId(kpi_id.into()),
            kind: EdgeKind::Contradicts,
            evidence: vec![ev_id.clone()],
            confidence: Confidence(0.8),
            created_at: now,
            created_by: ExtractorKind::Rule,
            is_tainted: false,
            requires_human_review: false,
        };
        store
            .apply_mutation(
                GraphMutation::UpsertEdge {
                    edge,
                    evidence_ids: vec![ev_id],
                },
                &cache,
                &event_tx,
            )
            .await
            .unwrap();

        run_once(&store, &cache, &event_tx).await;

        let updated = store
            .fetch_node(&NodeId(kpi_id.into()))
            .await
            .unwrap()
            .unwrap();
        assert!(
            updated.requires_human_review,
            "Contradicts edge must set requires_human_review"
        );
        assert!(
            updated.confidence.0 < initial_confidence.0,
            "Contradicts edge must dampen confidence: was {}, now {}",
            initial_confidence.0,
            updated.confidence.0
        );
    }
}

pub fn spawn(
    store: Arc<GraphStore>,
    cache: Arc<RwLock<GraphCache>>,
    event_tx: broadcast::Sender<EngineEvent>,
    config: EngineConfig,
) {
    let interval = config.workers.recompute_interval;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            _run_once(&store, &cache, &event_tx).await;
        }
    });
}

async fn _run_once(
    store: &GraphStore,
    cache: &RwLock<GraphCache>,
    event_tx: &broadcast::Sender<EngineEvent>,
) {
    let kpi_nodes = match store.fetch_nodes_by_kind(NodeKind::Kpi).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("recompute worker: fetch_nodes_by_kind failed: {e}");
            return;
        }
    };

    for mut node in kpi_nodes {
        // Parse existing KpiAttrs — skip nodes with unparseable attrs.
        let mut attrs: KpiAttrs = match serde_json::from_value(node.attrs.clone()) {
            Ok(a) => a,
            Err(e) => {
                tracing::debug!(
                    "recompute worker: skip '{}' — attrs parse failed: {e}",
                    node.id.0
                );
                continue;
            }
        };

        // Fetch evidence backing all incoming edges, partitioned into supporting vs.
        // contradicting. Uses aggregate_signed so Contradicts edges dampen confidence.
        let edge_evidence = match store.fetch_incoming_edge_evidence(&node.id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "recompute worker: fetch_incoming_edge_evidence failed for '{}': {e}",
                    node.id.0
                );
                continue;
            }
        };

        let has_contradiction = edge_evidence
            .iter()
            .any(|(k, _)| *k == EdgeKind::Contradicts);
        let mut support_conf: Vec<Confidence> = Vec::new();
        let mut contra_conf: Vec<Confidence> = Vec::new();
        for (k, ev) in &edge_evidence {
            if *k == EdgeKind::Contradicts {
                contra_conf.push(ev.confidence);
            } else {
                support_conf.push(ev.confidence);
            }
        }

        let aggregated_confidence = if edge_evidence.is_empty() {
            node.confidence
        } else {
            Confidence::aggregate_signed(&support_conf, &contra_conf)
        };

        // Compute trend from the two most recent metric values.
        let metric_values = match store.fetch_latest_metric_values(&node.id, 2).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "recompute worker: fetch_metric_values failed for '{}': {e}",
                    node.id.0
                );
                continue;
            }
        };

        let trend = match metric_values.as_slice() {
            [latest, previous] => {
                if latest.value > previous.value {
                    KpiTrend::Up
                } else if latest.value < previous.value {
                    KpiTrend::Down
                } else {
                    KpiTrend::Flat
                }
            }
            _ => KpiTrend::Unknown,
        };

        attrs.trend = trend;
        attrs.last_recomputed_at = Some(Utc::now());
        if let Some(latest) = metric_values.first() {
            attrs.value = Some(latest.value);
        }

        node.attrs = match serde_json::to_value(&attrs) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "recompute worker: attrs serialize failed for '{}': {e}",
                    node.id.0
                );
                continue;
            }
        };
        node.confidence = aggregated_confidence;
        node.updated_at = Utc::now();
        // Contradicting evidence requires a human to resolve the conflict.
        if has_contradiction {
            node.requires_human_review = true;
        }

        if let Err(e) = store
            .apply_mutation(
                GraphMutation::UpsertNode { node: node.clone() },
                cache,
                event_tx,
            )
            .await
        {
            tracing::warn!(
                "recompute worker: upsert_node failed for '{}': {e}",
                node.id.0
            );
        }
    }
}
