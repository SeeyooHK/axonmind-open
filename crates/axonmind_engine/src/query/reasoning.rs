use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReasoningSearchInput {
    pub query: String,
    /// Restrict results to these document node ids. None / empty = whole corpus.
    pub doc_node_ids: Option<Vec<String>>,
    /// Maximum results to return. Default: 20.
    pub max_results: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReasoningSearchOutput {
    pub sections: Vec<RetrievedSection>,
    /// false = BM25-only (no LLM provider present); true = LLM-reranked.
    pub reasoning_applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RetrievedSection {
    /// Bridges to the graph Document node (NodeKind::Document).
    pub doc_node_id: String,
    pub section_id: String,
    pub title: String,
    pub text: String,
    pub span_start: usize,
    pub span_end: usize,
    /// Breadcrumb path from document root to this section.
    pub path: Vec<String>,
}
