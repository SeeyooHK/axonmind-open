//! Phase 1 (brain map): organize the existing graph into ≤10 categories for the first-glance
//! radial summary.
//!
//! LLM-suggested when a provider is configured (the system prompt is assembled from
//! [`super::prompts`], so it is tunable in one place); otherwise a deterministic group-by-kind
//! fallback so the view always renders. This only *groups and labels* nodes that already exist —
//! it never invents facts, and it does not decide values or health (those enrich the view later,
//! per framework spec §10).

use std::collections::{HashMap, HashSet};

use axonmind_core::{AxonMindError, Edge, Node, NodeKind};
use serde::{Deserialize, Serialize};

use super::llm::LlmProvider;
use super::prompts::PromptLibrary;

/// Hard cap on categories — a first glance must stay glanceable (framework spec §10).
const MAX_CATEGORIES: usize = 10;

/// Fragment keys assembled (in order) into the categorization system prompt.
const CATEGORIZE_PROMPT: [&str; 4] = [
    "categorize.system",
    "categorize.rules",
    "categorize.optimization",
    "categorize.output",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuggestedCategory {
    pub label: String,
    pub headline_node_id: String,
    pub member_node_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SuggestedSummary {
    pub categories: Vec<SuggestedCategory>,
    /// How the grouping was produced: `"llm"`, `"fallback"`, or `"empty"` — so an operator can
    /// tell whether the LLM ran.
    pub source: String,
    /// node id → display name for every id referenced by the categories. Lets any renderer show
    /// real labels without holding the full graph.
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Suggest a summary over the non-`Document` graph. Uses the LLM when `provider` is `Some`,
/// otherwise the deterministic kind-based fallback. Errors only on an LLM call/parse failure.
pub async fn suggest_summary(
    provider: Option<&dyn LlmProvider>,
    library: &PromptLibrary,
    nodes: &[Node],
    edges: &[Edge],
) -> Result<SuggestedSummary, AxonMindError> {
    let concepts: Vec<&Node> = nodes
        .iter()
        .filter(|n| n.kind != NodeKind::Document)
        .collect();
    if concepts.is_empty() {
        return Ok(SuggestedSummary {
            categories: vec![],
            source: "empty".into(),
            labels: HashMap::new(),
        });
    }

    let mut summary = match provider {
        Some(llm) => {
            let system = library.assemble(&CATEGORIZE_PROMPT);
            let user = build_user_message(&concepts, edges);
            let raw = llm.complete(&system, &user).await?;
            let valid: HashSet<&str> = concepts.iter().map(|n| n.id.0.as_str()).collect();
            let parsed = parse_summary(&raw, &valid)?;
            optimize_llm_summary(parsed, &concepts, edges)
        }
        None => fallback_by_kind(&concepts, edges),
    };
    summary.labels = referenced_labels(&concepts, &summary);
    Ok(summary)
}

/// node id → name for every id referenced by the categories (headline + members), so a renderer
/// can show real labels without the full graph.
fn referenced_labels(concepts: &[&Node], summary: &SuggestedSummary) -> HashMap<String, String> {
    let names: HashMap<&str, &str> = concepts
        .iter()
        .map(|n| (n.id.0.as_str(), n.name.as_str()))
        .collect();
    let mut out = HashMap::new();
    for c in &summary.categories {
        for id in std::iter::once(&c.headline_node_id).chain(c.member_node_ids.iter()) {
            if let Some(name) = names.get(id.as_str()) {
                out.insert(id.clone(), (*name).to_string());
            }
        }
    }
    out
}

/// Render the graph as a compact node + edge listing for the LLM to group over.
fn build_user_message(concepts: &[&Node], edges: &[Edge]) -> String {
    let mut s = String::from("NODES (id | kind | name):\n");
    for n in concepts {
        s.push_str(&format!("{} | {:?} | {}\n", n.id.0, n.kind, n.name));
    }
    s.push_str("\nEDGES (from_id -> kind -> to_id):\n");
    for e in edges {
        s.push_str(&format!("{} -> {:?} -> {}\n", e.from.0, e.kind, e.to.0));
    }
    s
}

/// Parse the LLM's JSON, dropping any node ids it hallucinated and capping to [`MAX_CATEGORIES`].
/// A category with no surviving members is dropped; an invalid headline falls back to the first
/// surviving member so a circle always has a center.
fn parse_summary(raw: &str, valid_ids: &HashSet<&str>) -> Result<SuggestedSummary, AxonMindError> {
    #[derive(Deserialize)]
    struct Raw {
        categories: Vec<RawCat>,
    }
    #[derive(Deserialize)]
    struct RawCat {
        label: String,
        headline_node_id: String,
        member_node_ids: Vec<String>,
    }

    let parsed: Raw = serde_json::from_str(strip_fences(raw))
        .map_err(|e| AxonMindError::LlmProvider(format!("summary parse: {e}")))?;

    let mut assigned = HashSet::<String>::new();
    let mut categories: Vec<SuggestedCategory> = Vec::new();
    for c in parsed.categories {
        if categories.len() >= MAX_CATEGORIES {
            break;
        }

        // 1) keep only valid ids
        // 2) deduplicate within the category
        // 3) enforce globally unique assignment across categories
        let mut local_seen = HashSet::<String>::new();
        let members: Vec<String> = c
            .member_node_ids
            .into_iter()
            .filter(|id| valid_ids.contains(id.as_str()))
            .filter(|id| local_seen.insert(id.clone()))
            .filter(|id| !assigned.contains(id))
            .collect();
        if members.is_empty() {
            continue;
        }
        for id in &members {
            assigned.insert(id.clone());
        }

        let headline = if members.iter().any(|id| id == &c.headline_node_id) {
            c.headline_node_id
        } else {
            members[0].clone()
        };
        categories.push(SuggestedCategory {
            label: c.label,
            headline_node_id: headline,
            member_node_ids: members,
        });
    }

    // Ensure every valid node appears exactly once, even if the LLM omitted some.
    let mut unassigned: Vec<String> = valid_ids
        .iter()
        .filter(|id| !assigned.contains(**id))
        .map(|id| (*id).to_string())
        .collect();
    unassigned.sort();
    if !unassigned.is_empty() {
        if categories.len() < MAX_CATEGORIES {
            categories.push(SuggestedCategory {
                label: "Other".to_string(),
                headline_node_id: unassigned[0].clone(),
                member_node_ids: unassigned,
            });
        } else if let Some(last) = categories.last_mut() {
            last.member_node_ids.extend(unassigned);
        }
    }

    Ok(SuggestedSummary {
        categories,
        source: "llm".into(),
        labels: HashMap::new(),
    })
}

/// Deterministic post-LLM normalization:
/// - normalize/merge duplicate labels
/// - pick stronger headlines by kind + connectivity
/// - keep at most one tighter "Other" bucket and redistribute tiny leftovers when possible
fn optimize_llm_summary(
    summary: SuggestedSummary,
    concepts: &[&Node],
    edges: &[Edge],
) -> SuggestedSummary {
    let degree = node_degrees(edges);
    let node_kind: HashMap<&str, NodeKind> =
        concepts.iter().map(|n| (n.id.0.as_str(), n.kind)).collect();
    let neighbors = neighbor_map(edges);

    let mut merged: Vec<SuggestedCategory> = Vec::new();
    let mut label_index = HashMap::<String, usize>::new();
    for mut cat in summary.categories {
        cat.label = normalized_label(&cat.label);
        let key = canonical_label_key(&cat.label);
        if let Some(idx) = label_index.get(&key).copied() {
            let existing = &mut merged[idx];
            existing.member_node_ids.extend(cat.member_node_ids);
            existing.member_node_ids.sort();
            existing.member_node_ids.dedup();
            existing.headline_node_id = pick_headline(
                &existing.member_node_ids,
                &degree,
                &node_kind,
                Some(&existing.headline_node_id),
            );
        } else {
            label_index.insert(key, merged.len());
            cat.member_node_ids.sort();
            cat.member_node_ids.dedup();
            cat.headline_node_id = pick_headline(
                &cat.member_node_ids,
                &degree,
                &node_kind,
                Some(&cat.headline_node_id),
            );
            merged.push(cat);
        }
    }

    tighten_other_bucket(&mut merged, &neighbors);

    merged.sort_by_key(|c| std::cmp::Reverse(c.member_node_ids.len()));
    if let Some(other_idx) = merged.iter().position(|c| is_other_label(&c.label)) {
        let other = merged.remove(other_idx);
        merged.push(other);
    }
    if merged.len() > MAX_CATEGORIES {
        merged.truncate(MAX_CATEGORIES);
    }

    SuggestedSummary {
        categories: merged,
        source: summary.source,
        labels: summary.labels,
    }
}

/// Deterministic fallback: one category per node kind, biggest groups first (capped at 10), with
/// the highest-degree node in each group as its headline.
fn fallback_by_kind(concepts: &[&Node], edges: &[Edge]) -> SuggestedSummary {
    let mut degree: HashMap<&str, usize> = HashMap::new();
    for e in edges {
        *degree.entry(e.from.0.as_str()).or_default() += 1;
        *degree.entry(e.to.0.as_str()).or_default() += 1;
    }

    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<&Node>> = HashMap::new();
    for &n in concepts {
        let k = format!("{:?}", n.kind);
        if !groups.contains_key(&k) {
            order.push(k.clone());
        }
        groups.entry(k).or_default().push(n);
    }

    order.sort_by_key(|k| std::cmp::Reverse(groups[k].len()));

    let categories = order
        .into_iter()
        .take(MAX_CATEGORIES)
        .map(|k| {
            let members = &groups[&k];
            let headline = members
                .iter()
                .max_by_key(|n| degree.get(n.id.0.as_str()).copied().unwrap_or(0))
                .expect("kind group is never empty");
            SuggestedCategory {
                label: k.clone(),
                headline_node_id: headline.id.0.clone(),
                member_node_ids: members.iter().map(|n| n.id.0.clone()).collect(),
            }
        })
        .collect();

    SuggestedSummary {
        categories,
        source: "fallback".into(),
        labels: HashMap::new(),
    }
}

/// Strip a leading/trailing markdown code fence if a provider wrapped its JSON.
fn strip_fences(text: &str) -> &str {
    let t = text.trim();
    t.strip_prefix("```json")
        .or_else(|| t.strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(t)
}

fn normalized_label(raw: &str) -> String {
    let mut s = raw.trim().to_string();
    // Strip clause prefixes like "22.3 Warranties" or "8. Data".
    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut saw_digit = false;
    let mut saw_dot = false;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        if bytes[i].is_ascii_digit() {
            saw_digit = true;
        } else {
            saw_dot = true;
        }
        i += 1;
    }
    if saw_digit && saw_dot {
        let rest = s[i..].trim_start();
        if !rest.is_empty() {
            s = rest.to_string();
        }
    }
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        "Other".to_string()
    } else {
        collapsed
    }
}

fn canonical_label_key(label: &str) -> String {
    label
        .to_ascii_lowercase()
        .replace('&', " and ")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c.is_ascii_whitespace() {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_other_label(label: &str) -> bool {
    matches!(
        canonical_label_key(label).as_str(),
        "other" | "others" | "misc" | "miscellaneous" | "general"
    )
}

fn node_degrees(edges: &[Edge]) -> HashMap<&str, usize> {
    let mut out = HashMap::<&str, usize>::new();
    for e in edges {
        *out.entry(e.from.0.as_str()).or_default() += 1;
        *out.entry(e.to.0.as_str()).or_default() += 1;
    }
    out
}

fn pick_headline(
    members: &[String],
    degree: &HashMap<&str, usize>,
    node_kind: &HashMap<&str, NodeKind>,
    current: Option<&str>,
) -> String {
    if members.is_empty() {
        return current.unwrap_or_default().to_string();
    }
    let mut best = members[0].as_str();
    let mut best_score = headline_score(best, degree, node_kind);
    for id in members.iter().skip(1) {
        let score = headline_score(id, degree, node_kind);
        if score > best_score {
            best = id;
            best_score = score;
        }
    }
    if let Some(cur) = current {
        if members.iter().any(|id| id == cur)
            && headline_score(cur, degree, node_kind) >= best_score
        {
            return cur.to_string();
        }
    }
    best.to_string()
}

fn headline_score(
    node_id: &str,
    degree: &HashMap<&str, usize>,
    node_kind: &HashMap<&str, NodeKind>,
) -> (u8, usize) {
    let kind_rank = match node_kind.get(node_id).copied() {
        Some(NodeKind::Kpi) => 5,
        Some(NodeKind::Metric) => 4,
        Some(NodeKind::Objective) => 3,
        Some(NodeKind::Initiative) => 2,
        Some(NodeKind::Decision) => 1,
        _ => 0,
    };
    (kind_rank, degree.get(node_id).copied().unwrap_or(0))
}

fn neighbor_map(edges: &[Edge]) -> HashMap<String, HashSet<String>> {
    let mut out = HashMap::<String, HashSet<String>>::new();
    for e in edges {
        out.entry(e.from.0.clone())
            .or_default()
            .insert(e.to.0.clone());
        out.entry(e.to.0.clone())
            .or_default()
            .insert(e.from.0.clone());
    }
    out
}

fn tighten_other_bucket(
    categories: &mut Vec<SuggestedCategory>,
    neighbors: &HashMap<String, HashSet<String>>,
) {
    let Some(other_idx) = categories.iter().position(|c| is_other_label(&c.label)) else {
        return;
    };
    if categories.len() <= 1 {
        categories[other_idx].label = "Other".to_string();
        return;
    }
    let other_size = categories[other_idx].member_node_ids.len();
    if other_size == 0 {
        categories.remove(other_idx);
        return;
    }
    let total: usize = categories.iter().map(|c| c.member_node_ids.len()).sum();
    let should_redistribute = other_size <= 2 || (other_size * 100 / total.max(1)) < 12;
    if !should_redistribute {
        categories[other_idx].label = "Other".to_string();
        return;
    }

    let mut moved_any = false;
    let mut remain = Vec::<String>::new();
    let members = categories[other_idx].member_node_ids.clone();
    for node_id in members {
        let mut best_idx = None;
        let mut best_score = 0usize;
        for (idx, cat) in categories.iter().enumerate() {
            if idx == other_idx {
                continue;
            }
            let score = connectivity_score(&node_id, &cat.member_node_ids, neighbors);
            if score > best_score {
                best_score = score;
                best_idx = Some(idx);
            }
        }
        if let Some(idx) = best_idx {
            if best_score > 0 {
                categories[idx].member_node_ids.push(node_id);
                moved_any = true;
            } else {
                remain.push(node_id);
            }
        } else {
            remain.push(node_id);
        }
    }

    if moved_any {
        for cat in categories.iter_mut() {
            cat.member_node_ids.sort();
            cat.member_node_ids.dedup();
        }
    }
    if remain.is_empty() {
        categories.remove(other_idx);
    } else {
        categories[other_idx].label = "Other".to_string();
        categories[other_idx].member_node_ids = remain;
        categories[other_idx].headline_node_id = categories[other_idx]
            .member_node_ids
            .first()
            .cloned()
            .unwrap_or_else(|| categories[other_idx].headline_node_id.clone());
    }
}

fn connectivity_score(
    node_id: &str,
    target_members: &[String],
    neighbors: &HashMap<String, HashSet<String>>,
) -> usize {
    let Some(nn) = neighbors.get(node_id) else {
        return 0;
    };
    target_members
        .iter()
        .filter(|member| nn.contains(member.as_str()))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axonmind_core::{Confidence, EdgeId, EdgeKind, ExtractorKind, NodeId};
    use chrono::Utc;

    fn node(id: &str, kind: NodeKind) -> Node {
        let now = Utc::now();
        Node {
            id: NodeId(id.into()),
            kind,
            name: id.into(),
            attrs: serde_json::Value::Null,
            confidence: Confidence::RULE,
            created_at: now,
            updated_at: now,
            is_tainted: false,
            requires_human_review: false,
        }
    }

    fn edge(from: &str, to: &str) -> Edge {
        let now = Utc::now();
        Edge {
            id: EdgeId(format!("{from}-{to}")),
            from: NodeId(from.into()),
            to: NodeId(to.into()),
            kind: EdgeKind::DependsOn,
            evidence: vec![],
            confidence: Confidence::RULE,
            created_at: now,
            created_by: ExtractorKind::Rule,
            is_tainted: false,
            requires_human_review: false,
        }
    }

    #[test]
    fn fallback_groups_by_kind_biggest_first_with_highest_degree_headline() {
        // Intent: with no LLM, the view still renders meaningful circles — one per kind, the most
        // connected node fronting each, and the largest group first.
        let nodes = vec![
            node("p1", NodeKind::Process),
            node("p2", NodeKind::Process),
            node("k1", NodeKind::Kpi),
        ];
        let refs: Vec<&Node> = nodes.iter().collect();
        // p2 has degree 2, p1 degree 1 -> p2 must be the Process headline.
        let edges = vec![edge("p2", "k1"), edge("p2", "p1")];

        let out = fallback_by_kind(&refs, &edges);
        assert_eq!(out.source, "fallback");
        assert_eq!(out.categories[0].label, "Process", "largest group first");
        assert_eq!(
            out.categories[0].headline_node_id, "p2",
            "highest-degree node fronts the group"
        );
        assert_eq!(out.categories.len(), 2);
    }

    #[test]
    fn parse_drops_hallucinated_ids_and_recenters_bad_headline() {
        // Intent: the LLM cannot smuggle node ids that aren't in the graph, and a bogus headline
        // is repaired rather than rendering a dangling circle.
        let valid: HashSet<&str> = ["a", "b"].into_iter().collect();
        let raw = r#"{"categories":[
            {"label":"X","headline_node_id":"ghost","member_node_ids":["a","ghost","b"]}
        ]}"#;
        let out = parse_summary(raw, &valid).unwrap();
        assert_eq!(
            out.categories[0].member_node_ids,
            vec!["a", "b"],
            "hallucinated id dropped"
        );
        assert_eq!(
            out.categories[0].headline_node_id, "a",
            "bad headline recentered on first member"
        );
    }

    #[test]
    fn parse_caps_at_ten_categories_and_strips_fences() {
        let valid: HashSet<&str> = [
            "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "a8", "a9", "a10", "a11", "a12", "a13",
            "a14",
        ]
        .into_iter()
        .collect();
        let cats: Vec<String> = (0..15)
            .map(|i| {
                format!(
                    r#"{{"label":"C{i}","headline_node_id":"a{i}","member_node_ids":["a{i}"]}}"#
                )
            })
            .collect();
        let raw = format!("```json\n{{\"categories\":[{}]}}\n```", cats.join(","));
        let out = parse_summary(&raw, &valid).unwrap();
        assert_eq!(
            out.categories.len(),
            MAX_CATEGORIES,
            "capped at 10 even though 15 were returned"
        );
    }

    #[test]
    fn parse_enforces_unique_node_membership_across_categories() {
        let valid: HashSet<&str> = ["a", "b", "c"].into_iter().collect();
        let raw = r#"{"categories":[
            {"label":"X","headline_node_id":"a","member_node_ids":["a","b"]},
            {"label":"Y","headline_node_id":"b","member_node_ids":["b","c"]}
        ]}"#;
        let out = parse_summary(raw, &valid).unwrap();
        assert_eq!(out.categories.len(), 2);
        assert_eq!(out.categories[0].member_node_ids, vec!["a", "b"]);
        assert_eq!(
            out.categories[1].member_node_ids,
            vec!["c"],
            "node b must not appear twice across categories"
        );
    }

    #[test]
    fn optimize_merges_duplicate_labels_and_prefers_kpi_headline() {
        let nodes = vec![
            node("k1", NodeKind::Kpi),
            node("m1", NodeKind::Metric),
            node("p1", NodeKind::Process),
        ];
        let refs: Vec<&Node> = nodes.iter().collect();
        let edges = vec![edge("k1", "p1"), edge("k1", "m1")];
        let input = SuggestedSummary {
            categories: vec![
                SuggestedCategory {
                    label: "Claims & Servicing".into(),
                    headline_node_id: "p1".into(),
                    member_node_ids: vec!["p1".into()],
                },
                SuggestedCategory {
                    label: "claims and servicing".into(),
                    headline_node_id: "m1".into(),
                    member_node_ids: vec!["k1".into(), "m1".into()],
                },
            ],
            source: "llm".into(),
            labels: HashMap::new(),
        };
        let out = optimize_llm_summary(input, &refs, &edges);
        assert_eq!(out.categories.len(), 1);
        assert_eq!(out.categories[0].member_node_ids.len(), 3);
        assert_eq!(
            out.categories[0].headline_node_id, "k1",
            "KPI should win headline when present and well connected"
        );
    }

    #[test]
    fn optimize_redistributes_tiny_other_bucket_when_connected() {
        let nodes = vec![
            node("a", NodeKind::Kpi),
            node("b", NodeKind::Kpi),
            node("x", NodeKind::Process),
        ];
        let refs: Vec<&Node> = nodes.iter().collect();
        let edges = vec![edge("x", "a")];
        let input = SuggestedSummary {
            categories: vec![
                SuggestedCategory {
                    label: "A".into(),
                    headline_node_id: "a".into(),
                    member_node_ids: vec!["a".into(), "b".into()],
                },
                SuggestedCategory {
                    label: "Misc".into(),
                    headline_node_id: "x".into(),
                    member_node_ids: vec!["x".into()],
                },
            ],
            source: "llm".into(),
            labels: HashMap::new(),
        };
        let out = optimize_llm_summary(input, &refs, &edges);
        assert_eq!(
            out.categories.len(),
            1,
            "tiny connected Other should be absorbed"
        );
        assert!(out.categories[0].member_node_ids.contains(&"x".to_string()));
    }
}
