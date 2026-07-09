use std::collections::BTreeMap;

use axonmind_core::AxonMindError;

use crate::pageindex::tree::{PersistTree, SectionRow};
use crate::store::{DocumentIdentityRecord, UnitRefRecord};
use crate::structure::model::{CaptureSelector, LevelSpec, ProfileDefinition, StructureUnit};

const PATH_SEP: &str = " \u{203a} ";

#[derive(Debug, Clone)]
pub struct ParsedDocumentIndex {
    pub units: Vec<ParsedUnit>,
    pub refs: Vec<UnitRefRecord>,
    pub tree: PersistTree,
}

#[derive(Debug, Clone)]
pub struct ParsedUnit {
    pub unit_id: String,
    pub doc_node_id: String,
    pub parent_unit_id: Option<String>,
    pub section_id: String,
    pub unit_kind: String,
    pub label: String,
    pub label_norm: String,
    pub title: Option<String>,
    pub ordinal: i64,
    pub level: i64,
    pub text: String,
    pub span_start: i64,
    pub span_end: i64,
    pub page_start: Option<i64>,
    pub page_end: Option<i64>,
    pub path: String,
    pub citation: String,
    pub package_name: String,
    pub profile_name: String,
    pub profile_version: i64,
    pub confidence: f32,
    pub locators: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
struct LineSpan {
    text: String,
    start: usize,
}

#[derive(Debug, Clone)]
struct MarkerMatch {
    line_idx: usize,
    unit: StructureUnit,
    captures: BTreeMap<String, String>,
    title: Option<String>,
}

pub fn parse_document(
    package_name: &str,
    profile: &ProfileDefinition,
    doc_node_id: &str,
    identity: &DocumentIdentityRecord,
    markdown: &str,
) -> Result<Option<ParsedDocumentIndex>, AxonMindError> {
    let cleaned = strip_toc(markdown, profile);
    let lines = split_lines(&cleaned);
    let top_level_units = profile
        .units
        .iter()
        .filter(|unit| unit.parent.is_none())
        .cloned()
        .collect::<Vec<_>>();
    let markers = find_markers(&lines, profile, &top_level_units, None);
    if markers.is_empty() {
        return Ok(None);
    }

    let mut units = Vec::new();
    let mut sections = Vec::new();
    let mut refs = Vec::new();
    let mut seen_unit_ids = BTreeMap::new();
    build_units(
        package_name,
        profile,
        doc_node_id,
        identity,
        &cleaned,
        &lines,
        cleaned.len(),
        None,
        &markers,
        &mut units,
        &mut sections,
        &mut refs,
        &mut seen_unit_ids,
    )?;

    Ok(Some(ParsedDocumentIndex {
        refs,
        tree: PersistTree {
            doc_node_id: doc_node_id.to_string(),
            sha256: String::new(),
            title: identity.canonical_title.clone(),
            doc_summary: None,
            sections,
        },
        units,
    }))
}

fn build_units(
    package_name: &str,
    profile: &ProfileDefinition,
    doc_node_id: &str,
    identity: &DocumentIdentityRecord,
    markdown: &str,
    lines: &[LineSpan],
    span_limit: usize,
    parent: Option<&ParsedUnit>,
    markers: &[MarkerMatch],
    units: &mut Vec<ParsedUnit>,
    sections: &mut Vec<SectionRow>,
    refs: &mut Vec<UnitRefRecord>,
    seen_unit_ids: &mut BTreeMap<String, usize>,
) -> Result<(), AxonMindError> {
    for (ordinal, marker) in markers.iter().enumerate() {
        let start = lines[marker.line_idx].start;
        let end = markers
            .get(ordinal + 1)
            .map(|next| lines[next.line_idx].start)
            .unwrap_or(span_limit);
        let text = markdown[start..end].trim().to_string();
        if text.is_empty() {
            continue;
        }
        let label = render_template(&marker.unit.label, identity, parent, &marker.captures);
        let label_norm =
            render_template(&marker.unit.label_norm, identity, parent, &marker.captures);
        let citation = render_template(&marker.unit.citation, identity, parent, &marker.captures);
        let title = resolve_title(marker, lines, profile);
        let display_title = match title.as_deref() {
            Some(value) if !value.is_empty() => format!("{label} - {value}"),
            _ => label.clone(),
        };
        let path = match title.as_deref() {
            Some(value) if !value.is_empty() => {
                format!(
                    "{}{}{}{}{}",
                    identity.canonical_title, PATH_SEP, label, PATH_SEP, value
                )
            }
            _ => format!("{}{}{}", identity.canonical_title, PATH_SEP, label),
        };
        let unit_id = dedupe_unit_id(format!("{doc_node_id}:{label_norm}"), seen_unit_ids);
        let section_id = format!("{doc_node_id}#{:04}", sections.len() + 1);
        let locators = marker.captures.clone();
        let level = resolve_level(&marker.unit.level, &locators);
        let unit = ParsedUnit {
            unit_id: unit_id.clone(),
            doc_node_id: doc_node_id.to_string(),
            parent_unit_id: parent.map(|value| value.unit_id.clone()),
            section_id: section_id.clone(),
            unit_kind: marker.unit.kind.clone(),
            label: label.clone(),
            label_norm: label_norm.clone(),
            title: title.clone(),
            ordinal: ordinal as i64,
            level,
            text: text.clone(),
            span_start: start as i64,
            span_end: end as i64,
            page_start: None,
            page_end: None,
            path: path.clone(),
            citation,
            package_name: package_name.to_string(),
            profile_name: profile.profile.name.clone(),
            profile_version: profile.profile.version,
            confidence: 1.0,
            locators,
        };
        sections.push(SectionRow {
            section_id,
            doc_node_id: doc_node_id.to_string(),
            parent_section_id: parent.map(|value| value.section_id.clone()),
            ordinal: unit.ordinal,
            level,
            title: display_title,
            path,
            summary: None,
            text: Some(text.clone()),
            span_start: start as i64,
            span_end: end as i64,
        });
        refs.extend(extract_refs(profile, &unit));
        units.push(unit.clone());

        let child_specs = profile
            .units
            .iter()
            .filter(|candidate| candidate.parent.as_deref() == Some(marker.unit.kind.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if child_specs.is_empty() {
            continue;
        }
        let child_lines = slice_lines(lines, start, end);
        let child_markers = find_markers(&child_lines, profile, &child_specs, Some(marker));
        if child_markers.is_empty() {
            continue;
        }
        build_units(
            package_name,
            profile,
            doc_node_id,
            identity,
            markdown,
            &child_lines,
            end,
            Some(&unit),
            &child_markers,
            units,
            sections,
            refs,
            seen_unit_ids,
        )?;
    }
    Ok(())
}

/// Guarantees `unit_id` uniqueness within one document's parse. Two markers can legitimately
/// render the same `label_norm` (e.g. a duplicated section number left behind by an
/// imperfectly-stripped TOC) — without this, `replace_doc_units`'s single INSERT transaction
/// hits a PRIMARY KEY collision and the *entire* document's units are discarded (see
/// docs/structure_packages.md's 2026-07-06 findings). Suffixing keeps every extracted unit
/// instead of losing a whole document over one collision; the first occurrence keeps the clean
/// id so normal (non-colliding) documents are unaffected.
fn dedupe_unit_id(base: String, seen: &mut BTreeMap<String, usize>) -> String {
    let count = seen.entry(base.clone()).or_insert(0);
    *count += 1;
    if *count == 1 {
        base
    } else {
        format!("{base}~{count}")
    }
}

/// Strips a leading Markdown heading marker (`#`..`######` + whitespace) before applying a
/// structure marker regex. The PDF→Markdown extractor renders many structural headings
/// (e.g. article/section titles) as Markdown headings rather than bare text; marker regexes
/// are authored against the bare-text form (`^Article\s+\d+`), so without this they silently
/// miss every heading-rendered occurrence and only match the rare bare-text ones.
fn strip_heading_prefix(text: &str) -> &str {
    let stripped = text.trim_start_matches('#');
    if stripped.len() < text.len() && stripped.starts_with(|c: char| c.is_whitespace()) {
        stripped.trim_start()
    } else {
        text
    }
}

fn slice_lines(lines: &[LineSpan], start: usize, end: usize) -> Vec<LineSpan> {
    lines
        .iter()
        .filter(|line| line.start >= start && line.start < end)
        .cloned()
        .collect()
}

fn find_markers(
    lines: &[LineSpan],
    profile: &ProfileDefinition,
    candidates: &[StructureUnit],
    parent_marker: Option<&MarkerMatch>,
) -> Vec<MarkerMatch> {
    // Both the region boundary and the marker regex are invariant across lines for a given
    // candidate within one `find_markers` call — resolve them once here instead of inside the
    // per-line loop below. Previously `region_allows` recomputed the region boundary (an O(lines)
    // scan from the document start, recompiling the boundary regex on every line it checked) for
    // every single (line, candidate) pair, making a region-scoped candidate's cost O(lines^2).
    // Measured on the real GDPR Regulation (1457 lines, `recital` unit scoped to the `preamble`
    // region): 184s in this function alone. Hoisting both here makes it O(lines) total.
    let gates: Vec<RegionGate> = candidates
        .iter()
        .map(|candidate| resolve_region_gate(profile, candidate, parent_marker, lines))
        .collect();
    let compiled: Vec<Option<regex::Regex>> = candidates
        .iter()
        .map(|candidate| regex::Regex::new(&candidate.marker).ok())
        .collect();

    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        let target = strip_heading_prefix(trimmed);
        for (candidate, (gate, regex)) in candidates.iter().zip(gates.iter().zip(compiled.iter())) {
            if !gate.allows(idx) {
                continue;
            }
            let Some(regex) = regex else {
                continue;
            };
            let Some(captures) = regex.captures(target) else {
                continue;
            };
            let values = candidate
                .captures
                .iter()
                .filter_map(|(name, selector)| {
                    resolve_capture(selector, &captures).map(|value| (name.clone(), value))
                })
                .collect::<BTreeMap<_, _>>();
            out.push(MarkerMatch {
                line_idx: idx,
                unit: candidate.clone(),
                captures: values,
                title: captures.get(2).map(|m| m.as_str().trim().to_string()),
            });
            break;
        }
    }
    out
}

enum RegionGate {
    Allow,
    Deny,
    Boundary(Option<usize>),
}

impl RegionGate {
    fn allows(&self, line_idx: usize) -> bool {
        match self {
            RegionGate::Allow => true,
            RegionGate::Deny => false,
            RegionGate::Boundary(Some(boundary_idx)) => line_idx < *boundary_idx,
            RegionGate::Boundary(None) => true,
        }
    }
}

fn resolve_region_gate(
    profile: &ProfileDefinition,
    unit: &StructureUnit,
    parent_marker: Option<&MarkerMatch>,
    lines: &[LineSpan],
) -> RegionGate {
    if parent_marker.is_some() {
        return RegionGate::Allow;
    }
    let Some(region_name) = unit.region.as_deref() else {
        return RegionGate::Allow;
    };
    let Some(region) = profile
        .region
        .iter()
        .find(|value| value.name == region_name)
    else {
        return RegionGate::Deny;
    };
    if region.from != "start" {
        return RegionGate::Deny;
    }
    let Some(until_kind) = region.until.strip_prefix("first:") else {
        return RegionGate::Deny;
    };
    // Boundary = the first line (searched from the document start, not from the candidate's own
    // line) whose text matches an `until_kind` marker. A candidate belongs to the region iff it
    // sits before that boundary; no boundary found means the whole document is the region.
    // (Previously this scanned forward *from* the candidate's line and compared `idx == line_idx`,
    // which is only ever true when the candidate's own line happens to be the boundary itself —
    // every genuine region-scoped unit before the boundary was rejected. Confirmed via the
    // "note.1" line in tests/fixtures/generic_manual/fixtures/handbook-en.md, which the old logic
    // silently dropped and no existing test asserted on.)
    let boundary_regexes: Vec<regex::Regex> = profile
        .units
        .iter()
        .filter(|candidate| candidate.parent.is_none() && candidate.kind == until_kind)
        .filter_map(|candidate| regex::Regex::new(&candidate.marker).ok())
        .collect();
    let boundary = lines.iter().enumerate().find_map(|(idx, line)| {
        let target = strip_heading_prefix(line.text.trim());
        boundary_regexes
            .iter()
            .any(|regex| regex.is_match(target))
            .then_some(idx)
    });
    RegionGate::Boundary(boundary)
}

fn resolve_capture(selector: &CaptureSelector, captures: &regex::Captures<'_>) -> Option<String> {
    match selector {
        CaptureSelector::Index(index) => {
            captures.get(*index).map(|m| m.as_str().trim().to_string())
        }
        CaptureSelector::FirstOf(spec) => spec
            .split('|')
            .filter_map(|value| value.parse::<usize>().ok())
            .find_map(|index| captures.get(index).map(|m| m.as_str().trim().to_string()))
            .filter(|value| !value.is_empty()),
    }
}

fn resolve_title(
    marker: &MarkerMatch,
    lines: &[LineSpan],
    profile: &ProfileDefinition,
) -> Option<String> {
    match marker.unit.title.as_deref() {
        Some("inline") => marker.title.clone().filter(|value| !value.is_empty()),
        Some("inline_or_next_heading") => marker
            .title
            .clone()
            .filter(|value| !value.is_empty())
            .or_else(|| next_heading(lines, marker.line_idx, profile)),
        Some("next_heading") => next_heading(lines, marker.line_idx, profile),
        _ => None,
    }
}

fn next_heading(
    lines: &[LineSpan],
    start_idx: usize,
    profile: &ProfileDefinition,
) -> Option<String> {
    for line in lines.iter().skip(start_idx + 1) {
        let trimmed = line.text.trim();
        if trimmed.is_empty() || is_toc_heading(trimmed, profile) {
            continue;
        }
        return Some(trimmed.to_string());
    }
    None
}

fn strip_toc(markdown: &str, profile: &ProfileDefinition) -> String {
    let Some(toc) = profile.toc.as_ref() else {
        return markdown.to_string();
    };
    markdown
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if is_toc_heading(trimmed, profile) {
                return false;
            }
            !toc.strip_lines.iter().any(|pattern| {
                regex::Regex::new(pattern)
                    .ok()
                    .is_some_and(|regex| regex.is_match(trimmed))
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_toc_heading(line: &str, profile: &ProfileDefinition) -> bool {
    profile.toc.as_ref().is_some_and(|toc| {
        toc.strip_headings
            .iter()
            .any(|heading| heading.eq_ignore_ascii_case(line))
    })
}

fn extract_refs(profile: &ProfileDefinition, unit: &ParsedUnit) -> Vec<UnitRefRecord> {
    let mut refs = Vec::new();
    for reference in &profile.refs {
        let Ok(regex) = regex::Regex::new(&reference.pattern) else {
            continue;
        };
        for captures in regex.captures_iter(&unit.text) {
            let target_label_norm =
                render_numeric_ref_template(&reference.target_label_norm, &captures);
            if target_label_norm == unit.label_norm {
                continue;
            }
            refs.push(UnitRefRecord {
                from_unit_id: unit.unit_id.clone(),
                to_doc_node_id: None,
                target_label: target_label_norm.clone(),
                target_label_norm,
                ref_text: captures
                    .get(0)
                    .map(|value| value.as_str().to_string())
                    .unwrap_or_default(),
            });
        }
    }
    refs
}

fn render_template(
    template: &str,
    identity: &DocumentIdentityRecord,
    parent: Option<&ParsedUnit>,
    captures: &BTreeMap<String, String>,
) -> String {
    let mut out = template.replace("{doc}", &identity.canonical_title);
    for (key, value) in captures {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    if let Some(parent) = parent {
        out = out.replace("{parent.label}", &parent.label);
        out = out.replace("{parent.label_norm}", &parent.label_norm);
        for (key, value) in &parent.locators {
            out = out.replace(&format!("{{parent.{key}}}"), value);
        }
    }
    out
}

fn render_numeric_ref_template(template: &str, captures: &regex::Captures<'_>) -> String {
    let mut out = template.to_string();
    for idx in 1..captures.len() {
        let replacement = captures
            .get(idx)
            .map(|value| value.as_str())
            .unwrap_or_default();
        out = out.replace(&format!("{{{idx}}}"), replacement);
    }
    out
}

fn resolve_level(level: &LevelSpec, captures: &BTreeMap<String, String>) -> i64 {
    match level {
        LevelSpec::Fixed(value) => *value,
        LevelSpec::Depth(spec) => spec
            .strip_prefix("depth:")
            .and_then(|key| captures.get(key))
            .map(|value| value.split('.').count() as i64)
            .unwrap_or(1),
    }
}

fn split_lines(markdown: &str) -> Vec<LineSpan> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for line in markdown.lines() {
        out.push(LineSpan {
            text: line.to_string(),
            start: offset,
        });
        offset += line.len() + 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_handbook_fixture_chapter_and_clause() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");
        let pkg = crate::structure::model::StructurePackage::from_dir(&dir).expect("package");
        let profile = pkg
            .profiles
            .iter()
            .find(|profile| profile.profile.name == "handbook-en")
            .expect("profile");
        let markdown =
            std::fs::read_to_string(dir.join("fixtures/handbook-en.md")).expect("fixture");
        let identity = crate::structure::derive_identity(
            "doc.handbook",
            Some("Acme Operations Handbook"),
            Some("acme_operations_handbook_ed3.pdf"),
            Some(&markdown),
            std::slice::from_ref(&pkg),
        )
        .identity;
        let parsed = parse_document(
            &pkg.manifest.package.name,
            profile,
            "doc.handbook",
            &identity,
            &markdown,
        )
        .expect("parse")
        .expect("units");
        assert!(
            parsed
                .units
                .iter()
                .any(|unit| unit.label_norm == "chapter.3")
        );
        assert!(
            parsed
                .units
                .iter()
                .any(|unit| unit.label_norm == "chapter.3.c1")
        );
    }

    #[test]
    fn parses_procedure_fixture_and_strips_toc() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");
        let pkg = crate::structure::model::StructurePackage::from_dir(&dir).expect("package");
        let profile = pkg
            .profiles
            .iter()
            .find(|profile| profile.profile.name == "procedure-en")
            .expect("profile");
        let markdown =
            std::fs::read_to_string(dir.join("fixtures/procedure-en.md")).expect("fixture");
        let identity = crate::structure::derive_identity(
            "doc.procedure",
            Some("Acme Field Procedure 9"),
            Some("acme_field_procedure_9.pdf"),
            Some(&markdown),
            std::slice::from_ref(&pkg),
        )
        .identity;
        let parsed = parse_document(
            &pkg.manifest.package.name,
            profile,
            "doc.procedure",
            &identity,
            &markdown,
        )
        .expect("parse")
        .expect("units");
        assert_eq!(
            parsed
                .units
                .iter()
                .filter(|unit| unit.unit_kind == "step")
                .count(),
            1
        );
        assert_eq!(parsed.units[0].label_norm, "step.1.3.6");
    }

    fn handbook_profile() -> (
        crate::structure::model::StructurePackage,
        std::path::PathBuf,
    ) {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/generic_manual");
        let pkg = crate::structure::model::StructurePackage::from_dir(&dir).expect("package");
        (pkg, dir)
    }

    /// A region-scoped unit (`note`, `region = "preface"`) positioned before the region's
    /// boundary marker (`Chapter 3`) must be captured. Regression for the boundary-comparison
    /// bug in `region_allows`: it used to scan forward *from* the candidate's own line looking
    /// for the boundary and only accept an exact-position match, which no genuine region-scoped
    /// unit ever satisfies — every "before the boundary" candidate was silently rejected and no
    /// existing test asserted on `note` units to catch it.
    #[test]
    fn region_scoped_unit_before_boundary_is_captured() {
        let (pkg, _dir) = handbook_profile();
        let profile = pkg
            .profiles
            .iter()
            .find(|profile| profile.profile.name == "handbook-en")
            .expect("profile");
        let markdown = "(1) This handbook governs day-to-day operations.\n\nChapter 3\nEquipment safety checks\n\n1. The operator shall inspect equipment before each shift.\n";
        let identity = crate::structure::derive_identity(
            "doc.handbook",
            Some("Acme Operations Handbook"),
            None,
            Some(markdown),
            std::slice::from_ref(&pkg),
        )
        .identity;
        let parsed = parse_document(
            &pkg.manifest.package.name,
            profile,
            "doc.handbook",
            &identity,
            markdown,
        )
        .expect("parse")
        .expect("units");
        assert!(
            parsed.units.iter().any(|unit| unit.label_norm == "note.1"),
            "expected note.1 before the Chapter 3 boundary, got: {:?}",
            parsed
                .units
                .iter()
                .map(|u| &u.label_norm)
                .collect::<Vec<_>>()
        );
    }

    /// A structural marker rendered as a Markdown heading (`## Chapter 3`) must match the same
    /// as its bare-text form (`Chapter 3`). Regression for the PDF-extraction finding in
    /// docs/structure_packages.md (2026-07-06): the real GDPR Regulation PDF renders most
    /// article headings as `### Article N`, and the marker regexes (authored against bare text)
    /// silently missed all of them — only the rare bare-text occurrences matched.
    #[test]
    fn heading_rendered_marker_is_captured() {
        let (pkg, _dir) = handbook_profile();
        let profile = pkg
            .profiles
            .iter()
            .find(|profile| profile.profile.name == "handbook-en")
            .expect("profile");
        let markdown = "## Chapter 3\nEquipment safety checks\n\n1. The operator shall inspect equipment before each shift.\n";
        let identity = crate::structure::derive_identity(
            "doc.handbook",
            Some("Acme Operations Handbook"),
            None,
            Some(markdown),
            std::slice::from_ref(&pkg),
        )
        .identity;
        let parsed = parse_document(
            &pkg.manifest.package.name,
            profile,
            "doc.handbook",
            &identity,
            markdown,
        )
        .expect("parse")
        .expect("units");
        assert!(
            parsed
                .units
                .iter()
                .any(|unit| unit.label_norm == "chapter.3"),
            "expected chapter.3 despite the '## ' heading prefix, got: {:?}",
            parsed
                .units
                .iter()
                .map(|u| &u.label_norm)
                .collect::<Vec<_>>()
        );
    }

    /// Two markers producing the same `label_norm` within one document (e.g. a duplicated
    /// heading left behind by an imperfectly-stripped TOC) must not collide on `unit_id` — both
    /// units are kept, the second disambiguated. Regression for the `UNIQUE constraint failed:
    /// legal_units.unit_id` finding in docs/structure_packages.md (2026-07-06), which discarded
    /// the *entire* document's parsed units when `replace_doc_units`'s single insert
    /// transaction hit the collision.
    #[test]
    fn duplicate_label_norm_gets_a_disambiguated_unit_id() {
        let (pkg, _dir) = handbook_profile();
        let profile = pkg
            .profiles
            .iter()
            .find(|profile| profile.profile.name == "handbook-en")
            .expect("profile");
        let markdown = "Chapter 5\nFirst occurrence.\n\nChapter 5\nSecond occurrence.\n";
        let identity = crate::structure::derive_identity(
            "doc.handbook",
            Some("Acme Operations Handbook"),
            None,
            Some(markdown),
            std::slice::from_ref(&pkg),
        )
        .identity;
        let parsed = parse_document(
            &pkg.manifest.package.name,
            profile,
            "doc.handbook",
            &identity,
            markdown,
        )
        .expect("parse")
        .expect("units");
        let chapter_5s: Vec<_> = parsed
            .units
            .iter()
            .filter(|unit| unit.label_norm == "chapter.5")
            .collect();
        assert_eq!(
            chapter_5s.len(),
            2,
            "both occurrences must be kept, not just the first"
        );
        assert_ne!(
            chapter_5s[0].unit_id, chapter_5s[1].unit_id,
            "duplicate label_norm must not produce duplicate unit_id"
        );
    }
}
