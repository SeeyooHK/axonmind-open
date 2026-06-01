//! Parse DOCX and PPTX into `NormalizedDocument` via undoc → Markdown → markdown::parse_text.
//! sha256 is computed over the original file bytes.
use super::NormalizedDocument;
use axonmind_core::AxonMindError;
use sha2::{Digest, Sha256};

pub fn parse(path: &std::path::Path, bytes: &[u8]) -> Result<NormalizedDocument, AxonMindError> {
    let sha256: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let doc = undoc::parse_bytes(bytes).map_err(|e| AxonMindError::Ingest {
        message: format!("office parse: {e}"),
    })?;

    let markdown = undoc::render::to_markdown(&doc, &undoc::render::RenderOptions::default())
        .map_err(|e| AxonMindError::Ingest {
            message: format!("office render: {e}"),
        })?;

    super::markdown::parse_text(path, &markdown, sha256)
}
