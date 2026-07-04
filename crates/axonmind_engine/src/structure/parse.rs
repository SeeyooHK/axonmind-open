use std::collections::BTreeMap;

use axonmind_core::AxonMindError;

use crate::pageindex::tree::{PersistTree, SectionRow};
use crate::store::{DocumentIdentityRecord, LegalUnitRefRecord};
use crate::structure::model::{CaptureSelector, LevelSpec, ProfileDefinition, StructureUnit};

const PATH_SEP: &str = " \u{203a} ";

#[derive(Debug, Clone)]
pub struct ParsedDocumentIndex {
    pub units: Vec<ParsedUnit>,
    pub refs: Vec<LegalUnitRefRecord>,
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
    refs: &mut Vec<LegalUnitRefRecord>,
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
        let unit_id = format!("{doc_node_id}:{label_norm}");
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
        )?;
    }
    Ok(())
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
    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.text.trim();
        if trimmed.is_empty() {
            continue;
        }
        for candidate in candidates {
            if !region_allows(profile, candidate, parent_marker, idx, lines) {
                continue;
            }
            let Ok(regex) = regex::Regex::new(&candidate.marker) else {
                continue;
            };
            let Some(captures) = regex.captures(trimmed) else {
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

fn region_allows(
    profile: &ProfileDefinition,
    unit: &StructureUnit,
    parent_marker: Option<&MarkerMatch>,
    line_idx: usize,
    lines: &[LineSpan],
) -> bool {
    if parent_marker.is_some() {
        return true;
    }
    let Some(region_name) = unit.region.as_deref() else {
        return true;
    };
    let Some(region) = profile
        .region
        .iter()
        .find(|value| value.name == region_name)
    else {
        return false;
    };
    if region.from != "start" {
        return false;
    }
    let Some(until_kind) = region.until.strip_prefix("first:") else {
        return false;
    };
    for (idx, line) in lines.iter().enumerate() {
        if idx < line_idx {
            continue;
        }
        if profile
            .units
            .iter()
            .filter(|candidate| candidate.parent.is_none())
            .any(|candidate| {
                candidate.kind == until_kind
                    && regex::Regex::new(&candidate.marker)
                        .ok()
                        .is_some_and(|regex| regex.is_match(line.text.trim()))
            })
        {
            return idx == line_idx;
        }
    }
    true
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

fn extract_refs(profile: &ProfileDefinition, unit: &ParsedUnit) -> Vec<LegalUnitRefRecord> {
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
            refs.push(LegalUnitRefRecord {
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
    fn parses_regulation_fixture_article_and_paragraph() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../soverex-open/docs/legal_agent/structure");
        let pkg = crate::structure::model::StructurePackage::from_dir(&dir).expect("package");
        let profile = pkg
            .profiles
            .iter()
            .find(|profile| profile.profile.name == "eu-regulation-en")
            .expect("profile");
        let markdown =
            std::fs::read_to_string(dir.join("fixtures/eu-regulation-en.md")).expect("fixture");
        let identity = crate::structure::derive_identity(
            "doc.gdpr",
            Some("GDPR"),
            Some("legal_gdpr_Regulation_2016_679__GDPR_.pdf"),
            Some(&markdown),
            std::slice::from_ref(&pkg),
        )
        .identity;
        let parsed = parse_document(
            &pkg.manifest.package.name,
            profile,
            "doc.gdpr",
            &identity,
            &markdown,
        )
        .expect("parse")
        .expect("units");
        assert!(parsed.units.iter().any(|unit| unit.label_norm == "art.33"));
        assert!(
            parsed
                .units
                .iter()
                .any(|unit| unit.label_norm == "art.33.p1")
        );
    }

    #[test]
    fn parses_guidance_fixture_and_strips_toc() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../soverex-open/docs/legal_agent/structure");
        let pkg = crate::structure::model::StructurePackage::from_dir(&dir).expect("package");
        let profile = pkg
            .profiles
            .iter()
            .find(|profile| profile.profile.name == "edpb-guidance-en")
            .expect("profile");
        let markdown =
            std::fs::read_to_string(dir.join("fixtures/edpb-guidance-en.md")).expect("fixture");
        let identity = crate::structure::derive_identity(
            "doc.edpb",
            Some("EDPB Guidelines 07/2020"),
            Some("legal_gdpr_EDPB_guidelines_202007_controllerprocessor_final_en.pdf"),
            Some(&markdown),
            std::slice::from_ref(&pkg),
        )
        .identity;
        let parsed = parse_document(
            &pkg.manifest.package.name,
            profile,
            "doc.edpb",
            &identity,
            &markdown,
        )
        .expect("parse")
        .expect("units");
        assert_eq!(
            parsed
                .units
                .iter()
                .filter(|unit| unit.unit_kind == "section")
                .count(),
            1
        );
        assert_eq!(parsed.units[0].label_norm, "section.1.3.6");
    }
}
