use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use crate::pageindex::tree::{PersistTree, SectionRow};
use crate::store::{
    DocumentAliasRecord, DocumentIdentityRecord, LegalUnitRecord, LegalUnitRefRecord,
};

const PATH_SEP: &str = " \u{203a} ";

#[derive(Debug, Clone)]
pub struct LegalDocumentIndex {
    pub identity: DocumentIdentityRecord,
    pub units: Vec<LegalUnitRecord>,
    pub refs: Vec<LegalUnitRefRecord>,
    pub tree: PersistTree,
}

#[derive(Debug, Clone)]
struct DocumentProfile {
    canonical_title: String,
    language: Option<String>,
    jurisdiction: Vec<String>,
    domain: Vec<String>,
    instrument_type: Option<String>,
    corpus: Vec<String>,
    confidence: f32,
    aliases: Vec<(String, String)>,
    parser_profile: Option<String>,
}

#[derive(Debug, Clone)]
struct ParsedRef {
    target_label: String,
    target_label_norm: String,
    ref_text: String,
}

#[derive(Debug, Clone)]
struct ParsedUnit {
    parent_label_norm: Option<String>,
    unit_type: String,
    label: String,
    label_norm: String,
    article: Option<String>,
    recital: Option<String>,
    section_label: Option<String>,
    paragraph: Option<String>,
    title: Option<String>,
    ordinal: i64,
    level: i64,
    text: String,
    span_start: i64,
    span_end: i64,
    path: String,
    parser_profile: String,
    confidence: f32,
    refs: Vec<ParsedRef>,
}

#[derive(Debug, Clone)]
struct LineSpan {
    text: String,
    start: usize,
    end: usize,
}

