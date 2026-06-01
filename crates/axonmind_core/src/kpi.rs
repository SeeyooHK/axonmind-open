use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::node::NodeId;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum KpiUnit {
    Percent,
    /// ISO 4217 currency code, e.g. `"USD"`.
    Currency(String),
    Count,
    Ratio,
    Duration,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Period {
    Quarter,
    Month,
    Week,
    YearToDate,
    Custom(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum KpiStatus {
    Healthy,
    AtRisk,
    Critical,
    Unknown,
}

/// Computed by `kpi_recompute_worker`. Never set manually.
/// `Up`/`Down`/`Flat` = sign(value_t - value_{t-1}).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum KpiTrend {
    Up,
    Down,
    Flat,
    Unknown,
}

/// Stored as MessagePack inside `nodes.attrs` for `NodeKind::Kpi` nodes.
/// Parsed and validated by `GraphStore::apply_mutation` on every upsert.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KpiAttrs {
    pub value: Option<f64>,
    pub unit: KpiUnit,
    pub period: Period,
    pub status: KpiStatus,
    pub trend: KpiTrend,
    pub target: Option<f64>,
    /// Points to a `NodeKind::Person` or `NodeKind::Team` node.
    pub owner_node_id: Option<NodeId>,
    pub definition: String,
    /// Points to `NodeKind::Document` nodes that are the authoritative sources.
    pub source_refs: Vec<NodeId>,
    pub explanation: Option<String>,
    pub last_recomputed_at: Option<DateTime<Utc>>,
}
