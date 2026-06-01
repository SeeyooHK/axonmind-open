use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Confidence score in `[0.0, 1.0]`.
///
/// Default values by `ExtractorKind` (see `evidence.rs`):
/// - `Manual`:     1.00 — user entered, ground truth
/// - `Connector`:  0.95 — structured connector field
/// - `Rule`:       0.85 — deterministic extraction rule
/// - `Llm`:        0.50 — single LLM source
/// - `Calculated`: inherits from sources
///
/// Multiple evidence items aggregate via noisy-OR:
///   `combined = 1 - ∏(1 - evidence_i.confidence)`
///
/// UI rendering thresholds:
/// - ≥ 0.85 → solid line, no badge
/// - 0.50–0.85 → dashed line, "inferred" badge
/// - < 0.50 → dotted line, "low confidence" badge (opt-in to display)
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Confidence(pub f32);

impl Confidence {
    pub const MANUAL: Self = Self(1.00);
    pub const CONNECTOR: Self = Self(0.95);
    pub const RULE: Self = Self(0.85);
    pub const LLM: Self = Self(0.50);

    /// Noisy-OR aggregation. Three LLM extractions at 0.50 each → 0.875.
    pub fn aggregate(confidences: &[Self]) -> Self {
        let product: f32 = confidences.iter().map(|c| 1.0 - c.0).product();
        Self((1.0 - product).clamp(0.0, 1.0))
    }

    /// Contradiction-aware aggregation.
    ///
    /// `combined = support_or × (1 − contradiction_or)`
    ///
    /// When `contradiction` is empty this equals `aggregate(support)` exactly —
    /// no behavioural change for nodes that have no contradicting edges.
    /// When both sides are non-empty, strong contradiction pulls the result toward zero.
    pub fn aggregate_signed(support: &[Self], contradiction: &[Self]) -> Self {
        let or_of = |slice: &[Self]| -> f32 {
            if slice.is_empty() {
                return 0.0;
            }
            let product: f32 = slice.iter().map(|c| 1.0 - c.0).product();
            1.0 - product
        };
        let combined = or_of(support) * (1.0 - or_of(contradiction));
        Self(combined.clamp(0.0, 1.0))
    }

    pub fn value(self) -> f32 {
        self.0
    }
}

impl Default for Confidence {
    fn default() -> Self {
        Self::MANUAL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_signed_empty_contradiction_equals_aggregate() {
        // WHY: the backward-compat invariant — adding this function must not change
        // confidence computation for nodes with no contradicting evidence.
        let support = [Confidence::RULE, Confidence::LLM]; // 0.85, 0.50
        let expected = Confidence::aggregate(&support);
        let actual = Confidence::aggregate_signed(&support, &[]);
        assert!(
            (expected.0 - actual.0).abs() < 1e-6,
            "aggregate_signed with empty contradiction should equal aggregate: {expected:?} vs {actual:?}"
        );
    }

    #[test]
    fn strong_contradiction_dampens_below_support_alone() {
        // WHY: this is the whole point — contradicting evidence must lower confidence,
        // not just leave it unchanged.
        let support = [Confidence(0.9)];
        let contradiction = [Confidence(0.8)];
        let dampened = Confidence::aggregate_signed(&support, &contradiction);
        let support_only = Confidence::aggregate(&support);
        assert!(
            dampened.0 < support_only.0,
            "contradiction should lower confidence: dampened={dampened:?}, support_only={support_only:?}"
        );
    }

    #[test]
    fn both_empty_returns_zero() {
        assert_eq!(Confidence::aggregate_signed(&[], &[]).0, 0.0);
    }

    #[test]
    fn result_always_in_unit_interval() {
        let cases: &[(&[Confidence], &[Confidence])] = &[
            (&[Confidence(1.0)], &[Confidence(1.0)]),
            (&[], &[Confidence(0.99)]),
            (&[Confidence(0.99)], &[]),
        ];
        for (s, c) in cases {
            let v = Confidence::aggregate_signed(s, c).0;
            assert!((0.0..=1.0).contains(&v), "result out of [0,1]: {v}");
        }
    }
}
