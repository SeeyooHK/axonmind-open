//! Pure matching logic for package `evals/*.toml` retrieval regression cases
//! (retrieve_guarantee.md item 7 / Enhancement #9). Orchestration (running `document_search`,
//! checking corpus scope) lives on `AxonMindEngine::run_structure_evals` in `lib.rs`, which calls
//! `eval_case_result` here with the hits it already fetched — kept separate so the matching rule
//! itself is testable without a store.

use crate::query::DocumentSearchResult;
use crate::structure::model::EvalCase;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalOutcome {
    Passed,
    Failed,
    /// The eval's corpus currently claims zero documents — not a failure, since a freshly
    /// installed package (docs not ingested yet) must not fail its evals on install.
    Skipped,
}

#[derive(Debug, Clone)]
pub struct EvalCaseResult {
    pub name: String,
    pub outcome: EvalOutcome,
    /// Set on `Failed`/`Skipped`; `None` on `Passed`.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct PackageEvalReport {
    pub package_name: String,
    pub results: Vec<EvalCaseResult>,
}

impl PackageEvalReport {
    pub fn passed(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.outcome == EvalOutcome::Passed)
            .count()
    }

    pub fn failed(&self) -> Vec<&EvalCaseResult> {
        self.results
            .iter()
            .filter(|r| r.outcome == EvalOutcome::Failed)
            .collect()
    }

    pub fn skipped(&self) -> Vec<&EvalCaseResult> {
        self.results
            .iter()
            .filter(|r| r.outcome == EvalOutcome::Skipped)
            .collect()
    }

    pub fn all_pass(&self) -> bool {
        self.results
            .iter()
            .all(|r| r.outcome != EvalOutcome::Failed)
    }
}

/// Does any hit's locator satisfy any one of `eval.expect`'s maps? Every key in an expect map
/// must equal the hit's locator value for that key; a hit's extra locator keys (or a missing
/// key) don't block a match — see `EvalCase::expect`'s own doc comment for why.
pub fn eval_case_result(
    eval: &EvalCase,
    hits: &[DocumentSearchResult],
    expected_doc_ids: Option<&std::collections::HashSet<String>>,
) -> EvalCaseResult {
    let matched = hits.iter().any(|hit| {
        if expected_doc_ids.is_some_and(|ids| !ids.contains(&hit.doc_id)) {
            return false;
        }
        let Some(locator) = hit.locator.as_ref() else {
            return false;
        };
        eval.expect.iter().any(|expect| {
            expect
                .iter()
                .all(|(key, value)| locator.0.get(key) == Some(value))
        })
    });

    if matched {
        EvalCaseResult {
            name: eval.name.clone(),
            outcome: EvalOutcome::Passed,
            reason: None,
        }
    } else {
        let seen: Vec<String> = hits
            .iter()
            .take(5)
            .map(|hit| match &hit.locator {
                Some(locator) => format!("{:?}", locator.0),
                None => format!("<no locator: {}>", hit.title),
            })
            .collect();
        EvalCaseResult {
            name: eval.name.clone(),
            outcome: EvalOutcome::Failed,
            reason: Some(if seen.is_empty() {
                "no hits returned".to_string()
            } else {
                format!("top hits did not match: [{}]", seen.join(", "))
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::{PageLocator, UnitLocator};
    use std::collections::BTreeMap;

    fn hit(locator: Option<BTreeMap<String, String>>) -> DocumentSearchResult {
        DocumentSearchResult {
            doc_id: "doc.1".to_string(),
            section_id: "sec.1".to_string(),
            unit_id: Some("unit.1".to_string()),
            locator: locator.map(UnitLocator),
            page: PageLocator {
                start: None,
                end: None,
            },
            title: "Article 33".to_string(),
            snippet: "...".to_string(),
            score_source: "bm25".to_string(),
            citation_safe: true,
            doc_sha256: "sha".to_string(),
            package_name: "legal-eu-privacy".to_string(),
            package_version: 9,
            status: "unknown".to_string(),
            as_of: None,
            superseded_by: None,
        }
    }

    fn eval(expect: Vec<BTreeMap<String, String>>) -> EvalCase {
        EvalCase {
            name: "art-33".to_string(),
            query: "What does Article 33 require?".to_string(),
            corpus: "gdpr".to_string(),
            top_k: None,
            expect_document: None,
            expect,
        }
    }

    #[test]
    fn passes_when_a_hit_locator_is_a_superset_of_an_expect_map() {
        // A hit's locator ({article, number}) always carries more keys than an eval bothers to
        // assert — the eval only pins the ones it cares about, matching real production
        // locators, which the item-7 design deliberately keeps subset-matched (retrieve_guarantee.md
        // item 7 decision 1) so a grammar can add new captures without breaking existing evals.
        let hits = vec![hit(Some(BTreeMap::from([
            ("article".to_string(), "33".to_string()),
            ("number".to_string(), "33".to_string()),
        ])))];
        let result = eval_case_result(
            &eval(vec![BTreeMap::from([(
                "article".to_string(),
                "33".to_string(),
            )])]),
            &hits,
            None,
        );
        assert_eq!(result.outcome, EvalOutcome::Passed);
    }

    #[test]
    fn fails_when_no_hit_matches_any_expect_entry() {
        let hits = vec![hit(Some(BTreeMap::from([(
            "recital".to_string(),
            "1".to_string(),
        )])))];
        let result = eval_case_result(
            &eval(vec![BTreeMap::from([(
                "article".to_string(),
                "33".to_string(),
            )])]),
            &hits,
            None,
        );
        assert_eq!(result.outcome, EvalOutcome::Failed);
        assert!(result.reason.unwrap().contains("recital"));
    }

    #[test]
    fn fails_on_zero_hits_with_a_distinct_reason_from_a_wrong_hit() {
        let result = eval_case_result(
            &eval(vec![BTreeMap::from([(
                "article".to_string(),
                "33".to_string(),
            )])]),
            &[],
            None,
        );
        assert_eq!(result.outcome, EvalOutcome::Failed);
        assert_eq!(result.reason.as_deref(), Some("no hits returned"));
    }

    #[test]
    fn any_of_multiple_expect_entries_matches() {
        let hits = vec![hit(Some(BTreeMap::from([(
            "recital".to_string(),
            "1".to_string(),
        )])))];
        let result = eval_case_result(
            &eval(vec![
                BTreeMap::from([("article".to_string(), "33".to_string())]),
                BTreeMap::from([("recital".to_string(), "1".to_string())]),
            ]),
            &hits,
            None,
        );
        assert_eq!(result.outcome, EvalOutcome::Passed);
    }

    #[test]
    fn document_identity_oracle_rejects_same_locator_from_another_instrument() {
        let hits = vec![hit(Some(BTreeMap::from([(
            "article".to_string(),
            "34".to_string(),
        )])))];
        let expected = std::collections::HashSet::from(["doc.2".to_string()]);
        let result = eval_case_result(
            &eval(vec![BTreeMap::from([(
                "article".to_string(),
                "34".to_string(),
            )])]),
            &hits,
            Some(&expected),
        );
        assert_eq!(result.outcome, EvalOutcome::Failed);
    }
}
