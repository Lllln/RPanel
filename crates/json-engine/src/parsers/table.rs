use std::collections::HashMap;

use csv::ReaderBuilder;
use serde_json::{Map, Value};

use crate::parsers::scalar::parse_scalar;
use crate::pipeline::SourceFormat;

/// Parse multi-line TSV (Excel / spreadsheet / HTML table copy) or CSV into an array of objects.
pub fn try_parse(input: &str) -> Option<(SourceFormat, Value)> {
    let lines: Vec<&str> = input
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();

    if lines.len() < 2 {
        return None;
    }

    let cleaned = lines.join("\n");

    if lines[0].contains('\t')
        && let Some(value) = parse_delimited(&cleaned, b'\t')
    {
        return Some((SourceFormat::TableTsv, value));
    }

    if lines[0].contains(',')
        && !looks_like_non_csv_header(lines[0])
        && let Some(value) = parse_delimited(&cleaned, b',')
    {
        return Some((SourceFormat::TableCsv, value));
    }

    None
}

fn looks_like_non_csv_header(first_line: &str) -> bool {
    let trimmed = first_line.trim();
    trimmed.starts_with('{') || trimmed.starts_with('[') || trimmed.contains(": ")
}

fn parse_delimited(input: &str, delimiter: u8) -> Option<Value> {
    let mut reader = ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(true)
        .flexible(false)
        .from_reader(input.as_bytes());

    let raw_headers = reader.headers().ok()?.clone();
    if raw_headers.len() < 2 {
        return None;
    }

    let headers = normalize_headers(&raw_headers);
    let mut rows = Vec::new();

    for record in reader.records() {
        let record = record.ok()?;
        if record.len() != headers.len() {
            return None;
        }

        let mut obj = Map::with_capacity(headers.len());
        for (header, cell) in headers.iter().zip(record.iter()) {
            obj.insert(header.clone(), parse_scalar(cell));
        }
        rows.push(Value::Object(obj));
    }

    if rows.is_empty() {
        return None;
    }

    Some(Value::Array(rows))
}

fn normalize_headers(raw_headers: &csv::StringRecord) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut normalized = Vec::with_capacity(raw_headers.len());

    for (idx, field) in raw_headers.iter().enumerate() {
        let base = {
            let trimmed = field.trim();
            if trimmed.is_empty() {
                format!("col_{}", idx + 1)
            } else {
                trimmed.to_string()
            }
        };

        let count = seen.entry(base.clone()).or_insert(0);
        *count += 1;
        if *count == 1 {
            normalized.push(base);
        } else {
            normalized.push(format!("{base}_{count}"));
        }
    }

    normalized
}
