use std::path::Path;

use regex::Regex;

use crate::store::{DocumentAliasRecord, DocumentIdentityRecord};
use crate::structure::model::{CorpusBinding, IdentityRule, StructurePackage};

#[derive(Debug, Clone)]
pub struct DerivedIdentity {
    pub identity: DocumentIdentityRecord,
    pub profile_name: Option<String>,
}

pub fn derive_identity(
    doc_node_id: &str,
    raw_title: Option<&str>,
    source_path: Option<&str>,
    markdown: Option<&str>,
    packages: &[StructurePackage],
) -> DerivedIdentity {
    let source_filename = source_path
        .and_then(|p| Path::new(p).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or(raw_title.unwrap_or(doc_node_id))
        .to_string();
    let fallback_title = raw_title
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| filename_stem(&source_filename));

    let mut best = DocumentIdentityRecord {
        doc_node_id: doc_node_id.to_string(),
        source_filename: source_filename.clone(),
        source_path: source_path.map(str::to_string),
        raw_title: raw_title.map(str::to_string),
        canonical_title: fallback_title.clone(),
        language: None,
        jurisdiction: Vec::new(),
        domain: Vec::new(),
        instrument_type: None,
        corpus: Vec::new(),
        confidence: 0.35,
        reviewed_at: None,
        updated_at: chrono::Utc::now().timestamp(),
        pinned_profile: None,
        aliases: vec![DocumentAliasRecord {
            alias: source_filename.clone(),
            alias_norm: normalize_alias(&source_filename),
            source: "filename".to_string(),
        }],
    };
    let mut best_profile = None;

    for pkg in packages {
        for rule in &pkg.identity.rules {
            if let Some(identity) = apply_rule(
                doc_node_id,
                raw_title,
                source_path,
                markdown,
                &source_filename,
                &fallback_title,
                rule,
            ) {
                if identity.confidence > best.confidence {
                    best = identity;
                    best_profile = Some(rule.profile.clone());
                }
            }
        }
        if let Some(corpus) = pkg.corpus.as_ref() {
            apply_corpus_bindings(&mut best, &corpus.binds);
        }
    }

    DerivedIdentity {
        identity: best,
        profile_name: best_profile,
    }
}

fn apply_rule(
    doc_node_id: &str,
    raw_title: Option<&str>,
    source_path: Option<&str>,
    markdown: Option<&str>,
    source_filename: &str,
    fallback_title: &str,
    rule: &IdentityRule,
) -> Option<DocumentIdentityRecord> {
    let regex = Regex::new(&rule.pattern).ok()?;
    for source in &rule.sources {
        let haystack = source_text(source, source_filename, raw_title, fallback_title, markdown)?;
        let Some(captures) = regex.captures(&haystack) else {
            continue;
        };
        let canonical_title = render_numeric_template(&rule.canonical_title, &captures);
        let mut aliases = vec![DocumentAliasRecord {
            alias: source_filename.to_string(),
            alias_norm: normalize_alias(source_filename),
            source: "filename".to_string(),
        }];
        for alias in &rule.aliases {
            let value = render_numeric_template(alias, &captures);
            let norm = normalize_alias(&value);
            if norm.is_empty() || aliases.iter().any(|existing| existing.alias_norm == norm) {
                continue;
            }
            aliases.push(DocumentAliasRecord {
                alias: value,
                alias_norm: norm,
                source: "inferred".to_string(),
            });
        }
        return Some(DocumentIdentityRecord {
            doc_node_id: doc_node_id.to_string(),
            source_filename: source_filename.to_string(),
            source_path: source_path.map(str::to_string),
            raw_title: raw_title.map(str::to_string),
            canonical_title,
            language: rule.set.language.clone(),
            jurisdiction: rule.set.jurisdiction.clone(),
            domain: rule.set.domain.clone(),
            instrument_type: rule.set.instrument_type.clone(),
            corpus: rule.set.corpus.clone(),
            confidence: rule.confidence,
            reviewed_at: None,
            updated_at: chrono::Utc::now().timestamp(),
            pinned_profile: None,
            aliases,
        });
    }
    None
}

