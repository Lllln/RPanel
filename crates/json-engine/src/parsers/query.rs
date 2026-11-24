use serde_json::{Map, Value};

use crate::parsers::scalar::parse_scalar;
use crate::pipeline::SourceFormat;

/// Parse full URLs with query strings (`https://host/path?a=1&b=2`),
/// leading `?` queries (`?a=1&b=2`), or raw QueryStrings (`a=1&b=2`).
pub fn try_parse(input: &str) -> Option<(SourceFormat, Value)> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.contains('\n') || trimmed.contains('\r') {
        return None;
    }

    let query_part = extract_query_part(trimmed)?;
    if query_part.is_empty() || !query_part.contains('=') {
        return None;
    }

    let mut map = Map::new();
    let mut pair_count = 0usize;

    for (key, val) in form_urlencoded::parse(query_part.as_bytes()) {
        let key = key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        let parsed_val = parse_scalar(&val);
        match map.get_mut(&key) {
            None => {
                map.insert(key, parsed_val);
            }
            Some(Value::Array(arr)) => {
                arr.push(parsed_val);
            }
            Some(existing) => {
                let prev = existing.take();
                *existing = Value::Array(vec![prev, parsed_val]);
            }
        }
        pair_count += 1;
    }

    if pair_count == 0 {
        return None;
    }

    Some((SourceFormat::QueryString, Value::Object(map)))
}

fn extract_query_part(trimmed: &str) -> Option<&str> {
    // Strip URL fragment `#...` if present.
    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);

    if let Some((_, query)) = without_fragment.split_once('?') {
        return Some(query);
    }

    // Raw query string without `?`: must contain `&` with `=` pairs and no spaces
    // (or URL-encoded tokens).
    if without_fragment.contains('&')
        && without_fragment.contains('=')
        && !without_fragment.contains(' ')
    {
        return Some(without_fragment);
    }

    None
}
