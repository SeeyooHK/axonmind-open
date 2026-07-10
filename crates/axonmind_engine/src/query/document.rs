use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Arbitrary locator fields for a parsed unit (e.g. `article`/`paragraph` for a legal
/// instrument, `dosage`/`frequency` for a medical guideline) — whatever capture names a
/// structure package's grammar declares. A single-field tuple struct ("newtype") already
/// serializes transparently in JSON by default (no `#[serde(transparent)]` needed) — the wire
/// shape stays a flat object (`{"article":"33"}`), matching the pre-generic locator shape.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UnitLocator(pub BTreeMap<String, String>);

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

/// One row per document for the Library review UI (`retrieve_guarantee.md` item 4a): derived
/// identity plus which package/profile parsed it and how many units it produced. `profile_name`/
/// `profile_version`/`unit_count` are `None`/`0` for a document with no `doc_units` rows (no
/// structure package ever matched it, or it was never parsed past PageIndex).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentIdentityReport {
    pub doc_node_id: String,
    pub source_filename: String,
    pub source_path: Option<String>,
    pub canonical_title: String,
    pub instrument_type: Option<String>,
    pub corpus: Vec<String>,
    pub confidence: f32,
    pub profile_name: Option<String>,
    pub profile_version: Option<i64>,
    pub unit_count: i64,
    pub pinned_profile: Option<String>,
    pub updated_at: i64,
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
    pub locator: Option<UnitLocator>,
    pub page: PageLocator,
    pub title: String,
    pub snippet: String,
    pub score_source: String,
    pub citation_safe: bool,
    /// Content sha256 of the document at the time this unit was parsed (empty when the hit has
    /// no backing `doc_units` row, e.g. a generic PageIndex section match with no structure
    /// package involved). Content-addressed, so it alone identifies the exact document version
    /// this citation came from — see `retrieve_guarantee.md` item 5.
    pub doc_sha256: String,
    /// Structure package that claimed the unit, and the package version *in effect when this
    /// unit was parsed* (not necessarily the package's current version) — empty/0 when there is
    /// no backing `doc_units` row.
    pub package_name: String,
    pub package_version: i64,
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
    pub locator: Option<UnitLocator>,
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
    pub locator: UnitLocator,
    pub page: PageLocator,
    pub text: String,
    pub citation: String,
    pub citation_safe: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentQuoteInput {
    pub doc_id: String,
    pub locator: Option<UnitLocator>,
    pub section_id: Option<String>,
    pub label_norm: Option<String>,
    pub max_chars: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocumentQuoteOutput {
    pub quote: String,
    pub citation: String,
    pub locator: UnitLocator,
    pub page: PageLocator,
    pub truncated: bool,
    pub citation_safe: bool,
}
