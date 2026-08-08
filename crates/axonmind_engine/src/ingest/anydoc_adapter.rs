use super::NormalizedDocument;
use anydoc::Format;
use axonmind_core::AxonMindError;
use sha2::{Digest, Sha256};
use std::path::Path;

pub(crate) fn parse_docx_family(
    path: &Path,
    bytes: &[u8],
) -> Result<NormalizedDocument, AxonMindError> {
    parse_with_format(path, bytes, format_for_docx_family(path)?, false)
}

pub(crate) fn parse_spreadsheet_family(
    path: &Path,
    bytes: &[u8],
) -> Result<NormalizedDocument, AxonMindError> {
    parse_with_format(path, bytes, format_for_spreadsheet_family(path)?, true)
}

fn format_for_docx_family(path: &Path) -> Result<Format, AxonMindError> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("docx") => Ok(Format::Docx),
        Some("pptx") => Ok(Format::Pptx),
        Some(extension) => Err(family_error("DOCX/PPTX", extension)),
        None => Err(missing_extension_error("DOCX/PPTX")),
    }
}

fn format_for_spreadsheet_family(path: &Path) -> Result<Format, AxonMindError> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("csv") => Ok(Format::Csv),
        Some("xlsx") | Some("xls") | Some("xlsb") => Ok(Format::Excel),
        Some("ods") => Ok(Format::Ods),
        Some(extension) => Err(family_error("spreadsheet", extension)),
        None => Err(missing_extension_error("spreadsheet")),
    }
}

fn family_error(family: &str, extension: &str) -> AxonMindError {
    AxonMindError::Ingest {
        message: format!("{family} parser does not support .{extension}"),
    }
}

fn missing_extension_error(family: &str) -> AxonMindError {
    AxonMindError::Ingest {
        message: format!("{family} parser requires a file extension"),
    }
}

