use serde_json::{Map, Value};

use crate::parsers::scalar::parse_scalar;
use crate::pipeline::SourceFormat;

/// Parse Key-Value pairs, HTTP headers (`Header: value`), `.env`/`INI` (`KEY=value`),
/// and flat YAML-like configuration blocks.
pub fn try_parse(input: &str) -> Option<(SourceFormat, Value)> {
    let non_empty: Vec<&str> = input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with(';'))
        .collect();

    if non_empty.is_empty() {
        return None;
    }

    // Do not steal URLs or QueryStrings on a single line (let `query.rs` handle them).
    if non_empty.len() == 1 && looks_like_query_or_url(non_empty[0]) {
        return None;
    }

    // Handle optional HTTP request/status start line (e.g. `GET /api HTTP/1.1` or `HTTP/1.1 200 OK`).
    let mut map = Map::new();
    let mut start_idx = 0;
    if non_empty.len() >= 2 && is_http_start_line(non_empty[0]) {
        map.insert(
            "_status".to_string(),
            Value::String(non_empty[0].to_string()),
        );
        start_idx = 1;
    }

    let candidates = &non_empty[start_idx..];
    if candidates.is_empty() {
        return None;
    }

    let mut parsed_count = 0usize;
    for &line in candidates {
        // Skip section headers like `[database]` in INI files when other lines are KV.
        if line.starts_with('[') && line.ends_with(']') && candidates.len() > 1 {
            continue;
        }

        let (key, raw_val) = split_kv_line(line)?;

        let value = parse_scalar(raw_val);
        insert_or_promote_array(&mut map, key.to_string(), value);
        parsed_count += 1;
    }

    if parsed_count == 0 {
        return None;
    }

    Some((SourceFormat::KeyValue, Value::Object(map)))
}

fn looks_like_query_or_url(line: &str) -> bool {
    if line.starts_with("http://") || line.starts_with("https://") || line.starts_with('?') {
        return true;
    }
    // Single-line `a=1&b=2` belongs to query parser.
    if line.contains('&') && line.contains('=') && !line.contains(": ") {
        return true;
    }
    false
}

fn is_http_start_line(line: &str) -> bool {
    if line.starts_with("HTTP/") {
        return true;
    }
    const METHODS: &[&str] = &[
        "GET ", "POST ", "PUT ", "PATCH ", "DELETE ", "HEAD ", "OPTIONS ",
    ];
    METHODS.iter().any(|m| line.starts_with(m)) && line.contains("HTTP/")
}

fn split_kv_line(line: &str) -> Option<(&str, &str)> {
    // Strip optional leading YAML list marker? No, lists aren't flat KV maps.
    if line.starts_with("- ") {
        return None;
    }

    let colon_idx = find_kv_colon(line);
    let eq_idx = line.find('=');

    let sep_idx = match (colon_idx, eq_idx) {
        (Some(c), Some(e)) => c.min(e),
        (Some(c), None) => c,
        (None, Some(e)) => e,
        (None, None) => return None,
    };

    let raw_key = line[..sep_idx].trim();
    let raw_val = line[sep_idx + 1..].trim();

    if !is_valid_key(raw_key) {
        return None;
    }

    Some((raw_key, raw_val))
}

fn find_kv_colon(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    for (idx, &b) in bytes.iter().enumerate() {
        if b == b':' {
            // Avoid matching URL schemes like `https://...`
            if line[idx..].starts_with("://") {
                continue;
            }
            // Require `:` to be either at end of line (`key:`) or followed by whitespace (`key: val`)
            // or inside compact header (`key:val` where key has no spaces).
            let next = bytes.get(idx + 1).copied();
            if matches!(next, None | Some(b' ') | Some(b'\t')) || !line[..idx].contains(' ') {
                return Some(idx);
            }
        }
    }
    None
}

fn is_valid_key(key: &str) -> bool {
    if key.is_empty() || key.len() > 128 {
        return false;
    }
    // Reject prose sentences as keys.
    if key.split_whitespace().count() > 3 {
        return false;
    }
    let first = key.chars().next().unwrap_or(' ');
    if !(first.is_alphanumeric() || first == '_' || first == '-' || first == '@' || first == '$') {
        return false;
    }
    key.chars().all(|c| {
        c.is_alphanumeric()
            || matches!(c, '_' | '-' | '.' | '/' | ' ' | '@' | '$' | '[' | ']')
    })
}

fn insert_or_promote_array(map: &mut Map<String, Value>, key: String, value: Value) {
    match map.get_mut(&key) {
        None => {
            map.insert(key, value);
        }
        Some(Value::Array(arr)) => {
            arr.push(value);
        }
        Some(existing) => {
            let prev = existing.take();
            *existing = Value::Array(vec![prev, value]);
        }
    }
}
