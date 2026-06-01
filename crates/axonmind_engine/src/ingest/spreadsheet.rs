//! Parse XLSX/XLS/ODS/XLSB/CSV into `NormalizedDocument` using calamine.
//! Each worksheet becomes a `NormalizedTable`. No `blocks` are produced —
//! spreadsheets are pure data and go straight to the table extraction path.
use super::{NormalizedDocument, NormalizedTable, SourceSpan};
use axonmind_core::AxonMindError;
use calamine::{DataType, Reader, open_workbook_auto_from_rs};
use sha2::{Digest, Sha256};
use std::io::Cursor;

pub fn parse(path: &std::path::Path, bytes: &[u8]) -> Result<NormalizedDocument, AxonMindError> {
    let sha256: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let title = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_owned());

    let mut workbook =
        open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(|e| AxonMindError::Ingest {
            message: format!("spreadsheet open: {e}"),
        })?;

    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
    let mut tables = Vec::new();

    for name in &sheet_names {
        let range = workbook
            .worksheet_range(name)
            .map_err(|e| AxonMindError::Ingest {
                message: format!("sheet '{name}': {e}"),
            })?;

        let mut rows_iter = range.rows();

        let Some(first_row) = rows_iter.next() else {
            continue;
        };

        // First row: headers if all cells are non-numeric strings, else treat as data.
        let all_string = first_row
            .iter()
            .all(|c| !c.is_empty() && matches!(c, calamine::Data::String(_)));
        let (headers, data_rows): (Vec<String>, Vec<Vec<String>>) = if all_string {
            let hdrs = first_row.iter().map(|c| c.to_string()).collect();
            let data = rows_iter
                .map(|r| r.iter().map(|c| c.to_string()).collect())
                .collect();
            (hdrs, data)
        } else {
            // No header row — use column letters A, B, C, …
            let col_count = first_row.len();
            let hdrs = (0..col_count).map(|i| col_letter(i)).collect();
            let first: Vec<String> = first_row.iter().map(|c| c.to_string()).collect();
            let mut data = vec![first];
            data.extend(rows_iter.map(|r| r.iter().map(|c| c.to_string()).collect::<Vec<_>>()));
            (hdrs, data)
        };

        if headers.is_empty() {
            continue;
        }

        tables.push(NormalizedTable {
            headers,
            rows: data_rows,
            span: SourceSpan::default(),
        });
    }

    Ok(NormalizedDocument {
        id: format!("doc.{}", &sha256[..8]),
        source_path: Some(path.to_path_buf()),
        sha256,
        title,
        blocks: vec![],
        tables,
    })
}

fn col_letter(i: usize) -> String {
    let mut n = i;
    let mut s = String::new();
    loop {
        s.insert(0, (b'A' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    s
}
