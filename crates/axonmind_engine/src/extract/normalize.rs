//! Phase 3: normalize raw LLM output into canonical engine enums.
//!
//! LLMs emit near-miss kind names (`"drives"`, `"depends_on"`, `"DRIVES "`) instead of the
//! exact PascalCase enum variants. Bare serde deserialization drops these silently, losing
//! real relations. These helpers canonicalize first, then fall back to a hand-maintained
//! alias map.
//!
//! Two hard rules:
//! - Canonical variant names always parse (this is a strict superset of bare serde).
//! - We never map a string whose meaning inverts edge direction (e.g. `"owns"` is the inverse
//!   of `OwnedBy`). Returning `None` there forces the caller to drop the edge rather than
//!   assert a relation with `from`/`to` flipped.

use axonmind_core::{Confidence, EdgeKind, NodeKind};

/// Canonicalize an LLM edge-kind string. `None` → no safe mapping; caller drops the edge.
/// We never fabricate a default kind: that would assert an unevidenced relation.
pub fn normalize_edge_kind(raw: &str) -> Option<EdgeKind> {
    if let Some(k) = parse_canonical::<EdgeKind>(raw) {
        return Some(k);
    }
    match canon_key(raw).as_str() {
        "influences" | "influence" | "drives" | "drive" => Some(EdgeKind::Influences),
        "impacts" | "impact" | "affects" | "affect" => Some(EdgeKind::Impacts),
        "causes" | "cause" => Some(EdgeKind::Causes),
        "correlateswith" | "correlates" | "correlate" | "correlated" => {
            Some(EdgeKind::CorrelatesWith)
        }
        "dependson" | "depends" | "depend" | "requires" | "require" | "needs" | "uses" | "use" => {
            Some(EdgeKind::DependsOn)
        }
        "derivedfrom" | "derived" | "derivesfrom" | "derivation" | "aggregates" | "aggregate" => {
            Some(EdgeKind::DerivedFrom)
        }
        "blocks" | "block" | "limits" | "limit" | "prevents" | "prevent" => Some(EdgeKind::Blocks),
        "improves" | "improve" | "increases" | "increase" | "boosts" | "boost" => {
            Some(EdgeKind::Improves)
        }
        "degrades" | "degrade" | "decreases" | "decrease" | "reduces" | "reduce" | "hurts"
        | "hurt" => Some(EdgeKind::Degrades),
        "ownedby" | "owner" => Some(EdgeKind::OwnedBy),
        "measuredby" | "measures" | "measure" => Some(EdgeKind::MeasuredBy),
        "evidencedby" | "evidence" => Some(EdgeKind::EvidencedBy),
        "mentionedin" | "mentions" | "mention" => Some(EdgeKind::MentionedIn),
        "decidedby" | "decides" | "decide" => Some(EdgeKind::DecidedBy),
        "assignedto" | "assignee" => Some(EdgeKind::AssignedTo),
        "infunction" | "function" | "belongsfunction" => Some(EdgeKind::InFunction),
        "forproduct" | "product" | "lineofbusiness" | "lob" => Some(EdgeKind::ForProduct),
        "nextaction" | "action" => Some(EdgeKind::NextAction),
        "contradicts" | "contradict" | "conflictswith" | "disagreeswith" => {
            Some(EdgeKind::Contradicts)
        }
        "corroborates" | "corroborate" | "supports" | "support" | "confirms" | "confirm" => {
            Some(EdgeKind::Corroborates)
        }
        // Inverse-direction strings are intentionally unmapped — mapping them would flip
        // the edge's from/to. Caller must drop these.
        _ => None,
    }
}

