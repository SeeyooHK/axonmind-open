pub mod docx;
pub mod html;
pub mod image;
pub mod markdown;
pub mod pdf;
pub mod queue;
pub mod spreadsheet;
pub mod txt;

use axonmind_core::AxonMindError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Ingest job identifier. Always UUID v4 as string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(pub String);

impl From<String> for JobId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// A unit of work submitted to the IngestQueue.
#[derive(Debug, Clone)]
pub struct IngestJob {
    pub id: JobId,
    pub source: IngestSource,
    pub requested_at: DateTime<Utc>,
    pub options: IngestOptions,
}

#[derive(Debug, Clone)]
pub enum IngestSource {
    File(PathBuf),
    /// Recursive directory scan. Hidden files (dot-prefixed) are skipped.
    Directory(PathBuf),
    ManualJson(serde_json::Value),
    /// Markdown text pre-processed by soverex or Next.js. Bypasses file parsing.
    /// sha256 should be computed over the original source bytes, not the Markdown text.
    Markdown {
        text: String,
        source_path: Option<PathBuf>,
        sha256: Option<String>,
    },
    /// Phase 2: pre-parsed from seeyooEditor (browser). Bypasses Rust adapters.
    PreParsed(NormalizedDocument),
}

#[derive(Debug, Clone, Default)]
pub struct IngestOptions {
    /// If true, scan sub-directories recursively. Default: true for Directory source.
    pub recursive: bool,
    /// Reject files larger than this. Default: 50 MB.
    pub max_file_size_bytes: u64,
    /// If true, skip files already in document_cache with a matching sha256.
    pub skip_unchanged: bool,
}

/// Summary emitted in `EngineEvent::IngestCompleted`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IngestSummary {
    pub files_processed: usize,
    pub nodes_created: usize,
    pub edges_created: usize,
    pub evidence_created: usize,
    pub files_skipped: usize,
    pub errors: Vec<String>,
}

/// Returned by `ingest_file_with_content`: ingest stats, provenance handles, and the
/// document rendered back to markdown. The markdown lets a caller surface the parsed
/// content (e.g. for retrieval-augmented prompts) without re-parsing, while the same
/// call indexes the document into the graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestedDocument {
    pub summary: IngestSummary,
    /// Graph node id (`doc.<sha256[..8]>`) — stable handle for provenance and audit trails.
    pub doc_id: String,
    pub sha256: String,
    pub title: Option<String>,
    pub markdown: String,
}

// ── Normalized document model (parser-agnostic intermediate representation) ──

/// Parser-agnostic intermediate representation. Extraction rules operate on this,
/// not on comrak/calamine/docx/pdf-inspector types directly. Keeps extractors decoupled from parsers.
#[derive(Debug, Clone)]
pub struct NormalizedDocument {
    pub id: String,
    pub source_path: Option<PathBuf>,
    pub sha256: String,
    pub title: Option<String>,
    pub blocks: Vec<DocumentBlock>,
    pub tables: Vec<NormalizedTable>,
}

#[derive(Debug, Clone)]
pub enum DocumentBlock {
    Heading {
        level: u8,
        text: String,
        span: SourceSpan,
    },
    Paragraph {
        text: String,
        span: SourceSpan,
    },
    ListItem {
        text: String,
        /// `Some(n)` if this item came from an ordered list (`n.`/`n)` in the source);
        /// `None` for bullet/task-list items.
        ordinal: Option<usize>,
        span: SourceSpan,
    },
    CodeBlock {
        language: Option<String>,
        text: String,
        span: SourceSpan,
    },
}

#[derive(Debug, Clone)]
pub struct NormalizedTable {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// First byte offset of this table in the source document.
    pub span: SourceSpan,
}

/// Byte offset range in the source document for evidence `row_ref` construction.
#[derive(Debug, Clone, Copy, Default)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
}

/// Render a NormalizedDocument back to markdown for retrieval or re-display.
/// Interleaves blocks and tables by SourceSpan.start to preserve source order.
pub fn render_markdown(doc: &NormalizedDocument) -> String {
    enum Item<'a> {
        Block(&'a DocumentBlock),
        Table(&'a NormalizedTable),
    }
    let mut items: Vec<(usize, Item<'_>)> = Vec::new();
    for b in &doc.blocks {
        let start = match b {
            DocumentBlock::Heading { span, .. }
            | DocumentBlock::Paragraph { span, .. }
            | DocumentBlock::ListItem { span, .. }
            | DocumentBlock::CodeBlock { span, .. } => span.start,
        };
        items.push((start, Item::Block(b)));
    }
    for t in &doc.tables {
        items.push((t.span.start, Item::Table(t)));
    }
    items.sort_by_key(|(s, _)| *s);

    let mut out = String::new();
    for (_, item) in items {
        match item {
            Item::Block(DocumentBlock::Heading { level, text, .. }) => {
                out.push_str(&"#".repeat((*level).clamp(1, 6) as usize));
                out.push(' ');
                out.push_str(text);
                out.push_str("\n\n");
            }
            Item::Block(DocumentBlock::Paragraph { text, .. }) => {
                out.push_str(text);
                out.push_str("\n\n");
            }
            Item::Block(DocumentBlock::ListItem { text, ordinal, .. }) => {
                match ordinal {
                    Some(n) => out.push_str(&format!("{n}. ")),
                    None => out.push_str("- "),
                }
                out.push_str(text);
                out.push('\n');
            }
            Item::Block(DocumentBlock::CodeBlock { language, text, .. }) => {
                out.push_str("```");
                out.push_str(language.as_deref().unwrap_or(""));
                out.push('\n');
                out.push_str(text);
                out.push_str("\n```\n\n");
            }
            Item::Table(t) => {
                if !t.headers.is_empty() {
                    out.push_str("| ");
                    out.push_str(&t.headers.join(" | "));
                    out.push_str(" |\n| ");
                    out.push_str(&vec!["---"; t.headers.len()].join(" | "));
                    out.push_str(" |\n");
                }
                for row in &t.rows {
                    out.push_str("| ");
                    out.push_str(&row.join(" | "));
                    out.push_str(" |\n");
                }
                out.push('\n');
            }
        }
    }
    out.trim_end().to_string()
}

/// Dispatch to the correct parser adapter based on file extension.
pub fn dispatch_parse(path: &Path, bytes: &[u8]) -> Result<NormalizedDocument, AxonMindError> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("md") | Some("markdown") => markdown::parse(path, bytes),
        Some("txt") | Some("text") => txt::parse(path, bytes),
        Some("html") | Some("htm") => html::parse(path, bytes),
        Some("csv") | Some("xlsx") | Some("xls") | Some("ods") | Some("xlsb") => {
            spreadsheet::parse(path, bytes)
        }
        Some("docx") | Some("pptx") => docx::parse(path, bytes),
        Some("pdf") => pdf::parse(path, bytes),
        Some("jpg") | Some("jpeg") | Some("png") | Some("bmp") | Some("webp") | Some("tiff")
        | Some("tif") | Some("gif") => image::parse(path, bytes),
        Some(ext) => Err(AxonMindError::Ingest {
            message: format!("unsupported file type: .{ext}"),
        }),
        None => Err(AxonMindError::Ingest {
            message: "file has no extension".into(),
        }),
    }
}