pub fn infer_document_identity(
    doc_node_id: &str,
    raw_title: Option<&str>,
    source_path: Option<&str>,
) -> DocumentIdentityRecord {
    let source_filename = source_path
        .and_then(|p| Path::new(p).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or(raw_title.unwrap_or(doc_node_id))
        .to_string();
    let fallback_title = raw_title
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| filename_stem(&source_filename));
    let haystack = format!(
        "{} {}",
        source_filename.to_lowercase(),
        fallback_title.to_lowercase()
    );

    let mut profile = DocumentProfile {
        canonical_title: fallback_title.clone(),
        language: None,
        jurisdiction: vec![],
        domain: vec![],
        instrument_type: None,
        corpus: vec![],
        confidence: 0.35,
        aliases: vec![(fallback_title.clone(), "extracted".to_string())],
        parser_profile: None,
    };

    if haystack.contains("edpb")
        && haystack.contains("07")
        && haystack.contains("2020")
        && (haystack.contains("controller") || haystack.contains("processor"))
    {
        profile.canonical_title =
            "EDPB Guidelines 07/2020 on the concepts of controller and processor in the GDPR"
                .to_string();
        profile.language = Some("en".to_string());
        profile.jurisdiction = vec!["EU".to_string()];
        profile.domain = vec!["privacy".to_string(), "data protection".to_string()];
        profile.instrument_type = Some("guidelines".to_string());
        profile.corpus = vec!["gdpr".to_string()];
        profile.confidence = 0.97;
        profile.aliases.extend([
            ("EDPB 07/2020".to_string(), "inferred".to_string()),
            (
                "controller and processor guidelines".to_string(),
                "inferred".to_string(),
            ),
        ]);
        profile.parser_profile = Some("edpb_guidance_en".to_string());
    } else if haystack.contains("edpb")
        && (haystack.contains("9_2022")
            || haystack.contains("9/2022")
            || haystack.contains("personal data breach"))
    {
        profile.canonical_title =
            "EDPB Guidelines 9/2022 on personal data breach notification under GDPR".to_string();
        profile.language = Some("en".to_string());
        profile.jurisdiction = vec!["EU".to_string()];
        profile.domain = vec!["privacy".to_string(), "data protection".to_string()];
        profile.instrument_type = Some("guidelines".to_string());
        profile.corpus = vec!["gdpr".to_string()];
        profile.confidence = 0.97;
        profile.aliases.extend([
            ("EDPB 9/2022".to_string(), "inferred".to_string()),
            (
                "breach notification guidelines".to_string(),
                "inferred".to_string(),
            ),
        ]);
        profile.parser_profile = Some("edpb_guidance_en".to_string());
    } else if haystack.contains("2016_679")
        || haystack.contains("2016/679")
        || haystack.contains("gdpr")
    {
        profile.canonical_title = "Regulation (EU) 2016/679 GDPR".to_string();
        profile.language = Some("en".to_string());
        profile.jurisdiction = vec!["EU".to_string()];
        profile.domain = vec!["privacy".to_string(), "data protection".to_string()];
        profile.instrument_type = Some("regulation".to_string());
        profile.corpus = vec!["gdpr".to_string()];
        profile.confidence = 0.98;
        profile.aliases.extend([
            ("GDPR".to_string(), "inferred".to_string()),
            ("Regulation 2016/679".to_string(), "inferred".to_string()),
            (
                "Regulation (EU) 2016/679".to_string(),
                "inferred".to_string(),
            ),
        ]);
        profile.parser_profile = Some("eu_regulation_en".to_string());
    } else if haystack.contains("2018_1725") || haystack.contains("2018/1725") {
        profile.canonical_title = "Regulation (EU) 2018/1725".to_string();
        profile.language = Some("en".to_string());
        profile.jurisdiction = vec!["EU".to_string()];
        profile.domain = vec!["privacy".to_string(), "data protection".to_string()];
        profile.instrument_type = Some("regulation".to_string());
        profile.corpus = vec!["gdpr".to_string()];
        profile.confidence = 0.94;
        profile.aliases.extend([
            ("Regulation 2018/1725".to_string(), "inferred".to_string()),
            (
                "Regulation (EU) 2018/1725".to_string(),
                "inferred".to_string(),
            ),
        ]);
        profile.parser_profile = Some("eu_regulation_en".to_string());
    } else if haystack.contains("eprivacy") {
        profile.language = Some("en".to_string());
        profile.jurisdiction = vec!["EU".to_string()];
        profile.domain = vec!["privacy".to_string(), "data protection".to_string()];
        profile.instrument_type = Some("directive".to_string());
        profile.corpus = vec!["gdpr".to_string()];
        profile.confidence = 0.85;
        profile
            .aliases
            .push(("ePrivacy Directive".to_string(), "inferred".to_string()));
        profile.parser_profile = Some("eu_regulation_en".to_string());
    } else if haystack.contains("101_2018")
        || haystack.contains("101/2018")
        || haystack.contains("d.lgs")
        || haystack.contains("decreto legislativo")
    {
        profile.canonical_title = "Decreto legislativo 10 agosto 2018, n. 101".to_string();
        profile.language = Some("it".to_string());
        profile.jurisdiction = vec!["IT".to_string()];
        profile.domain = vec!["privacy".to_string(), "data protection".to_string()];
        profile.instrument_type = Some("statute".to_string());
        profile.corpus = vec!["gdpr".to_string()];
        profile.confidence = 0.96;
        profile.aliases.extend([
            ("d.lgs. 101/2018".to_string(), "inferred".to_string()),
            (
                "Decreto legislativo 101/2018".to_string(),
                "inferred".to_string(),
            ),
        ]);
        profile.parser_profile = Some("it_statute".to_string());
    }

    let mut aliases: Vec<DocumentAliasRecord> = Vec::new();
    aliases.push(DocumentAliasRecord {
        alias: source_filename.clone(),
        alias_norm: normalize_alias(&source_filename),
        source: "filename".to_string(),
    });
    for (alias, source) in profile.aliases {
        let alias_norm = normalize_alias(&alias);
        if !alias_norm.is_empty()
            && !aliases
                .iter()
                .any(|existing| existing.alias_norm == alias_norm)
        {
            aliases.push(DocumentAliasRecord {
                alias,
                alias_norm,
                source,
            });
        }
    }

    DocumentIdentityRecord {
        doc_node_id: doc_node_id.to_string(),
        source_filename,
        source_path: source_path.map(str::to_string),
        raw_title: raw_title.map(str::to_string),
        canonical_title: profile.canonical_title,
        language: profile.language,
        jurisdiction: profile.jurisdiction,
        domain: profile.domain,
        instrument_type: profile.instrument_type,
        corpus: profile.corpus,
        confidence: profile.confidence,
        reviewed_at: None,
        updated_at: chrono::Utc::now().timestamp(),
        pinned_profile: profile.parser_profile.clone(),
        aliases,
    }
}

