use chrono::{DateTime, FixedOffset, Local, Offset};
use serde_json::Value;

/// Inclusive lower bound for recognized Unix timestamps in seconds (`2001-09-09T01:46:40Z`).
/// Avoids false positives on short numbers, dates (`20261126`), or 9-digit IDs.
const MIN_UNIX_SECS: i64 = 1_000_000_000;
/// Inclusive upper bound for recognized Unix timestamps in seconds (`2100-01-01T00:00:00Z`).
const MAX_UNIX_SECS: i64 = 4_102_444_800;

/// Detected precision unit of a Unix epoch timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampUnit {
    Seconds,
    Milliseconds,
    Microseconds,
    Nanoseconds,
}

impl TimestampUnit {
    /// Concise Chinese label for the detected timestamp unit.
    pub const fn label_zh(self) -> &'static str {
        match self {
            Self::Seconds => "秒",
            Self::Milliseconds => "毫秒",
            Self::Microseconds => "微秒",
            Self::Nanoseconds => "纳秒",
        }
    }
}

/// Parsed Unix timestamp converted into the user's local date and time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedTimestamp {
    /// Cleaned numeric token without surrounding quotes/punctuation.
    pub raw: String,
    pub unit: TimestampUnit,
    /// Whole seconds since Unix epoch.
    pub epoch_secs: i64,
    /// Sub-second nanoseconds (`0..1_000_000_000`).
    pub subsec_nanos: u32,
    /// Local date-time string formatted as `YYYY-MM-DD HH:MM:SS[.fff...]`.
    pub local_datetime: String,
    /// Human-readable local timezone offset label such as `UTC+8` or `UTC`.
    pub timezone_label: String,
}

/// Try to parse a standalone Unix timestamp (10-digit seconds, optional fractional seconds,
/// 13-digit milliseconds, 16-digit microseconds, or 19-digit nanoseconds) and convert it
/// into the user's local timezone.
///
/// Tolerates surrounding whitespace, matching `"`/`'` quotes, and trailing `,`/`;`.
pub fn parse_timestamp(input: &str) -> Option<ParsedTimestamp> {
    let (raw, unit, epoch_secs, subsec_nanos) = parse_raw_epoch(input)?;
    let utc = DateTime::from_timestamp(epoch_secs, subsec_nanos)?;
    let local = utc.with_timezone(&Local);
    let offset = local.offset().fix();
    let (local_datetime, timezone_label) = format_in_offset(utc, subsec_nanos, offset);

    Some(ParsedTimestamp {
        raw,
        unit,
        epoch_secs,
        subsec_nanos,
        local_datetime,
        timezone_label,
    })
}

/// Try to detect a timestamp either directly from raw text or from a single-field plain-text
/// wrapper JSON (`{"text": "1732619278"}`) produced by `convert_to_json`.
pub fn detect_timestamp_in_editor(input: &str) -> Option<ParsedTimestamp> {
    if let Some(ts) = parse_timestamp(input) {
        return Some(ts);
    }
    let Value::Object(map) = serde_json::from_str::<Value>(input.trim()).ok()? else {
        return None;
    };
    if map.len() != 1 {
        return None;
    }
    let Value::String(text) = map.get("text")? else {
        return None;
    };
    parse_timestamp(text)
}

