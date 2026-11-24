use serde_json::{Number, Value};

/// Coerce a scalar text cell/value into a typed JSON value without mangling IDs.
pub fn parse_scalar(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Value::String(String::new());
    }

    if trimmed.eq_ignore_ascii_case("true") && (trimmed == "true" || trimmed == "True") {
        return Value::Bool(true);
    }
    if trimmed.eq_ignore_ascii_case("false") && (trimmed == "false" || trimmed == "False") {
        return Value::Bool(false);
    }
    if trimmed == "null" || trimmed == "None" {
        return Value::Null;
    }

    // Preserve strings with leading zeros like "00123" or phone/zip codes.
    let has_leading_zero = {
        let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
        digits.len() > 1 && digits.starts_with('0') && !digits.starts_with("0.")
    };

    if !has_leading_zero {
        if let Ok(int_val) = trimmed.parse::<i64>() {
            return Value::Number(Number::from(int_val));
        }
        if let Ok(uint_val) = trimmed.parse::<u64>() {
            return Value::Number(Number::from(uint_val));
        }
        if let Ok(float_val) = trimmed.parse::<f64>()
            && float_val.is_finite()
            && let Some(num) = Number::from_f64(float_val)
        {
            return Value::Number(num);
        }
    }

    // Strip matching surrounding quotes when present (e.g. YAML/INI quoted strings).
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        if trimmed.starts_with('"')
            && let Ok(unescaped) = serde_json::from_str::<String>(trimmed)
        {
            return Value::String(unescaped);
        }
        return Value::String(trimmed[1..trimmed.len() - 1].to_string());
    }

    // Parse embedded JSON objects/arrays inside KV or query values.
    let is_json_container = (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'));
    if is_json_container && let Ok(embedded) = serde_json::from_str::<Value>(trimmed) {
        return embedded;
    }

    Value::String(trimmed.to_string())
}
