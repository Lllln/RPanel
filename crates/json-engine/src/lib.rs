//! Smart JSON conversion, formatting, escaping, and real-time validation engine.

pub mod inspector;
pub mod parsers;
pub mod pipeline;
pub mod timestamp;
pub mod validate;

pub use inspector::{
    InsightKind, InsightTone, SmartInsight, detect_insight_in_editor, detect_smart_insight,
};
pub use pipeline::{ConvertResult, SourceFormat, convert_to_json};
pub use timestamp::{
    ParsedTimestamp, TimestampUnit, detect_timestamp_in_editor, parse_timestamp,
};
pub use validate::{ValidationError, ValidationStatus, validate_json};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum JsonEngineError {
    #[error("JSON syntax error at line {line}, column {column}: {message}")]
    InvalidJson {
        line: usize,
        column: usize,
        message: String,
    },
    #[error("Failed to serialize JSON: {0}")]
    Serialize(String),
    #[error("Failed to unescape string: {0}")]
    Unescape(String),
}

/// Pretty-print a JSON or JSON5 string with 2-space indentation.
pub fn format_pretty(input: &str) -> Result<String, JsonEngineError> {
    let value = parse_strict_or_loose(input)?;
    serde_json::to_string_pretty(&value).map_err(|e| JsonEngineError::Serialize(e.to_string()))
}

/// Minify a JSON or JSON5 string to a compact single-line representation.
pub fn minify(input: &str) -> Result<String, JsonEngineError> {
    let value = parse_strict_or_loose(input)?;
    serde_json::to_string(&value).map_err(|e| JsonEngineError::Serialize(e.to_string()))
}

/// Escape text into a JSON string literal (including surrounding quotes).
pub fn escape_json_string(input: &str) -> Result<String, JsonEngineError> {
    serde_json::to_string(input).map_err(|e| JsonEngineError::Serialize(e.to_string()))
}

/// Unescape a JSON string literal (or raw backslash-escaped payload).
pub fn unescape_json_string(input: &str) -> Result<String, JsonEngineError> {
    let trimmed = input.trim();
    if let Ok(unescaped) = serde_json::from_str::<String>(trimmed) {
        return Ok(unescaped);
    }
    let wrapped = format!("\"{trimmed}\"");
    serde_json::from_str::<String>(&wrapped)
        .map_err(|e| JsonEngineError::Unescape(e.to_string()))
}

