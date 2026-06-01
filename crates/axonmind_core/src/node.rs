use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::confidence::Confidence;

/// Stable node identifier.
///
/// ID strategy (matches soverex-open):
/// - Canonical business objects: deterministic slug set by the user or a rule.
///   Examples: `"kpi.revenue_growth"`, `"team.engineering"`, `"doc.8f42a91c"`.
/// - Extracted/generated/proposed objects: `uuid::Uuid::new_v4().to_string()`.
/// - Document nodes: `format!("doc.{}", &sha256[..8])`.
///
/// The slug prefix (`kpi.`, `doc.`, `team.`, etc.) is a convention, not enforced by the type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct NodeId(pub String);

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for NodeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for NodeId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum NodeKind {
    Kpi,
    Metric,
    Objective,
    Initiative,
    Risk,
    Opportunity,
    Decision,
    Insight,
    Document,
    Person,
    Team,
    Customer,
    Function,
    Product,
    Market,
    Process,
    System,
    Action,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Kind-specific payload. `NodeKind::Kpi` nodes use `KpiAttrs`; others use arbitrary JSON.
    /// Stored as MessagePack BLOB in SQLite (`nodes.attrs`); JSON in transit and export.
    pub attrs: serde_json::Value,
    pub confidence: Confidence,
    pub is_tainted: bool,
    pub requires_human_review: bool,
}