pub fn build_legal_index(
    doc_node_id: &str,
    raw_title: Option<&str>,
    source_path: Option<&str>,
    markdown: &str,
) -> Option<LegalDocumentIndex> {
    let identity = infer_document_identity(doc_node_id, raw_title, source_path);
    let parser_profile = parser_profile_for_identity(&identity)?;

    let parsed_units = match parser_profile.as_str() {
        "eu_regulation_en" | "it_statute" => {
            parse_regulation_like(markdown, &identity, &parser_profile)
        }
        "edpb_guidance_en" => parse_guidance(markdown, &identity, &parser_profile),
        _ => vec![],
    };

    if parsed_units.is_empty() {
        return None;
    }

    let mut label_to_id = std::collections::HashMap::new();
    let mut units = Vec::with_capacity(parsed_units.len());
    let mut sections = Vec::with_capacity(parsed_units.len());

    for (idx, parsed) in parsed_units.iter().enumerate() {
        let section_id = format!("{doc_node_id}#{:04}", idx + 1);
        let unit_id = format!("{doc_node_id}:{}", parsed.label_norm);
        label_to_id.insert(parsed.label_norm.clone(), unit_id.clone());
        units.push(LegalUnitRecord {
            unit_id: unit_id.clone(),
            doc_node_id: doc_node_id.to_string(),
            parent_unit_id: None,
            section_id: section_id.clone(),
            unit_type: parsed.unit_type.clone(),
            label: parsed.label.clone(),
            label_norm: parsed.label_norm.clone(),
            article: parsed.article.clone(),
            recital: parsed.recital.clone(),
            section_label: parsed.section_label.clone(),
            paragraph: parsed.paragraph.clone(),
            title: parsed.title.clone(),
            ordinal: parsed.ordinal,
            level: parsed.level,
            text: parsed.text.clone(),
            span_start: parsed.span_start,
            span_end: parsed.span_end,
            page_start: None,
            page_end: None,
            path: parsed.path.clone(),
            parser_profile: parsed.parser_profile.clone(),
            confidence: parsed.confidence,
        });
        sections.push(SectionRow {
            section_id,
            doc_node_id: doc_node_id.to_string(),
            parent_section_id: None,
            ordinal: parsed.ordinal,
            level: parsed.level,
            title: display_title(parsed),
            path: parsed.path.clone(),
            summary: None,
            text: Some(parsed.text.clone()),
            span_start: parsed.span_start,
            span_end: parsed.span_end,
        });
    }

    for (idx, parsed) in parsed_units.iter().enumerate() {
        if let Some(parent_label_norm) = parsed.parent_label_norm.as_ref() {
            if let Some(parent_unit_id) = label_to_id.get(parent_label_norm) {
                units[idx].parent_unit_id = Some(parent_unit_id.clone());
                if let Some(parent_idx) = units
                    .iter()
                    .position(|unit| &unit.unit_id == parent_unit_id)
                {
                    sections[idx].parent_section_id = Some(sections[parent_idx].section_id.clone());
                }
            }
        }
    }

    let mut refs = Vec::new();
    for parsed in &parsed_units {
        let from_unit_id = match label_to_id.get(&parsed.label_norm) {
            Some(id) => id.clone(),
            None => continue,
        };
        for reference in &parsed.refs {
            refs.push(LegalUnitRefRecord {
                from_unit_id: from_unit_id.clone(),
                to_doc_node_id: None,
                target_label: reference.target_label.clone(),
                target_label_norm: reference.target_label_norm.clone(),
                ref_text: reference.ref_text.clone(),
            });
        }
    }

    Some(LegalDocumentIndex {
        identity: identity.clone(),
        units,
        refs,
        tree: PersistTree {
            doc_node_id: doc_node_id.to_string(),
            sha256: String::new(),
            title: identity.canonical_title.clone(),
            doc_summary: None,
            sections,
        },
    })
}