fn parse_raw_epoch(input: &str) -> Option<(String, TimestampUnit, i64, u32)> {
    let mut token = input.trim();
    if token.is_empty() {
        return None;
    }

    // Strip optional trailing comma or semicolon commonly included when selecting JSON/SQL values.
    token = token.trim_end_matches([',', ';']).trim();

    // Strip matching outer quotes, backticks, or brackets/parens commonly grabbed in logs/shells.
    loop {
        if token.len() >= 2
            && ((token.starts_with('"') && token.ends_with('"'))
                || (token.starts_with('\'') && token.ends_with('\''))
                || (token.starts_with('`') && token.ends_with('`'))
                || (token.starts_with('[') && token.ends_with(']'))
                || (token.starts_with('(') && token.ends_with(')')))
        {
            token = token[1..token.len() - 1].trim();
        } else {
            break;
        }
    }

    if token.is_empty() || token.starts_with('0') {
        return None;
    }

    // Fractional seconds: e.g. `1732619278.123` or `1732619278.123456`.
    if let Some((int_part, frac_part)) = token.split_once('.') {
        if int_part.len() != 10
            || frac_part.is_empty()
            || frac_part.len() > 9
            || !int_part.bytes().all(|b| b.is_ascii_digit())
            || !frac_part.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let secs = int_part.parse::<i64>().ok()?;
        if !(MIN_UNIX_SECS..=MAX_UNIX_SECS).contains(&secs) {
            return None;
        }
        let scale = 10u32.pow((9 - frac_part.len()) as u32);
        let nanos = frac_part.parse::<u32>().ok()?.checked_mul(scale)?;
        return Some((token.to_string(), TimestampUnit::Seconds, secs, nanos));
    }

    if !token.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    match token.len() {
        10 => {
            let secs = token.parse::<i64>().ok()?;
            if !(MIN_UNIX_SECS..=MAX_UNIX_SECS).contains(&secs) {
                return None;
            }
            Some((token.to_string(), TimestampUnit::Seconds, secs, 0))
        }
        13 => {
            let ms = token.parse::<i64>().ok()?;
            let secs = ms / 1_000;
            if !(MIN_UNIX_SECS..=MAX_UNIX_SECS).contains(&secs) {
                return None;
            }
            let nanos = ((ms % 1_000) as u32) * 1_000_000;
            Some((token.to_string(), TimestampUnit::Milliseconds, secs, nanos))
        }
        16 => {
            let us = token.parse::<i64>().ok()?;
            let secs = us / 1_000_000;
            if !(MIN_UNIX_SECS..=MAX_UNIX_SECS).contains(&secs) {
                return None;
            }
            let nanos = ((us % 1_000_000) as u32) * 1_000;
            Some((token.to_string(), TimestampUnit::Microseconds, secs, nanos))
        }
        19 => {
            let ns = token.parse::<i64>().ok()?;
            let secs = ns / 1_000_000_000;
            if !(MIN_UNIX_SECS..=MAX_UNIX_SECS).contains(&secs) {
                return None;
            }
            let nanos = (ns % 1_000_000_000) as u32;
            Some((token.to_string(), TimestampUnit::Nanoseconds, secs, nanos))
        }
        _ => None,
    }
}

