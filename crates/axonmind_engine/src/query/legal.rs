use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LegalLocator {
    pub article: Option<String>,
    pub recital: Option<String>,
    pub section: Option<String>,
    pub paragraph: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PageLocator {
    pub start: Option<i64>,
    pub end: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ResolvedDocument {
    pub doc_id: String,
    pub canonical_title: String,
    pub source_filename: String,
    pub aliases: Vec<String>,
    pub language: Option<String>,
    pub corpus: Vec<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentResolveInput {
    pub query: Option<String>,
    pub corpus: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentResolveOutput {
    pub documents: Vec<ResolvedDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentSearchInput {
    pub query: String,
    pub doc_ids: Option<Vec<String>>,
    pub corpus: Option<String>,
    pub unit_types: Option<Vec<String>>,
    pub top_k: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentSearchResult {
    pub doc_id: String,
    pub section_id: String,
    pub unit_id: Option<String>,
    pub locator: Option<LegalLocator>,
    pub page: PageLocator,
    pub title: String,
    pub snippet: String,
    pub score_source: String,
    pub citation_safe: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentSearchOutput {
    pub results: Vec<DocumentSearchResult>,
    pub reasoning_applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentReadSectionInput {
    pub doc_id: String,
    pub locator: Option<LegalLocator>,
    pub section_id: Option<String>,
    pub label_norm: Option<String>,
    pub include_children: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentReadSectionOutput {
    pub doc_id: String,
    pub unit_id: String,
    pub section_id: String,
    pub title: String,
    pub locator: LegalLocator,
    pub page: PageLocator,
    pub text: String,
    pub citation_safe: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentQuoteInput {
    pub doc_id: String,
    pub locator: Option<LegalLocator>,
    pub section_id: Option<String>,
    pub label_norm: Option<String>,
    pub max_chars: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentQuoteOutput {
    pub quote: String,
    pub citation: String,
    pub locator: LegalLocator,
    pub page: PageLocator,
    pub truncated: bool,
    pub citation_safe: bool,
}
