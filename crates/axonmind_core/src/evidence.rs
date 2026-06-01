use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::confidence::Confidence;
use crate::node::NodeId;

/// Evidence identifier. Always UUID v4 as string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EvidenceId(pub String);

impl From<String> for EvidenceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for EvidenceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Used in `Edge.evidence`. Type alias for clarity; logically a foreign key into the evidence table.
pub type EvidenceRef = EvidenceId;

/// Confidence defaults by extractor:
/// - `Manual`: 1.00 — user typed it, treated as ground truth
/// - `Connector`: 0.95 — structured field from a managed connector
/// - `Rule`: 0.85 — deterministic regex/schema match
/// - `Llm`: 0.50 — LLM-extracted, single source
/// - `Calculated`: inherits from sources (e.g. trend = min(confidence(t), confidence(t-1)))
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ExtractorKind {
    Manual,
    Connector,
    Rule,
    Llm,
    Calculated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SourceType {
    Document,
    Table,
    Note,
    Meeting,
    Manual,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Evidence {
    pub id: EvidenceId,
    pub source_node_id: NodeId,
    pub source_type: SourceType,
    pub quote: Option<String>,
    /// Structured location reference. E.g. `"Sheet1!B7"` for CSV, `"p.12 §3"` for documents.
    pub row_ref: Option<String>,
    /// SHA-256 of the corresponding blob in `blobs/<sha256>`. Used by recompute worker.
    pub blob_sha256: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
    pub extractor: ExtractorKind,
    pub confidence: Confidence,
    pub is_tainted: bool,
    pub requires_human_review: bool,
}
