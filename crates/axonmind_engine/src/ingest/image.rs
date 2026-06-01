use super::NormalizedDocument;
use axonmind_core::AxonMindError;

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
