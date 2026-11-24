use serde_json::Value;

use crate::pipeline::SourceFormat;

/// Parse standard JSON or loose JSON5 (single quotes, trailing commas, unquoted keys, comments).
/// Also unwraps a JSON-encoded string whose inner text is itself a JSON object or array.
pub fn try_parse(input: &str) -> Option<(SourceFormat, Value)> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        match value {
            Value::Object(_) | Value::Array(_) => return Some((SourceFormat::Json, value)),
            Value::String(inner) => {
                let inner_trimmed = inner.trim();
                if let Ok(inner_val) = serde_json::from_str::<Value>(inner_trimmed)
                    && matches!(inner_val, Value::Object(_) | Value::Array(_))
                {
                    return Some((SourceFormat::Json, inner_val));
                }
                if let Ok(inner_val) = json5::from_str::<Value>(inner_trimmed)
                    && matches!(inner_val, Value::Object(_) | Value::Array(_))
                {
                    return Some((SourceFormat::Json5, inner_val));
                }
            }
            _ => {}
        }
    }

    // Only attempt JSON5 on structured containers (objects/arrays) or commented JSON blocks
    // to prevent bare words or numbers from falsely matching JSON5.
    let looks_structured = trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with("//")
        || trimmed.starts_with("/*");
    if looks_structured {
        if let Ok(value) = json5::from_str::<Value>(trimmed)
            && matches!(value, Value::Object(_) | Value::Array(_))
        {
            return Some((SourceFormat::Json5, value));
        }

        // Also support Python dict/list literals using True/False/None outside string literals.
        let normalized = normalize_python_literals(trimmed);
        if normalized != trimmed
            && let Ok(value) = json5::from_str::<Value>(&normalized)
            && matches!(value, Value::Object(_) | Value::Array(_))
        {
            return Some((SourceFormat::Json5, value));
        }
    }

    None
}

fn normalize_python_literals(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_quote: Option<char> = None;

    while i < chars.len() {
        let ch = chars[i];
        if let Some(q) = in_quote {
            out.push(ch);
            if ch == '\\' && i + 1 < chars.len() {
                i += 1;
                out.push(chars[i]);
            } else if ch == q {
                in_quote = None;
            }
            i += 1;
            continue;
        }

        if ch == '\'' || ch == '"' {
            in_quote = Some(ch);
            out.push(ch);
            i += 1;
            continue;
        }

        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let ident: String = chars[start..i].iter().collect();
            match ident.as_str() {
                "True" => out.push_str("true"),
                "False" => out.push_str("false"),
                "None" => out.push_str("null"),
                _ => out.push_str(&ident),
            }
            continue;
        }

        out.push(ch);
        i += 1;
    }

    out
}
