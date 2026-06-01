use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::confidence::Confidence;
use crate::evidence::{EvidenceRef, ExtractorKind};
use crate::node::NodeId;

/// Edge identifier. Always UUID v4 as string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EdgeId(pub String);

impl From<String> for EdgeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for EdgeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum EdgeKind {
    Influences,
    Causes,
    CorrelatesWith,
    DependsOn,
    DerivedFrom,
    Blocks,
    Improves,
    Degrades,
    OwnedBy,
    MeasuredBy,
    EvidencedBy,
    MentionedIn,
    DecidedBy,
    AssignedTo,
    InFunction,
    ForProduct,
    Impacts,
    NextAction,
    /// An external source explicitly contradicts this node's claim or value.
    /// Presence of this edge triggers `requires_human_review` and dampens confidence
    /// via `Confidence::aggregate_signed`.
    Contradicts,
    /// An external source explicitly corroborates this node's claim or value.
    /// Treated as supporting evidence in `aggregate_signed`.
    Corroborates,
}

/// A directed edge in the knowledge graph.
///
/// Invariant: `evidence` must be non-empty. `GraphStore::apply_mutation` rejects
/// `GraphMutation::UpsertEdge` with empty `evidence_ids` via `AxonMindError::EvidenceMissing`.
/// This is the single most important constraint in the system.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Edge {
    pub id: EdgeId,
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
    /// Non-empty. At least one `EvidenceRef` required — enforced at store level.
    pub evidence: Vec<EvidenceRef>,
    pub confidence: Confidence,
    pub created_at: DateTime<Utc>,
    pub created_by: ExtractorKind,
    pub is_tainted: bool,
    pub requires_human_review: bool,
}
