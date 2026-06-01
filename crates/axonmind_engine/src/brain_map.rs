use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use axonmind_core::{AxonMindError, Edge, EdgeKind, Node, NodeKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::extract::summarize::{SuggestedCategory, SuggestedSummary};

const DEFAULT_SUMMARY_NAME: &str = "Default Summary";
const DEFAULT_PERIOD: &str = "latest";
const DEFAULT_AS_OF: &str = "latest";
const SCOPED_SUMMARY_CACHE_VERSION: &str = "scoped_cache_v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryDefinition {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_period: Option<String>,
    #[serde(default)]
    pub default_as_of: Option<String>,
    pub lenses: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LensContext {
    #[serde(default)]
    pub inherit_selector: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensDefinition {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub hidden: Option<bool>,
    #[serde(default)]
    pub context: Option<LensContext>,
    pub selector: Value,
    pub measure: Value,
    #[serde(default)]
    pub health: Option<Value>,
    #[serde(default)]
    pub children: Vec<String>,
    #[serde(default)]
    pub headline_node_id: Option<String>,
    #[serde(default)]
    pub member_node_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryConfig {
    pub summary: SummaryDefinition,
    pub lenses: Vec<LensDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectiveLensContext {
    pub lens_id: String,
    pub effective_selector: Value,
    pub effective_period: String,
    pub effective_as_of: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryConfigSnapshot {
    pub config_path: String,
    pub config_exists: bool,
    pub config: SummaryConfig,
    pub effective_contexts: Vec<EffectiveLensContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SummaryConfigEdit {
    #[serde(default)]
    pub summary_name: Option<String>,
    #[serde(default)]
    pub default_period: Option<String>,
    #[serde(default)]
    pub default_as_of: Option<String>,
    #[serde(default)]
    pub lens_order: Option<Vec<String>>,
    #[serde(default)]
    pub lenses: Vec<LensEdit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LensEdit {
    pub lens_id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub hidden: Option<bool>,
    #[serde(default)]
    pub health: Option<Value>,
    #[serde(default)]
    pub measure_period: Option<String>,
    #[serde(default)]
    pub measure_as_of: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasureState {
    Resolved,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Good,
    Watch,
    AtRisk,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeasureResolution {
    #[serde(rename = "type")]
    pub measure_type: String,
    pub state: MeasureState,
    pub value: Option<f64>,
    pub unit: Option<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub observed_at: Option<String>,
    pub explanation: Option<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    #[serde(default)]
    pub evidence_lineage: Vec<EvidenceLineageItem>,
    #[serde(default)]
    pub lineage_gaps: Vec<LineageGap>,
    #[serde(default)]
    pub supporting_nodes: Vec<SupportingNodeRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LensResolution {
    pub lens_id: String,
    pub label: String,
    #[serde(default)]
    pub child_lens_ids: Vec<String>,
    pub selected_node_ids: Vec<String>,
    pub effective_context: EffectiveLensContext,
    pub measure_rule: Value,
    pub measure: MeasureResolution,
    pub health: HealthResolution,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResolution {
    pub state: HealthState,
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceLineageItem {
    pub evidence_id: String,
    pub source_node_id: String,
    pub source_node_name: String,
    pub source_type: String,
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub row_ref: Option<String>,
    #[serde(default)]
    pub quote: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageGap {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportingNodeRef {
    pub node_id: String,
    pub label: String,
    pub kind: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryResolution {
    pub summary_id: String,
    pub summary_name: String,
    pub source: String,
    pub lenses: Vec<LensResolution>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopedSummaryMode {
    Auto,
    CachedOnly,
    Regenerate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopedSummaryCacheEntry {
    pub scope_key: String,
    pub input_signature: String,
    pub summary: SuggestedSummary,
    pub created_at: i64,
    pub updated_at: i64,
}

impl SummaryConfig {
    pub fn from_suggested(summary: SuggestedSummary, nodes: &[Node]) -> Self {
        let kind_by_node_id: HashMap<&str, String> = nodes
            .iter()
            .map(|n| (n.id.0.as_str(), format!("{:?}", n.kind)))
            .collect();
        let mut used_ids = HashSet::<String>::new();
        let mut summary_lenses = Vec::with_capacity(summary.categories.len());
        let mut lenses = Vec::with_capacity(summary.categories.len());

        for (idx, cat) in summary.categories.into_iter().enumerate() {
            let lens_id = unique_lens_id(&cat.label, idx, &mut used_ids);
            summary_lenses.push(lens_id.clone());

            let selector = selector_for_members(&cat.member_node_ids, &kind_by_node_id);
            let lens = LensDefinition {
                id: lens_id,
                label: cat.label,
                description: None,
                hidden: None,
                context: Some(LensContext {
                    inherit_selector: Some(true),
                }),
                selector,
                measure: json!({
                    "type": "count",
                    "period": DEFAULT_PERIOD,
                    "as_of": DEFAULT_AS_OF
                }),
                health: Some(json!({
                    "type": "presence",
                    "unknown_when": "missing_or_low_confidence"
                })),
                children: Vec::new(),
                headline_node_id: Some(cat.headline_node_id),
                member_node_ids: cat.member_node_ids,
            };
            lenses.push(lens);
        }

        Self {
            summary: SummaryDefinition {
                id: "default".to_string(),
                name: DEFAULT_SUMMARY_NAME.to_string(),
                description: Some("Generated default summary".to_string()),
                default_period: Some(DEFAULT_PERIOD.to_string()),
                default_as_of: Some(DEFAULT_AS_OF.to_string()),
                lenses: summary_lenses,
            },
            lenses,
        }
    }

    pub fn validate_and_compute_effective_contexts(
        &self,
    ) -> Result<Vec<EffectiveLensContext>, AxonMindError> {
        if self.summary.lenses.len() > 10 {
            return Err(AxonMindError::ValidationFailed {
                message: format!(
                    "summary '{}' has {} top-level lenses (max 10)",
                    self.summary.id,
                    self.summary.lenses.len()
                ),
            });
        }

        let mut seen_ids = HashSet::new();
        let mut by_id = HashMap::new();
        for lens in &self.lenses {
            if !seen_ids.insert(lens.id.clone()) {
                return Err(AxonMindError::ValidationFailed {
                    message: format!("duplicate lens id '{}'", lens.id),
                });
            }
            validate_selector(&lens.selector, &format!("lens '{}'", lens.id))?;
            validate_measure(&lens.measure, &format!("lens '{}'", lens.id))?;
            if let Some(health) = &lens.health {
                validate_health(health, &format!("lens '{}'", lens.id))?;
            }
            by_id.insert(lens.id.clone(), lens);
        }

        for lens_id in &self.summary.lenses {
            if !by_id.contains_key(lens_id) {
                return Err(AxonMindError::ValidationFailed {
                    message: format!(
                        "summary '{}' references missing lens '{}'",
                        self.summary.id, lens_id
                    ),
                });
            }
        }

        for lens in &self.lenses {
            if lens.children.len() > 10 {
                return Err(AxonMindError::ValidationFailed {
                    message: format!(
                        "lens '{}' has {} children (max 10)",
                        lens.id,
                        lens.children.len()
                    ),
                });
            }
            for child_id in &lens.children {
                if !by_id.contains_key(child_id) {
                    return Err(AxonMindError::ValidationFailed {
                        message: format!(
                            "lens '{}' references missing child '{}'",
                            lens.id, child_id
                        ),
                    });
                }
            }
        }

        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();
        let mut out = Vec::new();

        for lens_id in &self.summary.lenses {
            let inherited_period = self.summary.default_period.clone();
            let inherited_as_of = self.summary.default_as_of.clone();
            walk_lens(
                lens_id,
                &by_id,
                None,
                inherited_period,
                inherited_as_of,
                &mut visiting,
                &mut visited,
                &mut out,
            )?;
        }
        Ok(out)
    }

    pub fn to_suggested_summary(&self, nodes: &[Node], source: String) -> SuggestedSummary {
        let name_by_id: HashMap<&str, &str> = nodes
            .iter()
            .map(|n| (n.id.0.as_str(), n.name.as_str()))
            .collect();
        let by_id: HashMap<&str, &LensDefinition> =
            self.lenses.iter().map(|l| (l.id.as_str(), l)).collect();

        let mut categories = Vec::new();
        for lens_id in &self.summary.lenses {
            let Some(lens) = by_id.get(lens_id.as_str()) else {
                continue;
            };
            if is_hidden_lens(lens) {
                continue;
            }
            let headline_node_id = lens
                .headline_node_id
                .clone()
                .or_else(|| lens.member_node_ids.first().cloned())
                .unwrap_or_else(|| lens.id.clone());

            categories.push(SuggestedCategory {
                label: lens.label.clone(),
                headline_node_id,
                member_node_ids: lens.member_node_ids.clone(),
            });
        }

        let mut labels = HashMap::<String, String>::new();
        for cat in &categories {
            for id in std::iter::once(&cat.headline_node_id).chain(cat.member_node_ids.iter()) {
                if let Some(name) = name_by_id.get(id.as_str()) {
                    labels.insert(id.clone(), (*name).to_string());
                }
            }
        }

        SuggestedSummary {
            categories,
            source,
            labels,
        }
    }
}

pub fn apply_summary_config_edit(
    cfg: &mut SummaryConfig,
    edit: SummaryConfigEdit,
) -> Result<(), AxonMindError> {
    if let Some(name) = edit.summary_name {
        let name = name.trim();
        if !name.is_empty() {
            cfg.summary.name = name.to_string();
        }
    }
    if let Some(period) = edit.default_period {
        let period = period.trim();
        if !period.is_empty() {
            cfg.summary.default_period = Some(period.to_string());
        }
    }
    if let Some(as_of) = edit.default_as_of {
        let as_of = as_of.trim();
        if !as_of.is_empty() {
            cfg.summary.default_as_of = Some(as_of.to_string());
        }
    }

    if let Some(order) = edit.lens_order {
        let known: HashSet<&str> = cfg.lenses.iter().map(|l| l.id.as_str()).collect();
        let mut seen = HashSet::<String>::new();
        let mut reordered = Vec::<String>::new();
        for lens_id in order {
            if !known.contains(lens_id.as_str()) {
                return Err(AxonMindError::ValidationFailed {
                    message: format!("cannot reorder unknown lens id '{}'", lens_id),
                });
            }
            if seen.insert(lens_id.clone()) {
                reordered.push(lens_id);
            }
        }
        for lens_id in &cfg.summary.lenses {
            if seen.insert(lens_id.clone()) {
                reordered.push(lens_id.clone());
            }
        }
        cfg.summary.lenses = reordered;
    }

    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (idx, lens) in cfg.lenses.iter().enumerate() {
        by_id.insert(lens.id.clone(), idx);
    }
    for lens_edit in edit.lenses {
        let Some(idx) = by_id.get(&lens_edit.lens_id).copied() else {
            return Err(AxonMindError::ValidationFailed {
                message: format!("cannot edit unknown lens id '{}'", lens_edit.lens_id),
            });
        };
        let lens = &mut cfg.lenses[idx];
        if let Some(label) = lens_edit.label {
            let label = label.trim();
            if !label.is_empty() {
                lens.label = label.to_string();
            }
        }
        if let Some(hidden) = lens_edit.hidden {
            lens.hidden = Some(hidden);
        }
        if let Some(health) = lens_edit.health {
            lens.health = Some(health);
        }
        if lens_edit.measure_period.is_some() || lens_edit.measure_as_of.is_some() {
            let mut obj = lens.measure.as_object().cloned().ok_or_else(|| {
                AxonMindError::ValidationFailed {
                    message: format!("lens '{}' has non-object measure", lens.id),
                }
            })?;
            if let Some(period) = lens_edit.measure_period {
                let period = period.trim();
                if !period.is_empty() {
                    obj.insert("period".to_string(), Value::String(period.to_string()));
                }
            }
            if let Some(as_of) = lens_edit.measure_as_of {
                let as_of = as_of.trim();
                if !as_of.is_empty() {
                    obj.insert("as_of".to_string(), Value::String(as_of.to_string()));
                }
            }
            lens.measure = Value::Object(obj);
        }
    }

    Ok(())
}

pub fn default_summary_path(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join("summaries").join("default.json")
}

pub fn scoped_summary_scope_key(doc_ids: &[String]) -> String {
    let mut ids = doc_ids.to_vec();
    ids.sort();
    ids.dedup();
    let mut hasher = Sha256::new();
    hasher.update(SCOPED_SUMMARY_CACHE_VERSION.as_bytes());
    hasher.update(b"|");
    for id in ids {
        hasher.update(id.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

pub fn scoped_summary_input_signature(
    doc_signatures: &[(String, Option<String>, Option<String>)],
    llm_enabled: bool,
) -> String {
    let mut rows = doc_signatures.to_vec();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    hasher.update(SCOPED_SUMMARY_CACHE_VERSION.as_bytes());
    hasher.update(if llm_enabled { b"|llm=1" } else { b"|llm=0" });
    for (id, sha256, path) in rows {
        hasher.update(b"\n");
        hasher.update(id.as_bytes());
        hasher.update(b"|sha256=");
        hasher.update(sha256.unwrap_or_default().as_bytes());
        hasher.update(b"|path=");
        hasher.update(path.unwrap_or_default().as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

pub fn scoped_summary_cache_path(workspace_dir: &Path, scope_key: &str) -> PathBuf {
    workspace_dir
        .join("summaries")
        .join("scoped")
        .join(format!("{scope_key}.json"))
}

pub async fn load_scoped_summary_cache(
    workspace_dir: &Path,
    scope_key: &str,
) -> Result<Option<ScopedSummaryCacheEntry>, AxonMindError> {
    let path = scoped_summary_cache_path(workspace_dir, scope_key);
    if tokio::fs::try_exists(&path).await? {
        let raw = tokio::fs::read_to_string(&path).await?;
        let entry = serde_json::from_str::<ScopedSummaryCacheEntry>(&raw).map_err(|e| {
            AxonMindError::Serialization(format!(
                "invalid scoped summary cache '{}': {e}",
                path.display()
            ))
        })?;
        Ok(Some(entry))
    } else {
        Ok(None)
    }
}

pub async fn save_scoped_summary_cache(
    workspace_dir: &Path,
    entry: &ScopedSummaryCacheEntry,
) -> Result<(), AxonMindError> {
    let path = scoped_summary_cache_path(workspace_dir, &entry.scope_key);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let raw = serde_json::to_string_pretty(entry)
        .map_err(|e| AxonMindError::Serialization(format!("encode scoped summary cache: {e}")))?;
    tokio::fs::write(path, raw).await?;
    Ok(())
}

pub async fn load_default_summary(
    workspace_dir: &Path,
) -> Result<Option<SummaryConfig>, AxonMindError> {
    let path = default_summary_path(workspace_dir);
    if tokio::fs::try_exists(&path).await? {
        let raw = tokio::fs::read_to_string(&path).await?;
        let cfg = serde_json::from_str::<SummaryConfig>(&raw).map_err(|e| {
            AxonMindError::Serialization(format!(
                "invalid summary config '{}': {e}",
                path.display()
            ))
        })?;
        Ok(Some(cfg))
    } else {
        Ok(None)
    }
}

pub async fn save_default_summary(
    workspace_dir: &Path,
    cfg: &SummaryConfig,
) -> Result<(), AxonMindError> {
    let path = default_summary_path(workspace_dir);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let raw = serde_json::to_string_pretty(cfg)
        .map_err(|e| AxonMindError::Serialization(format!("encode summary config: {e}")))?;
    tokio::fs::write(path, raw).await?;
    Ok(())
}

pub fn filter_node_ids_for_selector(nodes: &[Node], selector: &Value) -> Vec<String> {
    filter_node_ids_for_selector_with_edges(nodes, &[], selector)
}

pub fn filter_node_ids_for_selector_with_edges(
    nodes: &[Node],
    edges: &[Edge],
    selector: &Value,
) -> Vec<String> {
    let nodes_by_id: HashMap<String, &Node> = nodes.iter().map(|n| (n.id.0.clone(), n)).collect();
    nodes
        .iter()
        .filter(|n| matches_selector(selector, n, &nodes_by_id, edges))
        .map(|n| n.id.0.clone())
        .collect()
}

fn walk_lens(
    lens_id: &str,
    by_id: &HashMap<String, &LensDefinition>,
    inherited_selector: Option<Value>,
    inherited_period: Option<String>,
    inherited_as_of: Option<String>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    out: &mut Vec<EffectiveLensContext>,
) -> Result<(), AxonMindError> {
    if !visiting.insert(lens_id.to_string()) {
        return Err(AxonMindError::ValidationFailed {
            message: format!("cycle detected at lens '{}'", lens_id),
        });
    }
    let lens = by_id
        .get(lens_id)
        .copied()
        .ok_or_else(|| AxonMindError::ValidationFailed {
            message: format!("missing lens '{}'", lens_id),
        })?;

    let inherit_selector = lens
        .context
        .as_ref()
        .and_then(|c| c.inherit_selector)
        .unwrap_or(true);
    let effective_selector = if inherit_selector {
        merge_selectors(inherited_selector, Some(lens.selector.clone()))
    } else {
        lens.selector.clone()
    };

    let effective_period = measure_string(&lens.measure, "period")
        .or(inherited_period.clone())
        .unwrap_or_else(|| DEFAULT_PERIOD.to_string());
    let effective_as_of = measure_string(&lens.measure, "as_of")
        .or(inherited_as_of.clone())
        .unwrap_or_else(|| DEFAULT_AS_OF.to_string());

    if !visited.contains(lens_id) {
        out.push(EffectiveLensContext {
            lens_id: lens.id.clone(),
            effective_selector: effective_selector.clone(),
            effective_period: effective_period.clone(),
            effective_as_of: effective_as_of.clone(),
        });
        visited.insert(lens_id.to_string());
    }

    for child_id in &lens.children {
        walk_lens(
            child_id,
            by_id,
            Some(effective_selector.clone()),
            Some(effective_period.clone()),
            Some(effective_as_of.clone()),
            visiting,
            visited,
            out,
        )?;
    }

    visiting.remove(lens_id);
    Ok(())
}

fn validate_selector(value: &Value, context: &str) -> Result<(), AxonMindError> {
    let obj = value
        .as_object()
        .ok_or_else(|| AxonMindError::ValidationFailed {
            message: format!("{context}: selector must be an object"),
        })?;
    let selector_type =
        obj.get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| AxonMindError::ValidationFailed {
                message: format!("{context}: selector.type is required"),
            })?;

    if selector_type == "composite" {
        let and_items = obj
            .get("and")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let or_items = obj
            .get("or")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if and_items.is_empty() && or_items.is_empty() {
            return Err(AxonMindError::ValidationFailed {
                message: format!("{context}: composite selector must include 'and' or 'or'"),
            });
        }
        for item in and_items.iter().chain(or_items.iter()) {
            validate_selector(item, context)?;
        }
    }
    Ok(())
}

fn matches_selector(
    selector: &Value,
    node: &Node,
    nodes_by_id: &HashMap<String, &Node>,
    edges: &[Edge],
) -> bool {
    let Some(obj) = selector.as_object() else {
        return false;
    };
    let Some(selector_type) = obj.get("type").and_then(Value::as_str) else {
        return false;
    };
    match selector_type {
        "kind" => {
            let Some(kind) = obj.get("kind").and_then(Value::as_str) else {
                return false;
            };
            kind.eq_ignore_ascii_case(node_kind_name(node.kind))
        }
        "facet" => {
            let Some(facet) = obj.get("facet").and_then(Value::as_str) else {
                return false;
            };
            let expected = obj.get("equals");
            match expected {
                Some(v) => {
                    matches_facet_edge(node, facet, v, nodes_by_id, edges)
                        || compare_field_equals(&node.attrs, facet, v)
                }
                None => false,
            }
        }
        "attribute" => {
            let Some(field) = obj.get("field").and_then(Value::as_str) else {
                return false;
            };
            let expected = obj.get("equals");
            match expected {
                Some(v) => compare_field_equals(&node.attrs, field, v),
                None => false,
            }
        }
        "composite" => {
            let and_ok = obj
                .get("and")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .all(|p| matches_selector(p, node, nodes_by_id, edges))
                })
                .unwrap_or(true);
            let or_ok = obj
                .get("or")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .any(|p| matches_selector(p, node, nodes_by_id, edges))
                })
                .unwrap_or(true);
            and_ok && or_ok
        }
        "saved_query" => {
            let query = obj.get("query").and_then(Value::as_str).unwrap_or_default();
            if query == "all_concepts" {
                node.kind != NodeKind::Document
            } else {
                false
            }
        }
        _ => false,
    }
}

fn matches_facet_edge(
    node: &Node,
    facet: &str,
    expected: &Value,
    nodes_by_id: &HashMap<String, &Node>,
    edges: &[Edge],
) -> bool {
    let Some((edge_kind, facet_kind)) = facet_mapping(facet) else {
        return false;
    };

    edges.iter().any(|e| {
        if e.kind != edge_kind {
            return false;
        }
        let other_id = if e.from == node.id {
            &e.to.0
        } else if e.to == node.id {
            &e.from.0
        } else {
            return false;
        };
        let Some(other) = nodes_by_id.get(other_id) else {
            return false;
        };
        if other.kind != facet_kind {
            return false;
        }

        compare_stringish(other.name.as_str(), expected)
            || compare_stringish(other.id.0.as_str(), expected)
            || other
                .attrs
                .get("name")
                .and_then(Value::as_str)
                .map(|v| compare_stringish(v, expected))
                .unwrap_or(false)
            || other
                .attrs
                .get("label")
                .and_then(Value::as_str)
                .map(|v| compare_stringish(v, expected))
                .unwrap_or(false)
    })
}

fn facet_mapping(facet: &str) -> Option<(EdgeKind, NodeKind)> {
    match facet.to_ascii_lowercase().as_str() {
        "function" => Some((EdgeKind::InFunction, NodeKind::Function)),
        "product" | "line_of_business" | "lineofbusiness" | "lob" => {
            Some((EdgeKind::ForProduct, NodeKind::Product))
        }
        _ => None,
    }
}

fn compare_stringish(actual: &str, expected: &Value) -> bool {
    match expected {
        Value::String(s) => actual.eq_ignore_ascii_case(s),
        Value::Number(n) => actual.eq_ignore_ascii_case(&n.to_string()),
        Value::Bool(b) => actual.eq_ignore_ascii_case(&b.to_string()),
        _ => false,
    }
}

fn validate_measure(value: &Value, context: &str) -> Result<(), AxonMindError> {
    let obj = value
        .as_object()
        .ok_or_else(|| AxonMindError::ValidationFailed {
            message: format!("{context}: measure must be an object"),
        })?;
    obj.get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| AxonMindError::ValidationFailed {
            message: format!("{context}: measure.type is required"),
        })?;
    Ok(())
}

fn validate_health(value: &Value, context: &str) -> Result<(), AxonMindError> {
    let obj = value
        .as_object()
        .ok_or_else(|| AxonMindError::ValidationFailed {
            message: format!("{context}: health must be an object"),
        })?;
    obj.get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| AxonMindError::ValidationFailed {
            message: format!("{context}: health.type is required"),
        })?;
    Ok(())
}

fn measure_string(measure: &Value, key: &str) -> Option<String> {
    measure
        .as_object()
        .and_then(|o| o.get(key))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn node_kind_name(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Kpi => "Kpi",
        NodeKind::Metric => "Metric",
        NodeKind::Objective => "Objective",
        NodeKind::Initiative => "Initiative",
        NodeKind::Risk => "Risk",
        NodeKind::Opportunity => "Opportunity",
        NodeKind::Decision => "Decision",
        NodeKind::Insight => "Insight",
        NodeKind::Document => "Document",
        NodeKind::Person => "Person",
        NodeKind::Team => "Team",
        NodeKind::Customer => "Customer",
        NodeKind::Function => "Function",
        NodeKind::Product => "Product",
        NodeKind::Market => "Market",
        NodeKind::Process => "Process",
        NodeKind::System => "System",
        NodeKind::Action => "Action",
    }
}

fn compare_field_equals(attrs: &Value, field: &str, expected: &Value) -> bool {
    let Some(actual) = field_value(attrs, field) else {
        return false;
    };
    if actual == expected {
        return true;
    }

    let actual_str = value_as_string(actual);
    let expected_str = value_as_string(expected);
    match (actual_str, expected_str) {
        (Some(a), Some(e)) => a.eq_ignore_ascii_case(&e),
        _ => false,
    }
}

fn field_value<'a>(value: &'a Value, field: &str) -> Option<&'a Value> {
    let mut cur = value;
    for part in field.split('.') {
        cur = cur.get(part)?;
    }
    Some(cur)
}

fn value_as_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn merge_selectors(left: Option<Value>, right: Option<Value>) -> Value {
    let mut parts = Vec::<Value>::new();
    if let Some(v) = left {
        append_selector_part(&mut parts, v);
    }
    if let Some(v) = right {
        append_selector_part(&mut parts, v);
    }

    if parts.len() == 1 {
        parts.remove(0)
    } else {
        json!({ "type": "composite", "and": parts })
    }
}

pub fn is_hidden_lens(lens: &LensDefinition) -> bool {
    lens.hidden.unwrap_or(false)
}

fn append_selector_part(parts: &mut Vec<Value>, selector: Value) {
    if let Some(obj) = selector.as_object() {
        let is_composite = obj.get("type").and_then(Value::as_str) == Some("composite");
        let has_or = obj
            .get("or")
            .and_then(Value::as_array)
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if is_composite && !has_or {
            if let Some(and) = obj.get("and").and_then(Value::as_array) {
                for nested in and {
                    parts.push(nested.clone());
                }
                return;
            }
        }
    }
    parts.push(selector);
}

fn selector_for_members(member_ids: &[String], kind_by_node_id: &HashMap<&str, String>) -> Value {
    let mut seen_kinds = HashSet::<String>::new();
    let mut kinds = Vec::<String>::new();
    for id in member_ids {
        if let Some(kind) = kind_by_node_id.get(id.as_str()) {
            if seen_kinds.insert(kind.clone()) {
                kinds.push(kind.clone());
            }
        }
    }

    if kinds.len() == 1 {
        return json!({ "type": "kind", "kind": kinds[0] });
    }

    if !kinds.is_empty() {
        let variants = kinds
            .into_iter()
            .map(|k| json!({ "type": "kind", "kind": k }))
            .collect::<Vec<_>>();
        return json!({ "type": "composite", "or": variants });
    }

    json!({ "type": "saved_query", "query": "all_concepts" })
}

fn unique_lens_id(label: &str, idx: usize, used: &mut HashSet<String>) -> String {
    let base = slugify(label);
    let mut candidate = if base.is_empty() {
        format!("lens_{}", idx + 1)
    } else {
        base
    };

    let mut suffix = 2usize;
    while used.contains(&candidate) {
        candidate = format!("{}_{}", slugify(label), suffix);
        suffix += 1;
    }
    used.insert(candidate.clone());
    candidate
}

fn slugify(label: &str) -> String {
    let mut out = String::new();
    let mut last_underscore = false;
    for ch in label.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    out.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axonmind_core::{Confidence, Edge, EdgeId, EdgeKind, EvidenceId, ExtractorKind, NodeId};

    fn node(id: &str, kind: NodeKind, attrs: Value) -> Node {
        let now = chrono::Utc::now();
        Node {
            id: NodeId(id.to_string()),
            kind,
            name: id.to_string(),
            created_at: now,
            updated_at: now,
            attrs,
            confidence: Confidence::RULE,
            is_tainted: false,
            requires_human_review: false,
        }
    }

    fn edge(id: &str, from: &str, to: &str, kind: EdgeKind) -> Edge {
        let now = chrono::Utc::now();
        Edge {
            id: EdgeId(id.to_string()),
            from: NodeId(from.to_string()),
            to: NodeId(to.to_string()),
            kind,
            evidence: vec![EvidenceId("ev.test".to_string())],
            confidence: Confidence::RULE,
            created_at: now,
            created_by: ExtractorKind::Rule,
            is_tainted: false,
            requires_human_review: false,
        }
    }

    #[test]
    fn validate_and_compute_context_compiles_parent_child_selector() {
        let cfg = SummaryConfig {
            summary: SummaryDefinition {
                id: "default".into(),
                name: "Default".into(),
                description: None,
                default_period: Some("MTD".into()),
                default_as_of: Some("latest".into()),
                lenses: vec!["claims".into()],
            },
            lenses: vec![
                LensDefinition {
                    id: "claims".into(),
                    label: "Claims".into(),
                    description: None,
                    hidden: None,
                    context: None,
                    selector: json!({"type":"facet","facet":"function","equals":"Claims"}),
                    measure: json!({"type":"count"}),
                    health: Some(json!({"type":"presence"})),
                    children: vec!["open_claims".into()],
                    headline_node_id: None,
                    member_node_ids: vec![],
                },
                LensDefinition {
                    id: "open_claims".into(),
                    label: "Open Claims".into(),
                    description: None,
                    hidden: None,
                    context: Some(LensContext {
                        inherit_selector: Some(true),
                    }),
                    selector: json!({"type":"attribute","field":"status","equals":"open"}),
                    measure: json!({"type":"count"}),
                    health: Some(json!({"type":"presence"})),
                    children: vec![],
                    headline_node_id: None,
                    member_node_ids: vec![],
                },
            ],
        };

        let out = cfg.validate_and_compute_effective_contexts().unwrap();
        let child = out.iter().find(|x| x.lens_id == "open_claims").unwrap();
        let and_len = child
            .effective_selector
            .get("and")
            .and_then(Value::as_array)
            .map(|v| v.len())
            .unwrap_or(0);
        assert_eq!(and_len, 2);
        assert_eq!(child.effective_period, "MTD");
        assert_eq!(child.effective_as_of, "latest");
    }

    #[test]
    fn validate_rejects_missing_lens_reference() {
        let cfg = SummaryConfig {
            summary: SummaryDefinition {
                id: "default".into(),
                name: "Default".into(),
                description: None,
                default_period: None,
                default_as_of: None,
                lenses: vec!["missing".into()],
            },
            lenses: vec![],
        };
        let err = cfg.validate_and_compute_effective_contexts().unwrap_err();
        assert!(format!("{err}").contains("references missing lens"));
    }

    #[test]
    fn selector_filter_supports_kind_and_attribute_and_saved_query() {
        let nodes = vec![
            node(
                "kpi.loss_ratio",
                NodeKind::Kpi,
                json!({"status":"open","amount":12.3}),
            ),
            node(
                "doc.a",
                NodeKind::Document,
                json!({"status":"open","amount":5}),
            ),
            node(
                "risk.1",
                NodeKind::Risk,
                json!({"status":"closed","amount":3}),
            ),
        ];

        let by_kind = filter_node_ids_for_selector(&nodes, &json!({"type":"kind","kind":"Kpi"}));
        assert_eq!(by_kind, vec!["kpi.loss_ratio".to_string()]);

        let by_attr = filter_node_ids_for_selector(
            &nodes,
            &json!({"type":"attribute","field":"status","equals":"open"}),
        );
        assert_eq!(by_attr.len(), 2);

        let all_concepts = filter_node_ids_for_selector(
            &nodes,
            &json!({"type":"saved_query","query":"all_concepts"}),
        );
        assert_eq!(all_concepts.len(), 2);
        assert!(!all_concepts.contains(&"doc.a".to_string()));
    }

    #[test]
    fn selector_filter_supports_facet_edges_and_attribute_fallback() {
        let nodes = vec![
            node(
                "kpi.cycle_time",
                NodeKind::Kpi,
                json!({"function":"Claims"}),
            ),
            node("kpi.quality", NodeKind::Kpi, json!({})),
            node(
                "function.claims",
                NodeKind::Function,
                json!({"label":"Claims"}),
            ),
        ];
        let edges = vec![edge(
            "e1",
            "kpi.quality",
            "function.claims",
            EdgeKind::InFunction,
        )];

        let facet_hits = filter_node_ids_for_selector_with_edges(
            &nodes,
            &edges,
            &json!({"type":"facet","facet":"function","equals":"Claims"}),
        );
        assert!(facet_hits.contains(&"kpi.cycle_time".to_string())); // attribute fallback
        assert!(facet_hits.contains(&"kpi.quality".to_string())); // edge-backed facet
    }

    #[test]
    fn apply_summary_edit_supports_rename_reorder_hide_and_period() {
        let mut cfg = SummaryConfig {
            summary: SummaryDefinition {
                id: "default".into(),
                name: "Default".into(),
                description: None,
                default_period: Some("latest".into()),
                default_as_of: Some("latest".into()),
                lenses: vec!["claims".into(), "open_claims".into()],
            },
            lenses: vec![
                LensDefinition {
                    id: "claims".into(),
                    label: "Claims".into(),
                    description: None,
                    hidden: None,
                    context: None,
                    selector: json!({"type":"saved_query","query":"all_concepts"}),
                    measure: json!({"type":"count","period":"latest","as_of":"latest"}),
                    health: Some(json!({"type":"presence"})),
                    children: vec![],
                    headline_node_id: None,
                    member_node_ids: vec![],
                },
                LensDefinition {
                    id: "open_claims".into(),
                    label: "Open Claims".into(),
                    description: None,
                    hidden: None,
                    context: None,
                    selector: json!({"type":"saved_query","query":"all_concepts"}),
                    measure: json!({"type":"count","period":"latest","as_of":"latest"}),
                    health: Some(json!({"type":"presence"})),
                    children: vec![],
                    headline_node_id: None,
                    member_node_ids: vec![],
                },
            ],
        };

        apply_summary_config_edit(
            &mut cfg,
            SummaryConfigEdit {
                summary_name: Some("Ops View".into()),
                default_period: Some("MTD".into()),
                default_as_of: Some("2026-05-29".into()),
                lens_order: Some(vec!["open_claims".into(), "claims".into()]),
                lenses: vec![
                    LensEdit {
                        lens_id: "claims".into(),
                        label: Some("Claims Intake".into()),
                        hidden: Some(true),
                        health: Some(json!({"type":"threshold","green_lt":10.0})),
                        measure_period: Some("YTD".into()),
                        measure_as_of: Some("latest".into()),
                    },
                    LensEdit {
                        lens_id: "open_claims".into(),
                        ..Default::default()
                    },
                ],
            },
        )
        .unwrap();

        assert_eq!(cfg.summary.name, "Ops View");
        assert_eq!(cfg.summary.default_period.as_deref(), Some("MTD"));
        assert_eq!(cfg.summary.default_as_of.as_deref(), Some("2026-05-29"));
        assert_eq!(
            cfg.summary.lenses,
            vec!["open_claims".to_string(), "claims".to_string()]
        );

        let claims = cfg.lenses.iter().find(|l| l.id == "claims").unwrap();
        assert_eq!(claims.label, "Claims Intake");
        assert_eq!(claims.hidden, Some(true));
        assert_eq!(
            claims.measure.get("period").and_then(Value::as_str),
            Some("YTD")
        );
    }

    #[test]
    fn to_suggested_skips_hidden_top_level_lenses() {
        let cfg = SummaryConfig {
            summary: SummaryDefinition {
                id: "default".into(),
                name: "Default".into(),
                description: None,
                default_period: Some("latest".into()),
                default_as_of: Some("latest".into()),
                lenses: vec!["visible".into(), "hidden".into()],
            },
            lenses: vec![
                LensDefinition {
                    id: "visible".into(),
                    label: "Visible".into(),
                    description: None,
                    hidden: Some(false),
                    context: None,
                    selector: json!({"type":"saved_query","query":"all_concepts"}),
                    measure: json!({"type":"count"}),
                    health: Some(json!({"type":"presence"})),
                    children: vec![],
                    headline_node_id: Some("kpi.a".into()),
                    member_node_ids: vec!["kpi.a".into()],
                },
                LensDefinition {
                    id: "hidden".into(),
                    label: "Hidden".into(),
                    description: None,
                    hidden: Some(true),
                    context: None,
                    selector: json!({"type":"saved_query","query":"all_concepts"}),
                    measure: json!({"type":"count"}),
                    health: Some(json!({"type":"presence"})),
                    children: vec![],
                    headline_node_id: Some("kpi.b".into()),
                    member_node_ids: vec!["kpi.b".into()],
                },
            ],
        };
        let out = cfg.to_suggested_summary(&[], "config".into());
        assert_eq!(out.categories.len(), 1);
        assert_eq!(out.categories[0].label, "Visible");
    }
}
