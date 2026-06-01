use super::{ExplainKpiInput, ExplainKpiOutput, GetEvidenceInput, GetEvidenceOutput};
use crate::store::{GraphCache, GraphStore};
use axonmind_core::{AxonMindError, NodeKind};

pub async fn get_evidence(
    input: GetEvidenceInput,
    store: &GraphStore,
) -> Result<GetEvidenceOutput, AxonMindError> {
    let evidence = match (input.node_id, input.edge_id) {
        (Some(node_id), _) => store.fetch_evidence_for_node(&node_id).await?,
        (None, Some(edge_id)) => store.fetch_evidence_for_edge(&edge_id).await?,
        (None, None) => {
            return Err(AxonMindError::ValidationFailed {
                message: "get_evidence requires either node_id or edge_id".into(),
            });
        }
    };
    Ok(GetEvidenceOutput { evidence })
}

pub async fn explain_kpi(
    input: ExplainKpiInput,
    store: &GraphStore,
    _cache: &tokio::sync::RwLock<GraphCache>,
    llm: Option<&dyn crate::extract::llm::LlmProvider>,
) -> Result<ExplainKpiOutput, AxonMindError> {
    let kpi = store
        .fetch_node(&input.kpi_id)
        .await?
        .ok_or_else(|| AxonMindError::NodeNotFound(input.kpi_id.clone()))?;

    if kpi.kind != NodeKind::Kpi {
        return Err(AxonMindError::NotAKpi(input.kpi_id.clone()));
    }

    let evidence = store.fetch_evidence_for_node(&input.kpi_id).await?;

    let confidence = if evidence.is_empty() {
        0.0
    } else {
        axonmind_core::Confidence::aggregate(
            &evidence.iter().map(|e| e.confidence).collect::<Vec<_>>(),
        )
        .0
    };

    // Build deterministic rationale from evidence (fallback and LLM input).
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("KPI: {}", kpi.name));
    if let Some(def) = kpi.attrs.get("definition").and_then(|v| v.as_str()) {
        if !def.is_empty() {
            parts.push(format!("Definition: {def}"));
        }
    }
    for ev in &evidence {
        if let Some(quote) = &ev.quote {
            parts.push(format!(
                "Evidence (confidence {:.0}%): {quote}",
                ev.confidence.0 * 100.0
            ));
        }
    }

    // Phase 3: LLM rationale replaces the deterministic concatenation when available.
    let rationale = if let Some(llm) = llm {
        let quotes: Vec<String> = evidence.iter().filter_map(|e| e.quote.clone()).collect();
        if !quotes.is_empty() {
            match llm.explain_kpi_rationale(&kpi.name, &quotes).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("LLM rationale failed for {}: {e}", kpi.name);
                    parts.join("\n\n")
                }
            }
        } else {
            parts.join("\n\n")
        }
    } else {
        parts.join("\n\n")
    };

    Ok(ExplainKpiOutput {
        rationale,
        evidence,
        confidence,
    })
}
