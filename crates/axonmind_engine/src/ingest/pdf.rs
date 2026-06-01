//! Parse PDF into `NormalizedDocument` via unpdf → Markdown → markdown::parse_text.
//! sha256 is computed over the original PDF bytes.
use super::NormalizedDocument;
use axonmind_core::AxonMindError;
use sha2::{Digest, Sha256};

pub fn parse(path: &std::path::Path, bytes: &[u8]) -> Result<NormalizedDocument, AxonMindError> {
    let sha256: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let doc = unpdf::parse_bytes(bytes).map_err(|e| AxonMindError::Ingest {
        message: format!("pdf parse: {e}"),
    })?;

    let markdown = unpdf::render::to_markdown(&doc, &unpdf::render::RenderOptions::default())
        .map_err(|e| AxonMindError::Ingest {
            message: format!("pdf render: {e}"),
        })?;

    super::markdown::parse_text(path, &markdown, sha256)
}
