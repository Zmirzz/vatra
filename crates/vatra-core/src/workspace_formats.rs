//! Bounded document formats. Content is stored as data; HTML is never executed here.
use crate::{DocumentFormat, WorkspaceError};

pub const MAX_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;

pub fn validate_document(format: DocumentFormat, body: &str) -> Result<(), WorkspaceError> {
    if body.len() > MAX_DOCUMENT_BYTES {
        return Err(WorkspaceError::Invalid("The document is larger than 2 MiB.".into()));
    }
    if format == DocumentFormat::Csv {
        parse_workspace_csv(body)?;
    }
    Ok(())
}

pub fn parse_workspace_csv(body: &str) -> Result<Vec<Vec<String>>, WorkspaceError> {
    if body.len() > MAX_DOCUMENT_BYTES {
        return Err(WorkspaceError::Invalid("The CSV is larger than 2 MiB.".into()));
    }
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    // csv deliberately accepts some malformed quote shapes. Reject them before parsing instead of silently repairing data.
    #[derive(Clone, Copy)]
    enum Quote {
        Start,
        Plain,
        Quoted,
        Closed,
    }
    let mut state = Quote::Start;
    for c in body.chars() {
        state = match (state, c) {
            (Quote::Start, '"') => Quote::Quoted,
            (Quote::Plain, '"') => return Err(WorkspaceError::Invalid("CSV quotes must wrap the whole field.".into())),
            (Quote::Quoted, '"') => Quote::Closed,
            (Quote::Quoted, _) => Quote::Quoted,
            (Quote::Closed, '"') => Quote::Quoted,
            (Quote::Closed, ',' | '\n' | '\r') => Quote::Start,
            (Quote::Closed, _) => {
                return Err(WorkspaceError::Invalid(
                    "Only a separator can follow a quoted CSV field.".into(),
                ));
            }
            (_, ',' | '\n' | '\r') => Quote::Start,
            _ => Quote::Plain,
        };
    }
    if matches!(state, Quote::Quoted) {
        return Err(WorkspaceError::Invalid("The CSV has an unclosed quote.".into()));
    }
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(false)
        .from_reader(body.as_bytes());
    let mut rows = Vec::new();
    let mut cells = 0usize;
    for record in reader.records() {
        let record =
            record.map_err(|_| WorkspaceError::Invalid("The CSV rows must have the same number of fields.".into()))?;
        cells += record.len();
        if rows.len() >= 10_000 || record.len() > 100 || cells > 50_000 {
            return Err(WorkspaceError::Invalid(
                "Use at most 10,000 rows, 100 columns and 50,000 CSV cells.".into(),
            ));
        }
        rows.push(record.iter().map(String::from).collect());
    }
    Ok(rows)
}

pub fn safe_csv_export(body: &str) -> Result<String, WorkspaceError> {
    let rows = parse_workspace_csv(body)?;
    let mut writer = csv::WriterBuilder::new().has_headers(false).from_writer(Vec::new());
    for row in rows {
        let safe: Vec<String> = row
            .into_iter()
            .map(|cell| {
                if cell.trim_start().starts_with(['=', '+', '-', '@']) || cell.starts_with(['\t', '\r', '\n']) {
                    format!("'{cell}")
                } else {
                    cell
                }
            })
            .collect();
        writer
            .write_record(safe)
            .map_err(|_| WorkspaceError::Invalid("The CSV couldn't be exported.".into()))?;
    }
    let bytes = writer
        .into_inner()
        .map_err(|_| WorkspaceError::Invalid("The CSV couldn't be exported.".into()))?;
    String::from_utf8(bytes).map_err(|_| WorkspaceError::Invalid("The CSV text is not UTF-8.".into()))
}
