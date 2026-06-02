use axonmind_core::AxonMindError;
use serde_json::Value;

use crate::AxonMindEngine;
use crate::mcp::schemas::{self, ToolDef};
use crate::query::{
    ExplainKpiInput, FocusKpiInput, GetEvidenceInput, GraphSearchInput, ImpactRadiusInput,
    ReasoningSearchInput, SuggestActionsInput, TraceDecisionInput,
};

pub fn list_tools() -> Vec<ToolDef> {
    schemas::tool_defs()
}

pub async fn call_tool(
    engine: &AxonMindEngine,
    name: &str,
    arguments: Value,
) -> Result<Value, AxonMindError> {
    let args = if arguments.is_null() {
        Value::Object(Default::default())
    } else {
        arguments
    };

    macro_rules! parse {
        ($t:ty) => {
            serde_json::from_value::<$t>(args).map_err(|e| AxonMindError::ValidationFailed {
                message: format!("invalid arguments for {name}: {e}"),
            })?
        };
    }

    macro_rules! serialize {
        ($v:expr) => {
            serde_json::to_value($v).map_err(|e| AxonMindError::Serialization(e.to_string()))?
        };
    }

    match name {
        "focus_kpi" => Ok(serialize!(engine.focus_kpi(parse!(FocusKpiInput)).await?)),
        "explain_kpi" => Ok(serialize!(
            engine.explain_kpi(parse!(ExplainKpiInput)).await?
        )),
        "get_evidence" => Ok(serialize!(
            engine.get_evidence(parse!(GetEvidenceInput)).await?
        )),
        "impact_radius" => Ok(serialize!(
            engine.impact_radius(parse!(ImpactRadiusInput)).await?
        )),
        "trace_decision" => Ok(serialize!(
            engine.trace_decision(parse!(TraceDecisionInput)).await?
        )),
        "suggest_actions" => Ok(serialize!(
            engine.suggest_actions(parse!(SuggestActionsInput)).await?
        )),
        "graph_search" => Ok(serialize!(
            engine.graph_search(parse!(GraphSearchInput)).await?
        )),
        "reasoning_search" => Ok(serialize!(
            engine
                .reasoning_search(parse!(ReasoningSearchInput))
                .await?
        )),
        _ => Err(AxonMindError::ToolNotFound {
            name: name.to_owned(),
        }),
    }
}