fn parse_with_format(
    path: &Path,
    bytes: &[u8],
    format: Format,
    spreadsheet: bool,
) -> Result<NormalizedDocument, AxonMindError> {
    let sha256: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let markdown =
        anydoc::to_markdown_bytes(bytes, format).map_err(|error| AxonMindError::Ingest {
            message: format!("document parse: {error}"),
        })?;
    let mut document = super::markdown::parse_text(path, &markdown, sha256)?;
    if spreadsheet {
        document.blocks.clear();
        document.title = path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(str::to_owned);
    }
    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::DocumentBlock;
    use std::path::PathBuf;

    const FIXTURES: &[(&str, &str)] = &[
        (
            "docx-text.docx",
            "6b674297884f9ed57809763c9f60ea3a849d5cc6fb28c9837c714e322eceddcf",
        ),
        (
            "pptx-title-order.pptx",
            "b05bc15dd2b901373e566e938d988fa387aa27d9f03c0fc5918097fef1652a07",
        ),
        (
            "pptx-content-table.pptx",
            "c96aa52da19f273f602040490203d9319872f512707e1a6c5a3fc53251b6d050",
        ),
        (
            "xlsx-sheet.xlsx",
            "ddfec7c1e98c7b50611b1c3ac55c0aa0d9d413135aa7afc36732338e44f4d26c",
        ),
        (
            "xls-sheet.xls",
            "1b3fc8f35f4c7ad6bb4dcf9b9f1fdf4ddf1f4c7f2f6748f33b7948204f940136",
        ),
        (
            "ods-sheet.ods",
            "e2c092eb2173b9c7ea8dd2c42e8dd7ef284cf37de40e206f3c8e43571de88454",
        ),
        (
            "xlsb-issue2.xlsb",
            "06c648de5529e022ef399a2b6ebc227040e9fdf71a0780e19e614f556bdb53d7",
        ),
        (
            "docx-truncated.docx",
            "65b78f02a11298d0b860247911e34431aedb05637749f49819a6b7a70fe1967b",
        ),
        (
            "axonmind-revenue-v1.docx",
            "d3cae440860158c3b21a374e8be4c82706dde2464c9bf6dd7fc47a8ff76a99d9",
        ),
        (
            "axonmind-cac-v2.docx",
            "8dcb9e9651f09d1eb296a6b0df9880c0f79f978a16262f78c33f8fec1c0c64b2",
        ),
        (
            "axonmind-revenue-two-sheet.xlsx",
            "cdfab1fef978d319d3ea9752ec30441fe0e94418c205b008f45c9bd154ec61e7",
        ),
    ];

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/anydoc")
            .join(name)
    }

    fn block_text(block: &DocumentBlock) -> &str {
        match block {
            DocumentBlock::Heading { text, .. }
            | DocumentBlock::Paragraph { text, .. }
            | DocumentBlock::ListItem { text, .. }
            | DocumentBlock::CodeBlock { text, .. } => text,
        }
    }

    #[test]
    fn committed_binary_fixtures_match_locked_hashes() {
        for (name, expected) in FIXTURES {
            let bytes = std::fs::read(fixture(name)).expect("fixture must be committed");
            assert_eq!(format!("{:x}", Sha256::digest(bytes)), *expected, "{name}");
        }
    }

    #[test]
    fn family_mappings_are_explicit_and_case_sensitive() {
        assert_eq!(
            format_for_docx_family(Path::new("a.docx")).unwrap(),
            Format::Docx
        );
        assert_eq!(
            format_for_docx_family(Path::new("a.pptx")).unwrap(),
            Format::Pptx
        );
        assert_eq!(
            format_for_spreadsheet_family(Path::new("a.csv")).unwrap(),
            Format::Csv
        );
        assert_eq!(
            format_for_spreadsheet_family(Path::new("a.xlsx")).unwrap(),
            Format::Excel
        );
        assert_eq!(
            format_for_spreadsheet_family(Path::new("a.xls")).unwrap(),
            Format::Excel
        );
        assert_eq!(
            format_for_spreadsheet_family(Path::new("a.xlsb")).unwrap(),
            Format::Excel
        );
        assert_eq!(
            format_for_spreadsheet_family(Path::new("a.ods")).unwrap(),
            Format::Ods
        );
        for path in ["a.xlsx", "a.DOCX", "a.odt", "extensionless"] {
            assert!(format_for_docx_family(Path::new(path)).is_err(), "{path}");
        }
        for path in ["a.docx", "a.XLSX", "a.odt", "extensionless"] {
            assert!(
                format_for_spreadsheet_family(Path::new(path)).is_err(),
                "{path}"
            );
        }
    }

    #[test]
    fn docx_and_pptx_normalize_through_markdown() {
        let path = fixture("docx-text.docx");
        let bytes = std::fs::read(&path).unwrap();
        let doc = parse_docx_family(&path, &bytes).unwrap();
        assert_eq!(doc.title.as_deref(), Some("Fixture Document"));
        assert!(
            doc.blocks
                .iter()
                .any(|block| block_text(block).contains("Plain paragraph"))
        );
        assert!(doc.blocks.iter().any(|block| matches!(block, DocumentBlock::ListItem { text, depth: 1, .. } if text == "a) Alpha sub one")));
        assert!(doc.tables.iter().any(|table| {
            table
                .rows
                .iter()
                .any(|row| row == &["Wide head", "", "End"])
        }));

        let path = fixture("pptx-title-order.pptx");
        let bytes = std::fs::read(&path).unwrap();
        let doc = parse_docx_family(&path, &bytes).unwrap();
        assert!(doc.title.is_none());
        let texts: Vec<_> = doc.blocks.iter().map(block_text).collect();
        assert_eq!(
            texts,
            [
                "Kicker before the title",
                "Title placed second",
                "Body after the title",
                "Quarterly numbers"
            ]
        );
        assert!(matches!(
            doc.blocks[1],
            DocumentBlock::Heading { level: 2, .. }
        ));

        let path = fixture("pptx-content-table.pptx");
        let bytes = std::fs::read(&path).unwrap();
        let doc = parse_docx_family(&path, &bytes).unwrap();
        for expected in [
            "Deck Title Slide",
            "Top level point",
            "Nested detail",
            "Numbers Slide",
        ] {
            assert!(doc.blocks.iter().any(|block| block_text(block) == expected));
        }
        assert!(doc.tables.iter().any(|table| {
            table.headers == ["Region", "Total"]
                && table.rows.iter().any(|row| row == &["North", "42"])
        }));
    }

    #[test]
    fn spreadsheets_are_table_only_and_keep_original_identity() {
        for name in [
            "xlsx-sheet.xlsx",
            "xls-sheet.xls",
            "ods-sheet.ods",
            "xlsb-issue2.xlsb",
        ] {
            let path = fixture(name);
            let bytes = std::fs::read(&path).unwrap();
            let doc = parse_spreadsheet_family(&path, &bytes).unwrap();
            let sha = format!("{:x}", Sha256::digest(&bytes));
            assert_eq!(doc.sha256, sha);
            assert_eq!(doc.id, format!("doc.{}", &sha[..8]));
            assert_eq!(doc.source_path.as_deref(), Some(path.as_path()));
            assert!(doc.blocks.is_empty());
            assert!(!doc.tables.is_empty(), "{name}");
            assert_eq!(
                doc.title.as_deref(),
                path.file_stem().and_then(|value| value.to_str())
            );
            let cells: Vec<&str> = doc
                .tables
                .iter()
                .flat_map(|table| table.headers.iter().chain(table.rows.iter().flatten()))
                .map(String::as_str)
                .collect();
            match name {
                "xlsx-sheet.xlsx" | "xls-sheet.xls" => {
                    assert!(cells.contains(&"0.155"));
                    assert!(cells.contains(&"0.0000004"));
                    assert!(cells.contains(&"Span two"));
                }
                "ods-sheet.ods" => {
                    assert!(cells.contains(&"15.5%"));
                    assert!(cells.contains(&"$1,234.50"));
                    assert!(cells.contains(&"Span two"));
                }
                "xlsb-issue2.xlsb" => {
                    for expected in ["1", "a", "2", "b", "3", "c"] {
                        assert!(cells.contains(&expected), "missing {expected}: {cells:?}");
                    }
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn utf16_csv_and_hard_failure_have_locked_behavior() {
        let text = "name,note,value\r\ncafé,\"left,right\",\"first\r\nsecond\"\r\n";
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        let doc = parse_spreadsheet_family(Path::new("sample.csv"), &bytes).unwrap();
        assert!(doc.blocks.is_empty());
        assert_eq!(doc.tables[0].headers, ["name", "note", "value"]);
        assert_eq!(doc.tables[0].rows[0][0], "café");
        assert_eq!(doc.tables[0].rows[0][1], "left,right");
        assert!(doc.tables[0].rows[0][2].contains("first"));
        assert!(doc.tables[0].rows[0][2].contains("second"));

        let path = fixture("docx-truncated.docx");
        let error = parse_docx_family(&path, &std::fs::read(&path).unwrap()).unwrap_err();
        assert!(
            matches!(error, AxonMindError::Ingest { message } if message.starts_with("document parse:"))
        );
    }
}