fn format_in_offset(
    utc: DateTime<chrono::Utc>,
    subsec_nanos: u32,
    offset: FixedOffset,
) -> (String, String) {
    let dt = utc.with_timezone(&offset);
    let formatted = if subsec_nanos == 0 {
        dt.format("%Y-%m-%d %H:%M:%S").to_string()
    } else if subsec_nanos.is_multiple_of(1_000_000) {
        dt.format("%Y-%m-%d %H:%M:%S.%3f").to_string()
    } else if subsec_nanos.is_multiple_of(1_000) {
        dt.format("%Y-%m-%d %H:%M:%S.%6f").to_string()
    } else {
        dt.format("%Y-%m-%d %H:%M:%S.%9f").to_string()
    };

    let offset_secs = offset.local_minus_utc();
    let tz_label = if offset_secs == 0 {
        "UTC".to_string()
    } else if offset_secs % 3600 == 0 {
        format!("UTC{:+}", offset_secs / 3600)
    } else {
        let sign = if offset_secs >= 0 { '+' } else { '-' };
        let abs = offset_secs.unsigned_abs();
        let hours = abs / 3600;
        let mins = (abs % 3600) / 60;
        format!("UTC{sign}{hours:02}:{mins:02}")
    };

    (formatted, tz_label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_seconds_milliseconds_microseconds_nanoseconds() {
        let sec = parse_timestamp("1732619278").expect("10-digit seconds");
        assert_eq!(sec.unit, TimestampUnit::Seconds);
        assert_eq!(sec.epoch_secs, 1732619278);
        assert_eq!(sec.subsec_nanos, 0);

        let ms = parse_timestamp(" 1732619278456, ").expect("13-digit ms with comma");
        assert_eq!(ms.unit, TimestampUnit::Milliseconds);
        assert_eq!(ms.epoch_secs, 1732619278);
        assert_eq!(ms.subsec_nanos, 456_000_000);

        let us = parse_timestamp("\"1732619278456789\"").expect("16-digit us in quotes");
        assert_eq!(us.unit, TimestampUnit::Microseconds);
        assert_eq!(us.epoch_secs, 1732619278);
        assert_eq!(us.subsec_nanos, 456_789_000);

        let ns = parse_timestamp("'1732619278456789123'").expect("19-digit ns in quotes");
        assert_eq!(ns.unit, TimestampUnit::Nanoseconds);
        assert_eq!(ns.epoch_secs, 1732619278);
        assert_eq!(ns.subsec_nanos, 456_789_123);

        let frac = parse_timestamp("1732619278.5").expect("fractional seconds");
        assert_eq!(frac.unit, TimestampUnit::Seconds);
        assert_eq!(frac.epoch_secs, 1732619278);
        assert_eq!(frac.subsec_nanos, 500_000_000);

        let bracketed = parse_timestamp("[1732619278000]").expect("bracketed timestamp");
        assert_eq!(bracketed.unit, TimestampUnit::Milliseconds);
        assert_eq!(bracketed.epoch_secs, 1732619278);

        let backticked = parse_timestamp("`1732619278`").expect("backticked timestamp");
        assert_eq!(backticked.unit, TimestampUnit::Seconds);
        assert_eq!(backticked.epoch_secs, 1732619278);

        let parenthesized = parse_timestamp("(1732619278),").expect("parenthesized timestamp");
        assert_eq!(parenthesized.unit, TimestampUnit::Seconds);
        assert_eq!(parenthesized.epoch_secs, 1732619278);
    }

    #[test]
    fn rejects_non_timestamps_and_phone_numbers() {
        assert!(parse_timestamp("13800138000").is_none()); // 11-digit phone number
        assert!(parse_timestamp("20261126").is_none()); // 8-digit date
        assert!(parse_timestamp("8080").is_none());
        assert!(parse_timestamp("01732619278").is_none()); // leading zero
        assert!(parse_timestamp("9999999999").is_none()); // out of range
        assert!(parse_timestamp(r#"{"ts": 1732619278}"#).is_none());
        assert!(parse_timestamp("hello world").is_none());
    }

    #[test]
    fn formats_deterministic_offset_and_subseconds() {
        let utc = DateTime::from_timestamp(1732619278, 0).unwrap();
        let cst = FixedOffset::east_opt(8 * 3600).unwrap();
        let (dt, tz) = format_in_offset(utc, 0, cst);
        assert_eq!(dt, "2024-11-26 19:07:58");
        assert_eq!(tz, "UTC+8");

        let utc_ms = DateTime::from_timestamp(1732619278, 123_000_000).unwrap();
        let (dt_ms, _) = format_in_offset(utc_ms, 123_000_000, cst);
        assert_eq!(dt_ms, "2024-11-26 19:07:58.123");

        let ist = FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap();
        let (_, tz_ist) = format_in_offset(utc, 0, ist);
        assert_eq!(tz_ist, "UTC+05:30");
    }

    #[test]
    fn detects_timestamp_in_plain_text_wrapper() {
        let wrapped = "{\n  \"text\": \"1732619278000\"\n}";
        let detected = detect_timestamp_in_editor(wrapped).expect("wrapped timestamp");
        assert_eq!(detected.unit, TimestampUnit::Milliseconds);
        assert_eq!(detected.epoch_secs, 1732619278);

        assert!(detect_timestamp_in_editor(r#"{"a": 1, "text": "1732619278"}"#).is_none());
    }
}
