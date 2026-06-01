use thiserror::Error;

use crate::node::NodeId;

#[derive(Debug, Error)]
pub enum AxonMindError {
    #[error("node not found: {0}")]
    NodeNotFound(NodeId),

    #[error("edge must include at least one evidence reference")]
    EvidenceMissing,

    #[error("validation failed: {message}")]
    ValidationFailed { message: String },

    #[error("operation requires human review: {reason}")]
    RequiresHumanReview { reason: String },

    #[error("node is not a KPI: {0}")]
    NotAKpi(NodeId),

    #[error("database error: {0}")]
    Database(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("workspace not found at {path}")]
    WorkspaceNotFound { path: String },

    #[error("tool not found: {name}")]
    ToolNotFound { name: String },

    #[error("llm provider error: {0}")]
    LlmProvider(String),

    #[error("ingest error: {message}")]
    Ingest { message: String },

    #[error("graph cache is dirty; rebuild before serving queries")]
    CacheDirty,
}
