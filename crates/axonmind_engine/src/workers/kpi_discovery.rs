//! Phase 4: KPI discovery worker.
//!
//! Scans all KPI-kind nodes and proposes a KpiCandidate when:
//!   - ≥ 2 distinct Document nodes have a MentionedIn edge to the KPI
//!   - ≥ 3 evidence records back the KPI
//!   - no non-rejected candidate already exists with the same name
//!
//! Runs every WorkerConfig::discovery_interval (default: daily).
//! Emits EngineEvent::KpiCandidateProposed on each new proposal.
use crate::config::EngineConfig;
use crate::events::EngineEvent;
use crate::store::{
    CandidateId, CandidateStatus, GraphCache, GraphMutation, GraphStore, KpiCandidate,
};
use axonmind_core::{Confidence, NodeKind};
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

pub fn spawn(
    store: Arc<GraphStore>,
    cache: Arc<RwLock<GraphCache>>,
    event_tx: broadcast::Sender<EngineEvent>,
    config: EngineConfig,
) {
    let interval = config.workers.discovery_interval;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            run_once(&store, &cache, &event_tx).await;
        }
    });
}

async fn run_once(
    store: &GraphStore,
    cache: &RwLock<GraphCache>,
    event_tx: &broadcast::Sender<EngineEvent>,
) {
    let kpi_nodes = match store.fetch_nodes_by_kind(NodeKind::Kpi).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("discovery worker: fetch_nodes_by_kind failed: {e}");
            return;
        }
    };

    for node in kpi_nodes {
        let source_doc_count = match store.count_source_documents_for_node(&node.id).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("discovery worker: count_source_documents failed: {e}");
                continue;
            }
        };

        let evidence_count = match store.count_evidence_for_node(&node.id).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("discovery worker: count_evidence failed: {e}");
                continue;
            }
        };

        if source_doc_count < 2 || evidence_count < 3 {
            continue;
        }

        let already_proposed = match store.check_candidate_exists_by_name(&node.name).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("discovery worker: check_candidate failed: {e}");
                continue;
            }
        };

        if already_proposed {
            continue;
        }

        let definition = node
            .attrs
            .get("definition")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        // Aggregate confidence across evidence for the candidate score.
        let confidence =
            Confidence((node.confidence.0 * evidence_count.min(10) as f32 / 10.0).clamp(0.0, 1.0));

        let mutation = GraphMutation::ProposeKpiCandidate {
            candidate: KpiCandidate {
                id: CandidateId(Uuid::new_v4().to_string()),
                name: node.name.clone(),
                definition,
                detected_in: vec![node.id.clone()],
                confidence,
                proposed_at: Utc::now(),
                status: CandidateStatus::Pending,
                merged_into: None,
            },
        };

        if let Err(e) = store.apply_mutation(mutation, cache, event_tx).await {
            tracing::warn!(
                "discovery worker: propose_candidate failed for '{}': {e}",
                node.name
            );
        } else {
            tracing::info!("discovery worker: proposed candidate '{}'", node.name);
        }
    }
}