fn parser_profile_for_identity(identity: &DocumentIdentityRecord) -> Option<String> {
    let title = identity.canonical_title.to_lowercase();
    if title.contains("guidelines 07/2020")
        || title.contains("guidelines 9/2022")
        || title.contains("edpb guidelines")
        || identity
            .instrument_type
            .as_deref()
            .is_some_and(|kind| kind == "guidelines")
    {
        Some("edpb_guidance_en".to_string())
    } else if identity.language.as_deref() == Some("it")
        && identity
            .instrument_type
            .as_deref()
            .is_some_and(|kind| kind == "statute")
    {
        Some("it_statute".to_string())
    } else if identity
        .instrument_type
        .as_deref()
        .is_some_and(|kind| kind == "regulation" || kind == "directive")
    {
        Some("eu_regulation_en".to_string())
    } else {
        None
    }
}

fn parse_regulation_like(
    markdown: &str,
    identity: &DocumentIdentityRecord,
    parser_profile: &str,
) -> Vec<ParsedUnit> {
    let lines = split_lines(markdown);
    let mut markers = Vec::new();
    let mut in_articles = false;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = strip_heading_prefix(line.text.trim());
        if trimmed.is_empty() {
            continue;
        }
        if let Some(captures) = article_re().captures(trimmed) {
            in_articles = true;
            let article = captures
                .get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let inline_title = captures
                .get(3)
                .map(|m| m.as_str().trim().to_string())
                .filter(|s| !s.is_empty());
            markers.push((
                idx,
                "article".to_string(),
                format!("Article {article}"),
                format!("art.{}", article.to_lowercase()),
                Some(article),
                None,
                None,
                inline_title.or_else(|| next_heading_title(&lines, idx, parser_profile)),
            ));
            continue;
        }
        if !in_articles {
            if let Some(captures) = recital_re().captures(trimmed) {
                let recital = captures
                    .get(1)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                markers.push((
                    idx,
                    "recital".to_string(),
                    format!("Recital {recital}"),
                    format!("recital.{recital}"),
                    None,
                    Some(recital),
                    None,
                    None,
                ));
                continue;
            }
        }
        if let Some(captures) = annex_re().captures(trimmed) {
            let annex = captures
                .get(0)
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();
            markers.push((
                idx,
                "annex".to_string(),
                annex.clone(),
                normalize_label_norm("annex", &annex, None),
                None,
                None,
                Some(annex.clone()),
                Some(annex),
            ));
        }
    }

    build_units_from_markers(markdown, &lines, markers, identity, parser_profile, true)
}

fn parse_guidance(
    markdown: &str,
    identity: &DocumentIdentityRecord,
    parser_profile: &str,
) -> Vec<ParsedUnit> {
    let lines = split_lines(markdown);
    let mut markers = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = strip_heading_prefix(line.text.trim());
        if trimmed.is_empty() || is_toc_line(trimmed) {
            continue;
        }
        if let Some(captures) = section_re().captures(trimmed) {
            let section = captures
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let title = captures
                .get(2)
                .map(|m| m.as_str().trim().to_string())
                .filter(|s| !s.is_empty());
            markers.push((
                idx,
                "section".to_string(),
                section.clone(),
                format!("section.{section}"),
                None,
                None,
                Some(section),
                title,
            ));
        }
    }

    build_units_from_markers(markdown, &lines, markers, identity, parser_profile, false)
}