fn parse_strict_or_loose(input: &str) -> Result<serde_json::Value, JsonEngineError> {
    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(value) => Ok(value),
        Err(strict_err) => json5::from_str::<serde_json::Value>(input).map_err(|_| {
            JsonEngineError::InvalidJson {
                line: strict_err.line(),
                column: strict_err.column(),
                message: strict_err.to_string(),
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_standard_json_and_preserves_order() {
        let res = convert_to_json(r#"{"z":1,"a":[true,null,"ok"]}"#);
        assert_eq!(res.source_format, SourceFormat::Json);
        assert_eq!(res.value, json!({"z": 1, "a": [true, null, "ok"]}));
        let keys: Vec<&str> = res
            .value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, vec!["z", "a"]);
    }

    #[test]
    fn unwraps_double_encoded_json_string() {
        let res = convert_to_json(r#""{\"user\":\"alice\",\"id\":42}""#);
        assert_eq!(res.source_format, SourceFormat::Json);
        assert_eq!(res.value, json!({"user": "alice", "id": 42}));
    }

    #[test]
    fn parses_loose_json5_and_python_dict() {
        let json5_input = r#"{
            // comment
            name: 'RPanel',
            tags: ['rust', 'gpui',],
        }"#;
        let res = convert_to_json(json5_input);
        assert_eq!(res.source_format, SourceFormat::Json5);
        assert_eq!(res.value, json!({"name": "RPanel", "tags": ["rust", "gpui"]}));

        let py_input = "{'active': True, 'deleted': False, 'meta': None}";
        let py_res = convert_to_json(py_input);
        assert_eq!(py_res.source_format, SourceFormat::Json5);
        assert_eq!(
            py_res.value,
            json!({"active": true, "deleted": false, "meta": null})
        );
    }

    #[test]
    fn parses_tsv_table_to_object_array() {
        let tsv = "id\tname\tscore\tcode\n1\tAlice\t98.5\t0012\n2\tBob\t87\t0099";
        let res = convert_to_json(tsv);
        assert_eq!(res.source_format, SourceFormat::TableTsv);
        assert_eq!(
            res.value,
            json!([
                {"id": 1, "name": "Alice", "score": 98.5, "code": "0012"},
                {"id": 2, "name": "Bob", "score": 87, "code": "0099"}
            ])
        );
    }

    #[test]
    fn parses_csv_table_with_quotes() {
        let csv = "city,pop,capital\n\"San Francisco, CA\",808437,false\nTokyo,13960000,true";
        let res = convert_to_json(csv);
        assert_eq!(res.source_format, SourceFormat::TableCsv);
        assert_eq!(
            res.value,
            json!([
                {"city": "San Francisco, CA", "pop": 808437, "capital": false},
                {"city": "Tokyo", "pop": 13960000, "capital": true}
            ])
        );
    }

    #[test]
    fn parses_http_headers_and_duplicate_keys() {
        let headers = "\
HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: 512
Set-Cookie: a=1
Set-Cookie: b=2
";
        let res = convert_to_json(headers);
        assert_eq!(res.source_format, SourceFormat::KeyValue);
        assert_eq!(
            res.value,
            json!({
                "_status": "HTTP/1.1 200 OK",
                "Content-Type": "application/json",
                "Content-Length": 512,
                "Set-Cookie": ["a=1", "b=2"]
            })
        );
    }

    #[test]
    fn parses_env_and_yaml_like_kv() {
        let env = "# app config\nHOST=127.0.0.1\nPORT=8080\nDEBUG=true\nAPI_URL=https://api.example.com/v1";
        let res = convert_to_json(env);
        assert_eq!(res.source_format, SourceFormat::KeyValue);
        assert_eq!(
            res.value,
            json!({
                "HOST": "127.0.0.1",
                "PORT": 8080,
                "DEBUG": true,
                "API_URL": "https://api.example.com/v1"
            })
        );
    }

    #[test]
    fn parses_url_and_raw_query_string() {
        let url = "https://example.com/search?q=rust+gpui&page=2&tag=ui&tag=fast#top";
        let res = convert_to_json(url);
        assert_eq!(res.source_format, SourceFormat::QueryString);
        assert_eq!(
            res.value,
            json!({
                "q": "rust gpui",
                "page": 2,
                "tag": ["ui", "fast"]
            })
        );

        let raw = "a=1&b=hello%20world&ok=true";
        let raw_res = convert_to_json(raw);
        assert_eq!(raw_res.source_format, SourceFormat::QueryString);
        assert_eq!(
            raw_res.value,
            json!({"a": 1, "b": "hello world", "ok": true})
        );
    }

    #[test]
    fn falls_back_to_plain_text_object() {
        let res = convert_to_json("Just a regular sentence copied from an article.");
        assert_eq!(res.source_format, SourceFormat::PlainText);
        assert_eq!(
            res.value,
            json!({"text": "Just a regular sentence copied from an article."})
        );
    }

    #[test]
    fn pretty_minify_escape_unescape_and_validate() {
        let pretty = format_pretty("{a: 1, b: 'hi',}").unwrap();
        assert_eq!(pretty, "{\n  \"a\": 1,\n  \"b\": \"hi\"\n}");

        let compact = minify(&pretty).unwrap();
        assert_eq!(compact, r#"{"a":1,"b":"hi"}"#);

        let escaped = escape_json_string(&compact).unwrap();
        assert_eq!(escaped, r#""{\"a\":1,\"b\":\"hi\"}""#);

        let unescaped = unescape_json_string(&escaped).unwrap();
        assert_eq!(unescaped, compact);

        let raw_unescaped = unescape_json_string(r#"{\"a\":1}"#).unwrap();
        assert_eq!(raw_unescaped, r#"{"a":1}"#);

        assert_eq!(validate_json("   "), ValidationStatus::Empty);
        assert_eq!(validate_json(&pretty), ValidationStatus::Valid);

        match validate_json("{\n  \"a\": 1,\n}") {
            ValidationStatus::Invalid(err) => {
                assert_eq!(err.line, 3);
                assert_eq!(err.column, 1);
                assert!(err.byte_offset > 0);
            }
            other => panic!("expected invalid JSON status, got {other:?}"),
        }
    }
}
