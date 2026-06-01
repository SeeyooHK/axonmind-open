/// Blocker C: All 7 MVP tool input/output schemas.
///
/// These structs are the authoritative contract for:
/// - CLI `--json` output (must serialize identically)
/// - `AxonMindTransport` TypeScript interface (generated via ts-rs)
/// - MCP tool handlers in `mcp/schemas.rs`
///
/// Error behavior (all query functions):
/// - Missing node → `AxonMindError::NodeNotFound`
/// - Node exists but wrong kind → `AxonMindError::NotAKpi` (for KPI tools)
/// - `requires_human_review` objects: included in output with flags set; never silently hidden
/// - `is_tainted` objects: included in output with flags set
pub mod evidence;
pub mod focus;
pub mod impact;
pub mod reasoning;
pub mod search;

pub use reasoning::{ReasoningSearchInput, ReasoningSearchOutput, RetrievedSection};

use axonmind_core::{Edge, Evidence, EvidenceId, Node, NodeId, NodeKind};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

// ── Helper types used across multiple tools ──────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EdgeWithNodes {
    pub edge: Edge,
    pub from: Node,
    pub to: Node,
}

// ── focus_kpi ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FocusKpiInput {
    pub kpi_id: NodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FocusKpiOutput {
    pub kpi: Node,
    /// Edges with kind in {Influences, Improves, Causes} where to_id == kpi_id.
    pub drivers: Vec<EdgeWithNodes>,
    /// Edges with kind in {Blocks, Degrades} where to_id == kpi_id.
    pub blockers: Vec<EdgeWithNodes>,
    /// Outgoing edges from the KPI node where the target kind == Risk.
    pub risks: Vec<EdgeWithNodes>,
    pub owner: Option<Node>,
    pub evidence_count: usize,
}

// ── explain_kpi ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExplainKpiInput {
    pub kpi_id: NodeId,
    /// How many hops of drivers/blockers to include in the rationale. Default: 2.
    pub depth: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExplainKpiOutput {
    /// Rationale built from evidence quotes. Phase 3: LLM-generated and cached by (kpi_id, evidence_hash).
    /// Phase 1: deterministic text assembled from evidence.quote fields.
    pub rationale: String,
    pub evidence: Vec<Evidence>,
    pub confidence: f32,
}

// ── get_evidence ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GetEvidenceInput {
    /// Exactly one of edge_id or node_id must be set.
    pub edge_id: Option<axonmind_core::EdgeId>,
    pub node_id: Option<NodeId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GetEvidenceOutput {
    pub evidence: Vec<Evidence>,
}

// ── impact_radius ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImpactRadiusInput {
    pub node_id: NodeId,
    /// Maximum graph traversal depth. Default: 3.
    pub max_depth: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImpactRadiusOutput {
    pub affected: Vec<AffectedNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AffectedNode {
    pub node: Node,
    pub depth: u32,
    pub path: Vec<NodeId>,
}

// ── trace_decision ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TraceDecisionInput {
    pub decision_node_id: NodeId,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TraceDecisionOutput {
    pub decision: Node,
    /// Edges of kind DecidedBy or Causes pointing to this decision.
    pub caused_by: Vec<EdgeWithNodes>,
    pub evidenced_by: Vec<Evidence>,
    /// Edges of kind NextAction from this decision.
    pub next_actions: Vec<EdgeWithNodes>,
}

// ── suggest_actions ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SuggestActionsInput {
    pub kpi_id: NodeId,
    /// Filter to only suggest actions relevant to these statuses. Default: all.
    pub status_filter: Option<Vec<axonmind_core::KpiStatus>>,
    /// If false (default), exclude actions linked only through review-required edges.
    pub include_unreviewed: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SuggestActionsOutput {
    pub actions: Vec<Node>,
}

// ── graph_search ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GraphSearchInput {
    pub query: String,
    /// Filter by node kind. Default: all kinds.
    pub kinds: Option<Vec<NodeKind>>,
    /// Maximum results. Default: 20.
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GraphSearchOutput {
    pub nodes: Vec<Node>,
    /// Which FTS5 columns matched for each result (parallel to `nodes`).
    pub matched_via: Vec<Vec<SearchMatchKind>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SearchMatchKind {
    Name,
    Definition,
    EvidenceQuote,
}

// ── Graph export format (CLI export-json) ────────────────────────────────────

/// Schema v1 export. `schema_version` must be checked on import.
/// Blobs are NOT included by default; only `blob_sha256` references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphExportV1 {
    pub schema_version: u32,
    pub exported_at: chrono::DateTime<chrono::Utc>,
    pub workspace_id: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub evidence: Vec<Evidence>,
    pub edge_evidence: Vec<(axonmind_core::EdgeId, EvidenceId)>,
    pub metric_values: Vec<crate::store::MetricValue>,
    pub kpi_candidates: Vec<crate::store::KpiCandidate>,
}