#[allow(clippy::type_complexity)]
fn build_units_from_markers(
    markdown: &str,
    lines: &[LineSpan],
    markers: Vec<(
        usize,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )>,
    identity: &DocumentIdentityRecord,
    parser_profile: &str,
    parse_article_paragraphs: bool,
) -> Vec<ParsedUnit> {
    if markers.is_empty() {
        return vec![];
    }

    let mut units = Vec::new();

    for (ordinal, marker) in markers.iter().enumerate() {
        let start_idx = marker.0;
        let next_start = markers
            .get(ordinal + 1)
            .map(|next| lines[next.0].start)
            .unwrap_or(markdown.len());
        let start = lines[start_idx].start;
        let end = next_start.max(start);
        let text = markdown[start..end].trim().to_string();
        if text.is_empty() {
            continue;
        }

        let path = if marker.1 == "paragraph" {
            format!("{}{}{}", identity.canonical_title, PATH_SEP, marker.2)
        } else if let Some(title) = marker.7.as_ref() {
            format!(
                "{}{}{}{}{}",
                identity.canonical_title, PATH_SEP, marker.2, PATH_SEP, title
            )
        } else {
            format!("{}{}{}", identity.canonical_title, PATH_SEP, marker.2)
        };

        let mut unit = ParsedUnit {
            parent_label_norm: None,
            unit_type: marker.1.clone(),
            label: marker.2.clone(),
            label_norm: marker.3.clone(),
            article: marker.4.clone(),
            recital: marker.5.clone(),
            section_label: marker.6.clone(),
            paragraph: None,
            title: marker.7.clone(),
            ordinal: ordinal as i64,
            level: if marker.1 == "section" {
                section_level(&marker.2)
            } else {
                1
            },
            text: text.clone(),
            span_start: start as i64,
            span_end: end as i64,
            path,
            parser_profile: parser_profile.to_string(),
            confidence: 0.98,
            refs: vec![],
        };
        unit.refs = extract_refs(&unit.text, &unit.label_norm);
        units.push(unit.clone());

        if parse_article_paragraphs && unit.unit_type == "article" {
            units.extend(parse_article_children(
                markdown,
                lines,
                marker,
                ordinal as i64,
                identity,
                parser_profile,
            ));
        }
    }

    units
}