fn source_text(
    source: &str,
    source_filename: &str,
    raw_title: Option<&str>,
    fallback_title: &str,
    markdown: Option<&str>,
) -> Option<String> {
    if source == "filename" {
        return Some(source_filename.to_string());
    }
    if source == "raw_title" {
        return Some(raw_title.unwrap_or(fallback_title).to_string());
    }
    if let Some(rest) = source.strip_prefix("first_chars:") {
        let count: usize = rest.parse().ok()?;
        return Some(
            markdown
                .unwrap_or_default()
                .chars()
                .take(count)
                .collect::<String>(),
        );
    }
    if let Some(rest) = source.strip_prefix("first_lines:") {
        let count: usize = rest.parse().ok()?;
        return Some(
            markdown
                .unwrap_or_default()
                .lines()
                .take(count)
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    None
}

fn apply_corpus_bindings(identity: &mut DocumentIdentityRecord, bindings: &[CorpusBinding]) {
    for binding in bindings {
        if !matches_identity(identity, binding) {
            continue;
        }
        for alias in &binding.aliases {
            let norm = normalize_alias(alias);
            if norm.is_empty()
                || identity
                    .aliases
                    .iter()
                    .any(|existing| existing.alias_norm == norm)
            {
                continue;
            }
            identity.aliases.push(DocumentAliasRecord {
                alias: alias.clone(),
                alias_norm: norm,
                source: "corpus".to_string(),
            });
        }
        merge_unique(&mut identity.corpus, &binding.corpus);
        merge_unique(&mut identity.domain, &binding.domain);
    }
}

fn matches_identity(identity: &DocumentIdentityRecord, binding: &CorpusBinding) -> bool {
    let matcher = &binding.matcher;
    if let Some(title) = matcher.canonical_title.as_ref() {
        let Ok(regex) = Regex::new(title) else {
            return false;
        };
        if !regex.is_match(&identity.canonical_title) {
            return false;
        }
    }
    if let Some(corpus) = matcher.corpus.as_ref() {
        if !identity.corpus.iter().any(|value| value == corpus) {
            return false;
        }
    }
    if let Some(kind) = matcher.instrument_type.as_ref() {
        if identity.instrument_type.as_deref() != Some(kind.as_str()) {
            return false;
        }
    }
    true
}

fn render_numeric_template(template: &str, captures: &regex::Captures<'_>) -> String {
    let mut out = template.to_string();
    for idx in 1..captures.len() {
        let needle = format!("{{{idx}}}");
        let replacement = captures.get(idx).map(|m| m.as_str()).unwrap_or_default();
        out = out.replace(&needle, replacement);
    }
    out
}

fn merge_unique(target: &mut Vec<String>, values: &[String]) {
    for value in values {
        if !target.iter().any(|existing| existing == value) {
            target.push(value.clone());
        }
    }
}

fn normalize_alias(alias: &str) -> String {
    alias
        .chars()
        .flat_map(|c| c.to_lowercase())
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn filename_stem(filename: &str) -> String {
    Path::new(filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(filename)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_handbook_identity_from_opening_text() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");
        let pkg = crate::structure::model::StructurePackage::from_dir(&dir).expect("package");
        let derived = derive_identity(
            "doc.handbook",
            Some("Untitled"),
            Some("Acme_Operations_Handbook_Ed3.pdf"),
            Some("ACME OPERATIONS HANDBOOK, EDITION 3"),
            &[pkg],
        );
        assert_eq!(
            derived.identity.instrument_type.as_deref(),
            Some("handbook")
        );
        assert!(
            derived
                .identity
                .aliases
                .iter()
                .any(|alias| alias.alias == "The Handbook")
        );
        assert_eq!(derived.profile_name.as_deref(), Some("handbook-en"));
        // pinned_profile is reserved for an explicit user pin that survives
        // re-ingestion; package-derived identity must not write it. Profile
        // selection is carried separately via DerivedIdentity.profile_name.
        assert_eq!(derived.identity.pinned_profile, None);
    }
}
