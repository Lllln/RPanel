use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::parsers;

/// Detected input representation before normalization to JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceFormat {
    Json,
    Json5,
    TableTsv,
    TableCsv,
    KeyValue,
    QueryString,
    PlainText,
}

impl SourceFormat {
    pub fn display_label(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Json5 => "JSON5",
            Self::TableTsv => "TSV Table",
            Self::TableCsv => "CSV Table",
            Self::KeyValue => "Key-Value",
            Self::QueryString => "Query String",
            Self::PlainText => "Plain Text",
        }
    }
}

/// Output of the heuristic conversion pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvertResult {
    pub source_format: SourceFormat,
    pub value: Value,
    pub formatted: String,
}

/// Convert arbitrary text into canonical formatted JSON using the heuristic parser chain.
pub fn convert_to_json(input: &str) -> ConvertResult {
    let (source_format, value) = parsers::json_loose::try_parse(input)
        .or_else(|| parsers::table::try_parse(input))
        .or_else(|| parsers::kv::try_parse(input))
        .or_else(|| parsers::query::try_parse(input))
        .unwrap_or_else(|| parsers::fallback::wrap_plain_text(input));

    let formatted = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string());

    ConvertResult {
        source_format,
        value,
        formatted,
    }
}
