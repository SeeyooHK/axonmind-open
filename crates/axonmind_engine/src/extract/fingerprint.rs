//! Phase 2: structural fingerprint and change-classification for documents.
//!
//! Two hashes per document:
//!   content_sha256    — hash of raw file bytes (already computed in ingest_file)
//!   structural_sha256 — hash over extraction-relevant structure only:
//!                       heading texts + table shapes (headers, row labels, numeric flags)
//!
//! Change classifier:
//!   content same                        → Skip         (nothing changed)
//!   content differs, structural same    → CosmeticRefresh (skip LLM; rules may re-run)
//!   content differs, structural differs → FullReextract
//!   not in cache                        → FullReextract
use crate::extract::value_parse::parse_metric_cell;
use crate::ingest::{DocumentBlock, NormalizedDocument};
use sha2::{Digest, Sha256};

pub struct DocFingerprint {
    pub content_sha256: String,
    pub structural_sha256: String,
}

pub enum ReextractDecision {
    Skip,
    /// Content changed but extraction-relevant structure is identical.
    /// Skip the LLM call; optionally re-run deterministic rules to refresh evidence quotes.
    CosmeticRefresh,
    FullReextract,
}

/// Hash the extraction-relevant structure of `doc`.
///
/// Includes:
/// - Heading texts (lowercased, whitespace collapsed) — KPI detection keys off these.
/// - Table headers + first-column row labels (lowercased) + per-cell numeric shape.
///
/// Excludes: paragraph prose, casing changes, punctuation, blank lines.
pub fn structural_signature(doc: &NormalizedDocument) -> String {
    let mut hasher = Sha256::new();

    for block in &doc.blocks {
        if let DocumentBlock::Heading { text, .. } = block {
            let collapsed = text
                .trim()
                .to_ascii_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            hasher.update(b"h:");
            hasher.update(collapsed.as_bytes());
            hasher.update(b"\n");
        }
    }

    for table in &doc.tables {
        for header in &table.headers {
            let norm = header.trim().to_ascii_lowercase();
            hasher.update(b"th:");
            hasher.update(norm.as_bytes());
            hasher.update(b"|");
        }
        hasher.update(b"\n");

        for row in &table.rows {
            if let Some(label) = row.first() {
                let norm = label.trim().to_ascii_lowercase();
                hasher.update(b"l:");
                hasher.update(norm.as_bytes());
                hasher.update(b"|");
            }
            for cell in row.iter().skip(1) {
                hasher.update(if parse_metric_cell(cell.trim()).is_some() {
                    b"1"
                } else {
                    b"0"
                });
            }
            hasher.update(b"\n");
        }
    }

    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn classify(prev: Option<&DocFingerprint>, next: &DocFingerprint) -> ReextractDecision {
    match prev {
        None => ReextractDecision::FullReextract,
        Some(p) if p.content_sha256 == next.content_sha256 => ReextractDecision::Skip,
        Some(p) if p.structural_sha256 == next.structural_sha256 => {
            ReextractDecision::CosmeticRefresh
        }
        Some(_) => ReextractDecision::FullReextract,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{NormalizedDocument, NormalizedTable, SourceSpan};

    fn make_doc(headings: &[&str], tables: Vec<NormalizedTable>) -> NormalizedDocument {
        let blocks = headings
            .iter()
            .map(|t| DocumentBlock::Heading {
                level: 1,
                text: t.to_string(),
                span: SourceSpan::default(),
            })
            .collect();
        NormalizedDocument {
            id: "test".into(),
            source_path: None,
            sha256: "abc".into(),
            title: None,
            blocks,
            tables,
        }
    }

    /// WHY: cosmetic heading change must not trigger FullReextract — that wastes an LLM call.
    /// The structural hash must ignore casing and whitespace.
    #[test]
    fn casing_change_is_cosmetic() {
        let a = make_doc(&["Revenue Growth"], vec![]);
        let b = make_doc(&["revenue growth"], vec![]);
        assert_eq!(
            structural_signature(&a),
            structural_signature(&b),
            "casing-only heading change must produce the same structural hash"
        );
    }

    /// WHY: renaming a heading changes KPI detection keys — must be FullReextract.
    #[test]
    fn heading_rename_changes_signature() {
        let a = make_doc(&["Churn Rate"], vec![]);
        let b = make_doc(&["Retention Rate"], vec![]);
        assert_ne!(
            structural_signature(&a),
            structural_signature(&b),
            "heading rename must produce a different structural hash"
        );
    }

    /// WHY: changing a prose paragraph must not affect the structural hash.
    #[test]
    fn prose_change_does_not_affect_signature() {
        let mut a = make_doc(&["Revenue"], vec![]);
        let mut b = make_doc(&["Revenue"], vec![]);
        a.blocks.push(DocumentBlock::Paragraph {
            text: "Old text".into(),
            span: SourceSpan::default(),
        });
        b.blocks.push(DocumentBlock::Paragraph {
            text: "New text — completely rewritten".into(),
            span: SourceSpan::default(),
        });
        assert_eq!(
            structural_signature(&a),
            structural_signature(&b),
            "paragraph prose change must not change the structural hash"
        );
    }

    /// WHY: classify must return Skip when content bytes are identical —
    /// even if we have not yet parsed the doc.
    #[test]
    fn identical_content_hash_gives_skip() {
        let fp = DocFingerprint {
            content_sha256: "abc".into(),
            structural_sha256: "def".into(),
        };
        assert!(matches!(classify(Some(&fp), &fp), ReextractDecision::Skip));
    }

    /// WHY: new file (not in cache) must trigger FullReextract.
    #[test]
    fn no_cache_gives_full_reextract() {
        let fp = DocFingerprint {
            content_sha256: "abc".into(),
            structural_sha256: "def".into(),
        };
        assert!(matches!(
            classify(None, &fp),
            ReextractDecision::FullReextract
        ));
    }

    /// WHY: content changed but structural hash same → CosmeticRefresh, NOT FullReextract.
    /// This is the core saving — we skip the LLM call for cosmetic edits.
    #[test]
    fn same_structure_different_content_gives_cosmetic_refresh() {
        let prev = DocFingerprint {
            content_sha256: "old_content".into(),
            structural_sha256: "same_structure".into(),
        };
        let next = DocFingerprint {
            content_sha256: "new_content".into(),
            structural_sha256: "same_structure".into(),
        };
        assert!(matches!(
            classify(Some(&prev), &next),
            ReextractDecision::CosmeticRefresh
        ));
    }
}
