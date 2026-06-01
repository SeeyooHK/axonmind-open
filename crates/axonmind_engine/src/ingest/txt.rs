use super::NormalizedDocument;
use axonmind_core::AxonMindError;
use sha2::{Digest, Sha256};

pub fn parse(path: &std::path::Path, bytes: &[u8]) -> Result<NormalizedDocument, AxonMindError> {
    let text = std::str::from_utf8(bytes).map_err(|e| AxonMindError::Ingest {
        message: format!("invalid UTF-8: {e}"),
    })?;
    let sha256: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    // Plain text with blank-line paragraph breaks parses correctly through the Markdown adapter.
    super::markdown::parse_text(path, text, sha256)
}