/// Canonicalize an LLM node-kind string. `None` → caller skips the entity.
pub fn normalize_node_kind(raw: &str) -> Option<NodeKind> {
    if let Some(k) = parse_canonical::<NodeKind>(raw) {
        return Some(k);
    }
    match canon_key(raw).as_str() {
        "kpi" => Some(NodeKind::Kpi),
        "metric" | "measure" => Some(NodeKind::Metric),
        "objective" | "goal" | "okr" => Some(NodeKind::Objective),
        "initiative" | "project" | "program" => Some(NodeKind::Initiative),
        "risk" | "threat" => Some(NodeKind::Risk),
        "opportunity" => Some(NodeKind::Opportunity),
        "decision" => Some(NodeKind::Decision),
        "insight" | "finding" | "learning" => Some(NodeKind::Insight),
        "document" | "doc" => Some(NodeKind::Document),
        "person" | "actor" | "individual" | "employee" => Some(NodeKind::Person),
        "team" | "group" | "department" | "squad" => Some(NodeKind::Team),
        "customer" | "client" | "account" | "prospect" | "lead" => Some(NodeKind::Customer),
        "function" | "businessfunction" => Some(NodeKind::Function),
        "product" | "feature" => Some(NodeKind::Product),
        "market" | "segment" | "industry" => Some(NodeKind::Market),
        "process" | "workflow" | "procedure" => Some(NodeKind::Process),
        "system" | "platform" | "tool" | "application" => Some(NodeKind::System),
        "action" | "task" | "todo" => Some(NodeKind::Action),
        _ => None,
    }
}

/// True if two concept names are near-duplicates worth bridging across documents: they share
/// at least two significant tokens and the smaller token set is fully contained in the larger.
///
/// High-precision by design. It links `"Risk Evaluation"` with `"AI-Assisted Risk Evaluation"`,
/// but NOT `"Platform Warranties"` with `"Broker Warranties"` (which share only `warranties`),
/// and NOT single-token overlaps like `"Compliance"` ⊂ `"Regulatory Compliance"` (the ≥2-token
/// floor avoids over-linking on one common word). Identical token sets return `false`: those are
/// the same concept and already merge by slug, so there is nothing to bridge.
pub fn names_near_match(a: &str, b: &str) -> bool {
    let ta = significant_tokens(a);
    let tb = significant_tokens(b);
    if ta.is_empty() || tb.is_empty() || ta == tb {
        return false;
    }
    let (small, large) = if ta.len() <= tb.len() {
        (&ta, &tb)
    } else {
        (&tb, &ta)
    };
    small.len() >= 2 && small.is_subset(large)
}

/// Lowercase, split on non-alphanumeric, and drop stopwords and tokens shorter than 3 chars,
/// leaving the meaning-bearing tokens used for near-duplicate matching.
fn significant_tokens(name: &str) -> std::collections::HashSet<String> {
    const STOP: &[&str] = &[
        "the", "and", "for", "with", "that", "this", "from", "into", "are", "was", "its", "our",
    ];
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 3 && !STOP.contains(t))
        .map(|t| t.to_string())
        .collect()
}

/// For an entity the extractor resolved to `NodeKind::Customer`, classify the lifecycle
/// stage implied by the raw LLM kind string. `prospect`/`lead` are unconverted; `customer`/
/// `client`/`account` are an active (converted) relationship. Returns `None` for kind strings
/// that carry no stage signal, so non-Customer nodes get no lifecycle attribute.
///
/// Both stages share `NodeKind::Customer` because they are the same entity at different points
/// in its lifecycle, not different kinds — the distinction is stored as a node attribute.
pub fn customer_lifecycle(raw: &str) -> Option<&'static str> {
    match canon_key(raw).as_str() {
        "prospect" | "lead" => Some("prospect"),
        "customer" | "client" | "account" => Some("active"),
        _ => None,
    }
}

/// Strip a leading clause/section number from an extracted entity name so document
/// headings collapse to their underlying concept:
/// `"22.3 Platform Warranties"` → `"Platform Warranties"`, `"8. Data Retention"` → `"Data Retention"`.
///
/// Only strips when the leading number contains a dot (an internal `22.3` or a trailing `8.`)
/// and is followed by whitespace. This deliberately leaves bare numbers alone (`"2024 revenue plan"`,
/// `"3D Secure"`) so we don't merge genuinely distinct concepts that merely start with a digit.
/// Returns the original string if nothing is stripped or if stripping would empty the name.
pub fn strip_leading_clause_number(name: &str) -> &str {
    let trimmed = name.trim_start();
    let bytes = trimmed.as_bytes();
    let mut i = 0;
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
    if !saw_digit || !saw_dot {
        return name;
    }
    let rest = &trimmed[i..];
    // Require a whitespace separator after the number; "3.5kg" is not a clause prefix.
    if !rest.starts_with(char::is_whitespace) {
        return name;
    }
    let cleaned = rest.trim_start();
    if cleaned.is_empty() { name } else { cleaned }
}

