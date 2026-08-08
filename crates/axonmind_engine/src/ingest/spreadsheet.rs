//! Parse the CSV/XLSX/XLS/XLSB/ODS family through the shared anydoc adapter.
use super::NormalizedDocument;
use axonmind_core::AxonMindError;

pub fn parse(path: &std::path::Path, bytes: &[u8]) -> Result<NormalizedDocument, AxonMindError> {
    super::anydoc_adapter::parse_spreadsheet_family(path, bytes)
}