#[allow(clippy::type_complexity)]
fn parse_article_children(
    markdown: &str,
    lines: &[LineSpan],
    marker: &(
        usize,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
    article_ordinal: i64,
    identity: &DocumentIdentityRecord,
    parser_profile: &str,
) -> Vec<ParsedUnit> {
    let mut paragraph_markers = Vec::new();
    let article_line_idx = marker.0;
    let article_start = lines[article_line_idx].start;
    let article_end = article_end(markdown, article_start, lines, article_line_idx);
    let article_number = marker.4.clone().unwrap_or_default();

    for (idx, line) in lines.iter().enumerate().skip(article_line_idx + 1) {
        if line.start >= article_end {
            break;
        }
        let trimmed = strip_heading_prefix(line.text.trim());
        if trimmed.is_empty() {
            continue;
        }
        if let Some(captures) = paragraph_re().captures(trimmed) {
            let paragraph = captures
                .get(1)
                .or_else(|| captures.get(2))
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            paragraph_markers.push((idx, paragraph));
        }
    }

    let mut children = Vec::new();
    for (ordinal, (idx, paragraph)) in paragraph_markers.iter().enumerate() {
        let start = lines[*idx].start;
        let end = paragraph_markers
            .get(ordinal + 1)
            .map(|next| lines[next.0].start)
            .unwrap_or(article_end);
        let text = markdown[start..end].trim().to_string();
        if text.is_empty() {
            continue;
        }
        let label = format!("Article {}({})", article_number, paragraph);
        let label_norm = format!("art.{}.p{}", article_number.to_lowercase(), paragraph);
        let path = format!(
            "{}{}Article {}{}Paragraph {}",
            identity.canonical_title, PATH_SEP, article_number, PATH_SEP, paragraph
        );
        let mut unit = ParsedUnit {
            parent_label_norm: Some(marker.3.clone()),
            unit_type: "paragraph".to_string(),
            label,
            label_norm: label_norm.clone(),
            article: Some(article_number.clone()),
            recital: None,
            section_label: None,
            paragraph: Some(paragraph.clone()),
            title: marker.7.clone(),
            ordinal: article_ordinal * 100 + ordinal as i64 + 1,
            level: 2,
            text,
            span_start: start as i64,
            span_end: end as i64,
            path,
            parser_profile: parser_profile.to_string(),
            confidence: 0.95,
            refs: vec![],
        };
        unit.refs = extract_refs(&unit.text, &unit.label_norm);
        children.push(unit);
    }

    children
}

fn article_end(
    markdown: &str,
    article_start: usize,
    lines: &[LineSpan],
    article_idx: usize,
) -> usize {
    for line in lines.iter().skip(article_idx + 1) {
        let trimmed = strip_heading_prefix(line.text.trim());
        if article_re().is_match(trimmed) || annex_re().is_match(trimmed) {
            return line.start;
        }
    }
    markdown.len().max(article_start)
}

fn extract_refs(text: &str, current_label_norm: &str) -> Vec<ParsedRef> {
    let mut refs = Vec::new();
    for captures in article_ref_re().captures_iter(text) {
        let article = captures.get(1).map(|m| m.as_str()).unwrap_or_default();
        let paragraph = captures.get(2).map(|m| m.as_str().to_string());
        let label = if let Some(paragraph) = paragraph.as_deref() {
            format!("Article {article}({paragraph})")
        } else {
            format!("Article {article}")
        };
        let label_norm = if let Some(paragraph) = paragraph.as_deref() {
            format!("art.{}.p{}", article.to_lowercase(), paragraph)
        } else {
            format!("art.{}", article.to_lowercase())
        };
        if label_norm != current_label_norm {
            refs.push(ParsedRef {
                target_label: label,
                target_label_norm: label_norm,
                ref_text: captures
                    .get(0)
                    .map(|m| m.as_str())
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    for captures in recital_ref_re().captures_iter(text) {
        let recital = captures.get(1).map(|m| m.as_str()).unwrap_or_default();
        let label_norm = format!("recital.{recital}");
        if label_norm != current_label_norm {
            refs.push(ParsedRef {
                target_label: format!("Recital {recital}"),
                target_label_norm: label_norm,
                ref_text: captures
                    .get(0)
                    .map(|m| m.as_str())
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    for captures in section_ref_re().captures_iter(text) {
        let section = captures.get(1).map(|m| m.as_str()).unwrap_or_default();
        let label_norm = format!("section.{section}");
        if label_norm != current_label_norm {
            refs.push(ParsedRef {
                target_label: section.to_string(),
                target_label_norm: label_norm,
                ref_text: captures
                    .get(0)
                    .map(|m| m.as_str())
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    refs
}

fn split_lines(markdown: &str) -> Vec<LineSpan> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    for line in markdown.split_inclusive('\n') {
        let end = offset + line.len();
        lines.push(LineSpan {
            text: line.trim_end_matches(['\r', '\n']).to_string(),
            start: offset,
            end,
        });
        offset = end;
    }
    if !markdown.ends_with('\n') && !markdown.is_empty() {
        if let Some(last) = lines.last_mut() {
            last.end = markdown.len();
        }
    }
    lines
}

fn next_heading_title(
    lines: &[LineSpan],
    start_idx: usize,
    parser_profile: &str,
) -> Option<String> {
    for line in lines.iter().skip(start_idx + 1) {
        let trimmed = strip_heading_prefix(line.text.trim());
        if trimmed.is_empty() {
            continue;
        }
        if parser_profile == "edpb_guidance_en" && is_toc_line(trimmed) {
            continue;
        }
        if article_re().is_match(trimmed)
            || recital_re().is_match(trimmed)
            || annex_re().is_match(trimmed)
            || section_re().is_match(trimmed)
            || paragraph_re().is_match(trimmed)
        {
            return None;
        }
        return Some(trimmed.to_string());
    }
    None
}

fn section_level(label: &str) -> i64 {
    label.split('.').count() as i64
}

fn display_title(unit: &ParsedUnit) -> String {
    match unit.title.as_deref() {
        Some(title) if !title.is_empty() => format!("{} - {}", unit.label, title),
        _ => unit.label.clone(),
    }
}

fn filename_stem(filename: &str) -> String {
    Path::new(filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(filename)
        .replace('_', " ")
}

fn normalize_alias(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_label_norm(kind: &str, label: &str, paragraph: Option<&str>) -> String {
    let stem = normalize_alias(label).replace(' ', ".");
    match paragraph {
        Some(paragraph) => format!("{kind}.{stem}.p{paragraph}"),
        None => format!("{kind}.{stem}"),
    }
}

fn strip_heading_prefix(line: &str) -> &str {
    line.trim_start_matches('#').trim_start()
}

fn is_toc_line(line: &str) -> bool {
    let normalized = line.trim();
    normalized.eq_ignore_ascii_case("contents")
        || normalized.eq_ignore_ascii_case("table of contents")
        || normalized.eq_ignore_ascii_case("sommario")
        || toc_section_re().is_match(normalized)
}

fn article_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^(Article|Art\.|Articolo)\s+([0-9]+[A-Za-z]?)\b(?:\s*[-–:]\s*(.+))?$")
            .expect("article regex")
    })
}

fn recital_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\(([0-9]+)\)\s+").expect("recital regex"))
}

fn annex_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^(ANNEX|Annex|Allegato)\b.*$").expect("annex regex"))
}

fn section_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^([0-9]+(?:\.[0-9]+)+)\s+(.+)$").expect("section regex"))
}

fn toc_section_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[0-9]+(?:\.[0-9]+)+\s+.+\s+[0-9]+$").expect("toc regex"))
}

fn paragraph_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(?:([0-9]+)\.\s+|\(([0-9]+)\)\s+)").expect("paragraph regex"))
}

fn article_ref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(?:Article|Art\.|Articolo)\s+([0-9]+[A-Za-z]?)(?:\(([0-9]+)\))?")
            .expect("article ref regex")
    })
}

fn recital_ref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\brecital\s+([0-9]+)\b").expect("recital ref regex"))
}

fn section_ref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bsection\s+([0-9]+(?:\.[0-9]+)+)\b").expect("section ref regex")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_article_and_paragraph_locators() {
        let markdown = "# Article 33\n\nNotification of a personal data breach\n\n1. In the case of a personal data breach, the controller shall without undue delay.\n\n2. The processor shall notify the controller.\n";
        let index = build_legal_index(
            "doc.gdpr",
            Some("GDPR"),
            Some("legal_gdpr_Regulation_2016_679__GDPR_.pdf"),
            markdown,
        )
        .expect("legal index");
        assert!(index.units.iter().any(|unit| unit.label_norm == "art.33"));
        assert!(
            index
                .units
                .iter()
                .any(|unit| unit.label_norm == "art.33.p1")
        );
    }

    #[test]
    fn parses_guidance_sections_and_skips_toc_lines() {
        let markdown = "Table of Contents\n1.3.6 Processor obligations 14\n\n1.3.6 Processor obligations\nThe processor must assist the controller.\n";
        let index = build_legal_index(
            "doc.edpb",
            Some("EDPB Guidelines 07/2020"),
            Some("legal_gdpr_EDPB_guidelines_202007_controllerprocessor_final_en.pdf"),
            markdown,
        )
        .expect("legal index");
        assert_eq!(
            index
                .units
                .iter()
                .filter(|unit| unit.unit_type == "section")
                .count(),
            1
        );
        assert_eq!(index.units[0].label_norm, "section.1.3.6");
    }
}