/// Clamp an LLM-supplied confidence to `[0,1]`. NaN/inf → `Confidence::LLM` (0.50).
pub fn sanitize_confidence(raw: f32) -> Confidence {
    if raw.is_finite() {
        Confidence(raw.clamp(0.0, 1.0))
    } else {
        Confidence::LLM
    }
}

/// Try to deserialize the exact canonical variant name (e.g. `"Influences"`).
fn parse_canonical<T: for<'de> serde::Deserialize<'de>>(raw: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(raw.trim().to_string())).ok()
}

/// Lowercase, trim, and strip separators so `"depends_on"`, `"Depends On"`, and
/// `"depends-on"` all collapse to `"dependson"`.
fn canon_key(raw: &str) -> String {
    raw.trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_verb_synonyms_case_and_space_insensitively() {
        assert_eq!(normalize_edge_kind("drives"), Some(EdgeKind::Influences));
        assert_eq!(normalize_edge_kind("DRIVES "), Some(EdgeKind::Influences));
        assert_eq!(normalize_edge_kind("depends_on"), Some(EdgeKind::DependsOn));
        assert_eq!(normalize_edge_kind("impacts"), Some(EdgeKind::Impacts));
        assert_eq!(
            normalize_edge_kind("line_of_business"),
            Some(EdgeKind::ForProduct)
        );
        assert_eq!(normalize_edge_kind("function"), Some(EdgeKind::InFunction));
    }

    #[test]
    fn impacts_and_influences_stay_distinct() {
        // The plan example conflated these; they are separate variants and must not merge.
        assert_eq!(normalize_edge_kind("impact"), Some(EdgeKind::Impacts));
        assert_eq!(normalize_edge_kind("influence"), Some(EdgeKind::Influences));
    }

    #[test]
    fn refuses_direction_inverting_strings() {
        // The property that matters: we never silently flip an edge. "owns" is the inverse of
        // OwnedBy; mapping it would put from/to backwards, so we drop instead.
        assert_eq!(normalize_edge_kind("owns"), None);
        assert_eq!(normalize_edge_kind("blocked_by"), None);
        assert_eq!(normalize_edge_kind("influenced_by"), None);
        assert_eq!(normalize_edge_kind("caused_by"), None);
    }

    #[test]
    fn unknown_kinds_return_none_not_a_default() {
        assert_eq!(normalize_edge_kind("frobnicates"), None);
        assert_eq!(normalize_node_kind("widget"), None);
    }

    #[test]
    fn every_canonical_edge_kind_round_trips() {
        // Superset guarantee: canonical names must always parse to themselves.
        for name in [
            "Influences",
            "Causes",
            "CorrelatesWith",
            "DependsOn",
            "DerivedFrom",
            "Blocks",
            "Improves",
            "Degrades",
            "OwnedBy",
            "MeasuredBy",
            "EvidencedBy",
            "MentionedIn",
            "DecidedBy",
            "AssignedTo",
            "InFunction",
            "ForProduct",
            "Impacts",
            "NextAction",
            "Contradicts",
            "Corroborates",
        ] {
            assert!(
                normalize_edge_kind(name).is_some(),
                "canonical edge kind {name} failed to parse"
            );
        }
    }

    #[test]
    fn every_canonical_node_kind_round_trips() {
        for name in [
            "Kpi",
            "Metric",
            "Objective",
            "Initiative",
            "Risk",
            "Opportunity",
            "Decision",
            "Insight",
            "Document",
            "Person",
            "Team",
            "Customer",
            "Function",
            "Product",
            "Market",
            "Process",
            "System",
            "Action",
        ] {
            assert!(
                normalize_node_kind(name).is_some(),
                "canonical node kind {name} failed to parse"
            );
        }
    }

    #[test]
    fn node_synonyms_map() {
        assert_eq!(normalize_node_kind("goal"), Some(NodeKind::Objective));
        assert_eq!(normalize_node_kind("project"), Some(NodeKind::Initiative));
        assert_eq!(normalize_node_kind("client"), Some(NodeKind::Customer));
        assert_eq!(normalize_node_kind("function"), Some(NodeKind::Function));
        // A prospect/lead is a prospective customer — alias to Customer, not a new kind.
        assert_eq!(normalize_node_kind("prospect"), Some(NodeKind::Customer));
        assert_eq!(normalize_node_kind("lead"), Some(NodeKind::Customer));
    }

    #[test]
    fn near_match_bridges_synonyms_but_not_siblings() {
        // The bridge's whole purpose: connect a concept the exact-slug merge missed...
        assert!(names_near_match(
            "Risk Evaluation",
            "AI-Assisted Risk Evaluation"
        ));
        assert!(names_near_match(
            "AI-Assisted Risk Evaluation",
            "Risk Evaluation"
        )); // order-independent
        // ...without fusing genuinely different concepts that merely share one word.
        assert!(!names_near_match(
            "Platform Warranties",
            "Broker Warranties"
        ));
        // Single shared token is below the ≥2 floor → not a bridge (avoids "Risk" linking everything).
        assert!(!names_near_match("Compliance", "Regulatory Compliance"));
        // Identical concept → already merges by slug, nothing to bridge.
        assert!(!names_near_match("Data Retention", "Data Retention"));
    }

    #[test]
    fn customer_lifecycle_distinguishes_prospect_from_active() {
        // Same kind, different stage: the distinction we preserve as an attribute.
        assert_eq!(customer_lifecycle("prospect"), Some("prospect"));
        assert_eq!(customer_lifecycle("lead"), Some("prospect"));
        assert_eq!(customer_lifecycle("customer"), Some("active"));
        assert_eq!(customer_lifecycle("Client"), Some("active"));
        assert_eq!(customer_lifecycle("account"), Some("active"));
        // No stage signal for non-customer kinds → no lifecycle attribute is set.
        assert_eq!(customer_lifecycle("product"), None);
        assert_eq!(customer_lifecycle("risk"), None);
    }

    #[test]
    fn strips_dotted_clause_numbers_to_concept() {
        // The point: numbered headings must collapse to the bare concept so duplicates merge.
        assert_eq!(
            strip_leading_clause_number("22.3 Platform Warranties"),
            "Platform Warranties"
        );
        assert_eq!(
            strip_leading_clause_number("8. Data Retention"),
            "Data Retention"
        );
        // Two different section numbers for the same concept must yield the identical string,
        // which is what lets the upsert merge them into one node.
        assert_eq!(
            strip_leading_clause_number("14.7 Data Retention"),
            strip_leading_clause_number("8. Data Retention")
        );
    }

    #[test]
    fn leaves_bare_numbers_and_non_clause_names_untouched() {
        // No dot → not treated as a clause prefix, so distinct concepts stay distinct.
        assert_eq!(
            strip_leading_clause_number("2024 revenue plan"),
            "2024 revenue plan"
        );
        // Dot but no whitespace separator → a measurement/version, not a clause number.
        assert_eq!(
            strip_leading_clause_number("3.5kg payload"),
            "3.5kg payload"
        );
        // No leading number at all.
        assert_eq!(
            strip_leading_clause_number("Broker Control"),
            "Broker Control"
        );
        // Number-only after stripping would empty the name → keep original.
        assert_eq!(strip_leading_clause_number("22.3 "), "22.3 ");
    }

    #[test]
    fn sanitize_confidence_clamps_and_handles_non_finite() {
        assert_eq!(sanitize_confidence(1.4).value(), 1.0);
        assert_eq!(sanitize_confidence(-0.2).value(), 0.0);
        assert_eq!(sanitize_confidence(0.73).value(), 0.73);
        assert_eq!(
            sanitize_confidence(f32::NAN).value(),
            Confidence::LLM.value()
        );
        assert_eq!(
            sanitize_confidence(f32::INFINITY).value(),
            Confidence::LLM.value()
        );
    }
}
