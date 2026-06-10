use super::NormalizedDocument;
use axonmind_core::AxonMindError;

#[cfg(feature = "llm")]
fn mime_for_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("tiff") | Some("tif") => "image/tiff",
        Some("gif") => "image/gif",
        _ => "image/png",
    }
}

/// LLM-first image ingest: ask the active provider to transcribe, fall back to Tesseract.
/// Called from `ingest_file` (async context) when an image extension is detected.
#[cfg(feature = "llm")]
pub async fn parse_with_llm(
    path: &std::path::Path,
    bytes: &[u8],
    sha256: String,
    llm: &dyn crate::extract::llm::LlmProvider,
) -> Result<NormalizedDocument, AxonMindError> {
    let mime = mime_for_path(path);
    match llm.transcribe_image(bytes, mime).await {
        Ok(text) if !text.trim().is_empty() => super::markdown::parse_text(path, &text, sha256),
        Ok(_) => parse(path, bytes).map_err(|_| AxonMindError::Ingest {
            message: "image transcription returned empty content".into(),
        }),
        // OCR fallback: if OCR feature is absent or Tesseract unavailable, surface the LLM error
        // (more actionable than "rebuild with --features ocr").
        Err(llm_err) => parse(path, bytes).map_err(|_| llm_err),
    }
}

pub fn parse(path: &std::path::Path, bytes: &[u8]) -> Result<NormalizedDocument, AxonMindError> {
    parse_inner(path, bytes)
}

#[cfg(feature = "ocr")]
fn parse_inner(path: &std::path::Path, bytes: &[u8]) -> Result<NormalizedDocument, AxonMindError> {
    use sha2::{Digest, Sha256};
    let sha256: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let text = run_tesseract(bytes)?;
    super::markdown::parse_text(path, &text, sha256)
}

#[cfg(not(feature = "ocr"))]
fn parse_inner(
    _path: &std::path::Path,
    _bytes: &[u8],
) -> Result<NormalizedDocument, AxonMindError> {
    Err(AxonMindError::Ingest {
        message: "image OCR requires the `ocr` feature — \
                  rebuild with `--features ocr` and ensure Tesseract is installed \
                  (e.g. `brew install tesseract` on macOS)"
            .into(),
    })
}

#[cfg(feature = "ocr")]
fn run_tesseract(bytes: &[u8]) -> Result<String, AxonMindError> {
    tesseract::Tesseract::new(None, Some("eng"))
        .map_err(|e| AxonMindError::Ingest {
            message: format!("Tesseract init: {e}"),
        })?
        .set_image_from_mem(bytes)
        .map_err(|e| AxonMindError::Ingest {
            message: format!("Tesseract set_image: {e}"),
        })?
        .get_text()
        .map_err(|e| AxonMindError::Ingest {
            message: format!("Tesseract OCR: {e}"),
        })
}
