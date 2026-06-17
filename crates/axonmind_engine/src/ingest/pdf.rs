//! Parse PDF into `NormalizedDocument` via pdf-inspector → Markdown → markdown::parse_text.
//! pdf-inspector (pure Rust) decodes Type0/CID `Identity-H` fonts through their `/ToUnicode`
//! CMap, yielding clean UTF-8 — unlike unpdf, which emitted NUL-interleaved code units — and
//! emits Markdown with heading/list/table structure that `parse_text` (comrak) re-parses.
//! sha256 is computed over the original PDF bytes.
use super::NormalizedDocument;
use axonmind_core::AxonMindError;
use sha2::{Digest, Sha256};

pub fn parse(path: &std::path::Path, bytes: &[u8]) -> Result<NormalizedDocument, AxonMindError> {
    let sha256: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let result = pdf_inspector::process_pdf_mem(bytes).map_err(|e| AxonMindError::Ingest {
        message: format!("pdf parse: {e}"),
    })?;
    let markdown = result.markdown.unwrap_or_default();

    // Fail loud rather than index a blank/garbled document when no text was recovered
    // (matches the image-ingest convention). pdf-inspector flags image-based pages and
    // broken font encodings; surface that so the caller can route to OCR.
    if markdown.trim().is_empty() {
        let reason = if !result.pages_needing_ocr.is_empty() {
            format!(
                "{} of {} page(s) are image-based and need OCR",
                result.pages_needing_ocr.len(),
                result.page_count
            )
        } else if result.has_encoding_issues {
            "broken font encoding detected".to_string()
        } else {
            "no extractable text".to_string()
        };
        return Err(AxonMindError::Ingest {
            message: format!(
                "pdf yielded no extractable text ({reason}); this PDF likely needs OCR — \
                 export the page(s) as images to ingest via the image OCR path"
            ),
        });
    }

    // Text was recovered, but some pages still have broken encodings / need OCR.
    // Index what we have, but surface the partial loss instead of hiding it.
    if result.has_encoding_issues {
        tracing::warn!(
            "pdf {}: broken font encoding on some pages ({} of {} flagged for OCR); \
             indexing recovered text only, some content may be missing or garbled",
            path.display(),
            result.pages_needing_ocr.len(),
            result.page_count
        );
    }

    super::markdown::parse_text(path, &markdown, sha256)
}
