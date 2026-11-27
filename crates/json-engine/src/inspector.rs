//! Geek-oriented smart text detectors for instant cursor-side insight cards.
//!
//! Pure-Rust, zero-network detectors covering Unix timestamps, date strings, JWT tokens,
//! CSS/hex colors, IPv4 CIDRs, 5-field Cron expressions, radix literals, Unicode escapes,
//! URL percent-encoding, and Base64/Base64URL payloads.

use std::net::Ipv4Addr;

use chrono::{
    DateTime, Datelike, Duration, FixedOffset, Local, NaiveDate, NaiveDateTime, Offset, TimeZone,
    Timelike, Utc,
};
use serde_json::{Map, Value};

use crate::timestamp::parse_timestamp;

/// Category of a detected smart insight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightKind {
    Timestamp,
    DateTime,
    Jwt,
    Color,
    Cidr,
    Cron,
    Radix,
    UnicodeEscape,
    UrlEncoded,
    Base64,
}

/// Visual accent tone for rendering the insight card and editor banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightTone {
    Green,
    Blue,
    Purple,
    Amber,
    Neutral,
}

/// A single structured insight rendered in the floating quick-action menu or editor banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmartInsight {
    pub kind: InsightKind,
    pub tone: InsightTone,
    /// Short badge label shown on the left (e.g. "时间", "JWT", "颜色", "Cron").
    pub badge: String,
    /// Primary decoded/converted display line.
    pub primary: String,
    /// Secondary detail & click-hint line.
    pub subtitle: String,
    /// Compact pill tag on the right (e.g. "毫秒", "HS256", "HEX→RGB").
    pub pill: String,
    /// Text written to the clipboard when the user clicks the insight card.
    pub copy_text: String,
    /// Optional 24-bit RGB swatch (`0xRRGGBB`) rendered for color insights.
    pub color_swatch: Option<u32>,
    /// Optional pre-decoded JSON payload to open directly in the floating JSON editor.
    pub editor_json: Option<String>,
}

/// Inspect raw selected text and return the highest-confidence smart insight, if any.
pub fn detect_smart_insight(input: &str) -> Option<SmartInsight> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.len() > 16_384 {
        return None;
    }

    detect_timestamp_insight(trimmed)
        .or_else(|| detect_jwt(trimmed))
        .or_else(|| detect_datetime(trimmed))
        .or_else(|| detect_color(trimmed))
        .or_else(|| detect_cidr(trimmed))
        .or_else(|| detect_cron(trimmed))
        .or_else(|| detect_radix(trimmed))
        .or_else(|| detect_unicode_escape(trimmed))
        .or_else(|| detect_url_encoded(trimmed))
        .or_else(|| detect_base64(trimmed))
}

/// Inspect either raw editor content or a single-field plain-text wrapper (`{"text": "..."}`)
/// produced by `convert_to_json`.
pub fn detect_insight_in_editor(input: &str) -> Option<SmartInsight> {
    if let Some(insight) = detect_smart_insight(input) {
        return Some(insight);
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
    detect_smart_insight(text)
}

fn strip_token_wrappers(input: &str) -> &str {
    let mut token = input.trim();
    token = token.trim_end_matches([',', ';']).trim();
    if token.len() >= 2
        && ((token.starts_with('"') && token.ends_with('"'))
            || (token.starts_with('\'') && token.ends_with('\''))
            || (token.starts_with('`') && token.ends_with('`')))
    {
        token = token[1..token.len() - 1].trim();
    }
    token
}

fn format_offset_label(offset: FixedOffset) -> String {
    let offset_secs = offset.local_minus_utc();
    if offset_secs == 0 {
        "UTC".to_string()
    } else if offset_secs % 3600 == 0 {
        format!("UTC{:+}", offset_secs / 3600)
    } else {
        let sign = if offset_secs >= 0 { '+' } else { '-' };
        let abs = offset_secs.unsigned_abs();
        let hours = abs / 3600;
        let mins = (abs % 3600) / 60;
        format!("UTC{sign}{hours:02}:{mins:02}")
    }
}

fn truncate_single_line(text: &str, max_chars: usize) -> String {
    let normalized = text
        .replace("\r\n", " ")
        .replace(['\n', '\r', '\t'], " ");
    let mut out = String::new();
    for (idx, ch) in normalized.chars().enumerate() {
        if idx >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------------------
// 1. Unix Timestamp -> Local DateTime (reusing `crate::timestamp`)
// ---------------------------------------------------------------------------

fn detect_timestamp_insight(input: &str) -> Option<SmartInsight> {
    let ts = parse_timestamp(input)?;
    let subtitle = format!("本地时间 ({}) · 点击复制", ts.timezone_label);
    Some(SmartInsight {
        kind: InsightKind::Timestamp,
        tone: InsightTone::Green,
        badge: "时间".to_string(),
        primary: ts.local_datetime.clone(),
        subtitle,
        pill: ts.unit.label_zh().to_string(),
        copy_text: ts.local_datetime,
        color_swatch: None,
        editor_json: None,
    })
}

// ---------------------------------------------------------------------------
// 2. JWT Decoder
// ---------------------------------------------------------------------------

fn detect_jwt(input: &str) -> Option<SmartInsight> {
    let mut token = strip_token_wrappers(input);
    if let Some(rest) = token
        .strip_prefix("Bearer ")
        .or_else(|| token.strip_prefix("bearer "))
    {
        token = rest.trim();
    }

    let mut parts = token.split('.');
    let header_b64 = parts.next()?;
    let payload_b64 = parts.next()?;
    let sig_b64 = parts.next()?;
    if parts.next().is_some() || header_b64.len() < 4 || payload_b64.len() < 4 {
        return None;
    }
    if !header_b64
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'=')
        || !payload_b64
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'=')
        || !sig_b64
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'=')
    {
        return None;
    }

    let header_bytes = decode_base64_flex(header_b64, true)?;
    let payload_bytes = decode_base64_flex(payload_b64, true)?;
    let header_val: Value = serde_json::from_slice(&header_bytes).ok()?;
    let payload_val: Value = serde_json::from_slice(&payload_bytes).ok()?;
    let header_obj = header_val.as_object()?;
    let payload_obj = payload_val.as_object()?;

    if !header_obj.contains_key("alg") && !header_obj.contains_key("typ") {
        return None;
    }

    let alg = header_obj
        .get("alg")
        .and_then(Value::as_str)
        .unwrap_or("JWT")
        .to_string();

    let now_secs = Utc::now().timestamp();
    let (status_text, tone, exp_local_opt) = match payload_obj.get("exp").and_then(Value::as_i64) {
        Some(exp) => {
            let exp_local = DateTime::from_timestamp(exp, 0).map(|dt| {
                dt.with_timezone(&Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            });
            let delta = exp - now_secs;
            if delta >= 0 {
                (
                    format!("有效 · 剩余 {}", format_compact_duration(delta)),
                    InsightTone::Green,
                    exp_local,
                )
            } else {
                (
                    format!("已过期 · {}", format_compact_duration(-delta)),
                    InsightTone::Amber,
                    exp_local,
                )
            }
        }
        None => ("JWT 已解码 · 无 exp".to_string(), InsightTone::Purple, None),
    };

    let identity = ["sub", "email", "username", "name", "user_id", "iss"]
        .iter()
        .find_map(|key| {
            payload_obj.get(*key).and_then(|v| match v {
                Value::String(s) if !s.is_empty() => Some(format!("{key}: {s}")),
                Value::Number(n) => Some(format!("{key}: {n}")),
                _ => None,
            })
        });

    let primary = match identity {
        Some(id) => format!("{status_text} · {id}"),
        None => status_text,
    };

    let subtitle = match exp_local_opt.as_deref() {
        Some(exp_str) => format!("过期时间 {exp_str} · 点击复制 Payload"),
        None => format!("{} 个字段 · 点击复制 Payload", payload_obj.len()),
    };

    let payload_pretty = serde_json::to_string_pretty(&payload_val).ok()?;
    let iat_local_opt = payload_obj
        .get("iat")
        .and_then(Value::as_i64)
        .and_then(|iat| DateTime::from_timestamp(iat, 0))
        .map(|iat_dt| {
            iat_dt
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        });

    let mut combined = Map::new();
    combined.insert("header".to_string(), header_val);
    combined.insert("payload".to_string(), payload_val);
    if exp_local_opt.is_some() || iat_local_opt.is_some() {
        let mut meta = Map::new();
        if let Some(exp_str) = exp_local_opt {
            meta.insert("exp_local".to_string(), Value::String(exp_str));
        }
        if let Some(iat_str) = iat_local_opt {
            meta.insert("iat_local".to_string(), Value::String(iat_str));
        }
        combined.insert("_meta".to_string(), Value::Object(meta));
    }
    combined.insert("signature".to_string(), Value::String(sig_b64.to_string()));
    let editor_json = serde_json::to_string_pretty(&Value::Object(combined)).ok();

    Some(SmartInsight {
        kind: InsightKind::Jwt,
        tone,
        badge: "JWT".to_string(),
        primary: truncate_single_line(&primary, 48),
        subtitle,
        pill: alg,
        copy_text: payload_pretty,
        color_swatch: None,
        editor_json,
    })
}

fn format_compact_duration(secs: i64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h{m}m")
        }
    } else {
        let d = secs / 86_400;
        let h = (secs % 86_400) / 3600;
        if h == 0 {
            format!("{d}天")
        } else {
            format!("{d}天{h}h")
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Date / ISO-8601 String -> Unix Timestamp
// ---------------------------------------------------------------------------

fn detect_datetime(input: &str) -> Option<SmartInsight> {
    let token = strip_token_wrappers(input);
    if token.len() < 10 || token.len() > 40 || token.contains('\n') {
        return None;
    }

    // 3a. RFC 3339 / ISO-8601 with explicit timezone offset or `Z`.
    if let Ok(dt) = DateTime::parse_from_rfc3339(token) {
        let secs = dt.timestamp();
        let millis = dt.timestamp_millis();
        let tz = format_offset_label(*dt.offset());
        return Some(build_datetime_insight(secs, millis, &tz, "ISO→秒"));
    }

    // Also allow space separator before explicit timezone, e.g. `2026-11-26 19:48:00+08:00`.
    if let Ok(dt) = DateTime::parse_from_str(token, "%Y-%m-%d %H:%M:%S%:z")
        .or_else(|_| DateTime::parse_from_str(token, "%Y-%m-%d %H:%M:%S%.f%:z"))
    {
        let secs = dt.timestamp();
        let millis = dt.timestamp_millis();
        let tz = format_offset_label(*dt.offset());
        return Some(build_datetime_insight(secs, millis, &tz, "日期→秒"));
    }

    // 3b. Local naive date-time formats.
    let naive_formats = [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y/%m/%d %H:%M:%S%.f",
        "%Y/%m/%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%Y/%m/%d %H:%M",
    ];
    for fmt in naive_formats {
        if let Ok(naive) = NaiveDateTime::parse_from_str(token, fmt) {
            let local_dt = Local.from_local_datetime(&naive).earliest()?;
            let tz = format_offset_label(local_dt.offset().fix());
            return Some(build_datetime_insight(
                local_dt.timestamp(),
                local_dt.timestamp_millis(),
                &tz,
                "日期→秒",
            ));
        }
    }

    // 3c. Date-only (`YYYY-MM-DD` or `YYYY/MM/DD`), interpreted as local midnight.
    let date_formats = ["%Y-%m-%d", "%Y/%m/%d"];
    for fmt in date_formats {
        if let Ok(date) = NaiveDate::parse_from_str(token, fmt) {
            let naive = date.and_hms_opt(0, 0, 0)?;
            let local_dt = Local.from_local_datetime(&naive).earliest()?;
            let tz = format_offset_label(local_dt.offset().fix());
            return Some(build_datetime_insight(
                local_dt.timestamp(),
                local_dt.timestamp_millis(),
                &tz,
                "零点→秒",
            ));
        }
    }

    None
}

fn build_datetime_insight(secs: i64, millis: i64, tz: &str, pill: &str) -> SmartInsight {
    SmartInsight {
        kind: InsightKind::DateTime,
        tone: InsightTone::Blue,
        badge: "时间戳".to_string(),
        primary: secs.to_string(),
        subtitle: format!("毫秒 {millis} ({tz}) · 点击复制秒级"),
        pill: pill.to_string(),
        copy_text: secs.to_string(),
        color_swatch: None,
        editor_json: None,
    }
}

// ---------------------------------------------------------------------------
// 4. CSS / HEX Color Preview & Conversion
// ---------------------------------------------------------------------------

fn detect_color(input: &str) -> Option<SmartInsight> {
    let token = strip_token_wrappers(input);
    if token.len() < 4 || token.len() > 48 || token.contains('\n') {
        return None;
    }

    if let Some((r, g, b, a)) = parse_hex_color(token) {
        let swatch = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        let rgb_str = format_rgb(r, g, b, a);
        let hsl_str = format_hsl(r, g, b, a);
        return Some(SmartInsight {
            kind: InsightKind::Color,
            tone: InsightTone::Neutral,
            badge: "颜色".to_string(),
            primary: rgb_str.clone(),
            subtitle: format!("{hsl_str} · 点击复制 RGB"),
            pill: "HEX→RGB".to_string(),
            copy_text: rgb_str,
            color_swatch: Some(swatch),
            editor_json: None,
        });
    }

    if let Some((r, g, b, a)) = parse_rgb_color(token) {
        let swatch = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        let hex_str = format_hex(r, g, b, a);
        let hsl_str = format_hsl(r, g, b, a);
        return Some(SmartInsight {
            kind: InsightKind::Color,
            tone: InsightTone::Neutral,
            badge: "颜色".to_string(),
            primary: hex_str.clone(),
            subtitle: format!("{hsl_str} · 点击复制 HEX"),
            pill: "RGB→HEX".to_string(),
            copy_text: hex_str,
            color_swatch: Some(swatch),
            editor_json: None,
        });
    }

    if let Some((r, g, b, a)) = parse_hsl_color(token) {
        let swatch = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
        let hex_str = format_hex(r, g, b, a);
        let rgb_str = format_rgb(r, g, b, a);
        return Some(SmartInsight {
            kind: InsightKind::Color,
            tone: InsightTone::Neutral,
            badge: "颜色".to_string(),
            primary: hex_str.clone(),
            subtitle: format!("{rgb_str} · 点击复制 HEX"),
            pill: "HSL→HEX".to_string(),
            copy_text: hex_str,
            color_swatch: Some(swatch),
            editor_json: None,
        });
    }

    None
}

fn parse_hex_color(token: &str) -> Option<(u8, u8, u8, Option<f32>)> {
    let hex = token.strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            Some((r, g, b, None))
        }
        4 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
            let a = (u8::from_str_radix(&hex[3..4], 16).ok()? * 17) as f32 / 255.0;
            Some((r, g, b, Some(a)))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b, None))
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let a = u8::from_str_radix(&hex[6..8], 16).ok()? as f32 / 255.0;
            Some((r, g, b, Some(a)))
        }
        _ => None,
    }
}

fn parse_rgb_color(token: &str) -> Option<(u8, u8, u8, Option<f32>)> {
    let lower = token.to_ascii_lowercase();
    let inner = lower
        .strip_prefix("rgb(")
        .or_else(|| lower.strip_prefix("rgba("))
        .and_then(|s| s.strip_suffix(')'))?;

    let (rgb_part, alpha_part) = match inner.split_once('/') {
        Some((left, right)) => (left.trim(), Some(right.trim())),
        None => (inner.trim(), None),
    };

    let parts: Vec<&str> = if rgb_part.contains(',') {
        rgb_part.split(',').map(str::trim).filter(|s| !s.is_empty()).collect()
    } else {
        rgb_part.split_whitespace().collect()
    };

    let (r_str, g_str, b_str, a_str) = match (parts.as_slice(), alpha_part) {
        ([r, g, b], a) => (*r, *g, *b, a),
        ([r, g, b, a], None) => (*r, *g, *b, Some(*a)),
        _ => return None,
    };

    let r: u8 = r_str.parse().ok()?;
    let g: u8 = g_str.parse().ok()?;
    let b: u8 = b_str.parse().ok()?;
    let alpha = match a_str {
        Some(a) => Some(parse_alpha_value(a)?),
        None => None,
    };
    Some((r, g, b, alpha))
}

fn parse_hsl_color(token: &str) -> Option<(u8, u8, u8, Option<f32>)> {
    let lower = token.to_ascii_lowercase();
    let inner = lower
        .strip_prefix("hsl(")
        .or_else(|| lower.strip_prefix("hsla("))
        .and_then(|s| s.strip_suffix(')'))?;

    let (hsl_part, alpha_part) = match inner.split_once('/') {
        Some((left, right)) => (left.trim(), Some(right.trim())),
        None => (inner.trim(), None),
    };

    let parts: Vec<&str> = if hsl_part.contains(',') {
        hsl_part.split(',').map(str::trim).filter(|s| !s.is_empty()).collect()
    } else {
        hsl_part.split_whitespace().collect()
    };

    let (h_str, s_str, l_str, a_str) = match (parts.as_slice(), alpha_part) {
        ([h, s, l], a) => (*h, *s, *l, a),
        ([h, s, l, a], None) => (*h, *s, *l, Some(*a)),
        _ => return None,
    };

    let h: f32 = h_str.strip_suffix("deg").unwrap_or(h_str).parse().ok()?;
    let s: f32 = s_str.strip_suffix('%')?.parse().ok()?;
    let l: f32 = l_str.strip_suffix('%')?.parse().ok()?;
    if !(0.0..=100.0).contains(&s) || !(0.0..=100.0).contains(&l) || !h.is_finite() {
        return None;
    }
    let alpha = match a_str {
        Some(a) => Some(parse_alpha_value(a)?),
        None => None,
    };

    let (r, g, b) = hsl_to_rgb(h.rem_euclid(360.0), s / 100.0, l / 100.0);
    Some((r, g, b, alpha))
}

fn parse_alpha_value(raw: &str) -> Option<f32> {
    let val = if let Some(pct) = raw.strip_suffix('%') {
        pct.parse::<f32>().ok()? / 100.0
    } else {
        raw.parse::<f32>().ok()?
    };
    if val.is_finite() && (0.0..=1.0).contains(&val) {
        Some(val)
    } else {
        None
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match h {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (u16, u8, u8) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;
    let l = (max + min) / 2.0;

    let (h, s) = if delta.abs() < f32::EPSILON {
        (0.0, 0.0)
    } else {
        let s = delta / (1.0 - (2.0 * l - 1.0).abs());
        let raw_h = if (max - rf).abs() < f32::EPSILON {
            ((gf - bf) / delta).rem_euclid(6.0)
        } else if (max - gf).abs() < f32::EPSILON {
            (bf - rf) / delta + 2.0
        } else {
            (rf - gf) / delta + 4.0
        };
        (raw_h * 60.0, s)
    };

    (
        h.round().rem_euclid(360.0) as u16,
        (s * 100.0).round().clamp(0.0, 100.0) as u8,
        (l * 100.0).round().clamp(0.0, 100.0) as u8,
    )
}

fn format_hex(r: u8, g: u8, b: u8, a: Option<f32>) -> String {
    match a {
        Some(alpha) if (alpha - 1.0).abs() > 0.002 => {
            let a_byte = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
            format!("#{r:02X}{g:02X}{b:02X}{a_byte:02X}")
        }
        _ => format!("#{r:02X}{g:02X}{b:02X}"),
    }
}

fn format_rgb(r: u8, g: u8, b: u8, a: Option<f32>) -> String {
    match a {
        Some(alpha) if (alpha - 1.0).abs() > 0.002 => format!("rgba({r}, {g}, {b}, {alpha:.2})"),
        _ => format!("rgb({r}, {g}, {b})"),
    }
}

fn format_hsl(r: u8, g: u8, b: u8, a: Option<f32>) -> String {
    let (h, s, l) = rgb_to_hsl(r, g, b);
    match a {
        Some(alpha) if (alpha - 1.0).abs() > 0.002 => format!("hsla({h}, {s}%, {l}%, {alpha:.2})"),
        _ => format!("hsl({h}, {s}%, {l}%)"),
    }
}

// ---------------------------------------------------------------------------
// 5. IPv4 CIDR Calculator
// ---------------------------------------------------------------------------

fn detect_cidr(input: &str) -> Option<SmartInsight> {
    let token = strip_token_wrappers(input);
    let (ip_str, prefix_str) = token.split_once('/')?;
    let ip: Ipv4Addr = ip_str.parse().ok()?;
    let prefix: u32 = prefix_str.parse().ok()?;
    if prefix > 32 {
        return None;
    }

    let ip_u32 = u32::from(ip);
    let mask_u32 = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    let network_u32 = ip_u32 & mask_u32;
    let broadcast_u32 = network_u32 | !mask_u32;

    let mask = Ipv4Addr::from(mask_u32);
    let network = Ipv4Addr::from(network_u32);
    let broadcast = Ipv4Addr::from(broadcast_u32);

    let (range_text, usable_hosts) = match prefix {
        32 => (network.to_string(), 1u64),
        31 => (format!("{network} – {broadcast}"), 2u64),
        _ => {
            let first = Ipv4Addr::from(network_u32 + 1);
            let last = Ipv4Addr::from(broadcast_u32 - 1);
            let count = (1u64 << (32 - prefix)) - 2;
            (format!("{first} – {last}"), count)
        }
    };

    let pill = if usable_hosts >= 1_000_000 {
        format!("{:.1}M 主机", usable_hosts as f64 / 1_000_000.0)
    } else if usable_hosts >= 10_000 {
        format!("{:.1}K 主机", usable_hosts as f64 / 1_000.0)
    } else {
        format!("{usable_hosts} 主机")
    };

    Some(SmartInsight {
        kind: InsightKind::Cidr,
        tone: InsightTone::Neutral,
        badge: "CIDR".to_string(),
        primary: range_text.clone(),
        subtitle: format!("掩码 {mask} · 广播 {broadcast} · 点击复制"),
        pill,
        copy_text: format!("{range_text} (mask {mask}, broadcast {broadcast})"),
        color_swatch: None,
        editor_json: None,
    })
}

// ---------------------------------------------------------------------------
// 6. 5-Field Cron Expression Translator + Next Run
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CronField {
    allowed: Vec<bool>,
    min: u32,
    is_wildcard: bool,
}

impl CronField {
    fn matches(&self, value: u32) -> bool {
        value >= self.min
            && self
                .allowed
                .get((value - self.min) as usize)
                .copied()
                .unwrap_or(false)
    }
}

fn detect_cron(input: &str) -> Option<SmartInsight> {
    let token = strip_token_wrappers(input);
    if token.contains('\n') || token.len() > 80 {
        return None;
    }
    let parts: Vec<&str> = token.split_whitespace().collect();
    if parts.len() != 5 {
        return None;
    }

    // Require at least one cron syntax symbol so plain 5-number sequences like "1 2 3 4 5" don't match.
    let has_cron_syntax = parts
        .iter()
        .any(|p| p.contains('*') || p.contains('/') || p.contains('-') || p.contains(','));
    if !has_cron_syntax {
        return None;
    }

    let min_f = parse_cron_field(parts[0], 0, 59, None)?;
    let hour_f = parse_cron_field(parts[1], 0, 23, None)?;
    let dom_f = parse_cron_field(parts[2], 1, 31, None)?;
    let mon_f = parse_cron_field(parts[3], 1, 12, Some(month_alias))?;
    let dow_f = parse_cron_field(parts[4], 0, 6, Some(dow_alias))?;

    let summary = describe_cron(parts[0], parts[1], parts[2], parts[3], parts[4]);
    let next_run = next_cron_occurrence(&min_f, &hour_f, &dom_f, &mon_f, &dow_f, Local::now());

    let subtitle = match next_run {
        Some(next) => format!("下次: {} · 点击复制说明", next.format("%Y-%m-%d %H:%M")),
        None => "5 字段 Cron · 点击复制说明".to_string(),
    };

    Some(SmartInsight {
        kind: InsightKind::Cron,
        tone: InsightTone::Purple,
        badge: "Cron".to_string(),
        primary: truncate_single_line(&summary, 44),
        subtitle,
        pill: "定时".to_string(),
        copy_text: summary,
        color_swatch: None,
        editor_json: None,
    })
}

fn month_alias(token: &str) -> Option<u32> {
    match token.to_ascii_uppercase().as_str() {
        "JAN" => Some(1),
        "FEB" => Some(2),
        "MAR" => Some(3),
        "APR" => Some(4),
        "MAY" => Some(5),
        "JUN" => Some(6),
        "JUL" => Some(7),
        "AUG" => Some(8),
        "SEP" => Some(9),
        "OCT" => Some(10),
        "NOV" => Some(11),
        "DEC" => Some(12),
        _ => None,
    }
}

fn dow_alias(token: &str) -> Option<u32> {
    match token.to_ascii_uppercase().as_str() {
        "SUN" | "7" => Some(0),
        "MON" => Some(1),
        "TUE" => Some(2),
        "WED" => Some(3),
        "THU" => Some(4),
        "FRI" => Some(5),
        "SAT" => Some(6),
        _ => None,
    }
}

fn parse_cron_atom(raw: &str, min: u32, max: u32, alias: Option<fn(&str) -> Option<u32>>) -> Option<u32> {
    if let Some(f) = alias
        && let Some(v) = f(raw)
    {
        return Some(v);
    }
    let val: u32 = raw.parse().ok()?;
    if val == 7 && min == 0 && max == 6 {
        return Some(0);
    }
    (min..=max).contains(&val).then_some(val)
}

fn parse_cron_field(
    expr: &str,
    min: u32,
    max: u32,
    alias: Option<fn(&str) -> Option<u32>>,
) -> Option<CronField> {
    let mut allowed = vec![false; (max - min + 1) as usize];
    let mut any = false;

    for part in expr.split(',') {
        if part.is_empty() {
            return None;
        }
        let (base, step) = match part.split_once('/') {
            Some((b, s)) => {
                let st: u32 = s.parse().ok()?;
                if st == 0 || st > (max - min + 1) {
                    return None;
                }
                (b, st)
            }
            None => (part, 1),
        };

        let (start, end) = if base == "*" {
            (min, max)
        } else if let Some((a, b)) = base.split_once('-') {
            let s = parse_cron_atom(a, min, max, alias)?;
            let e = parse_cron_atom(b, min, max, alias)?;
            if s > e {
                return None;
            }
            (s, e)
        } else {
            let single = parse_cron_atom(base, min, max, alias)?;
            if part.contains('/') {
                (single, max)
            } else {
                (single, single)
            }
        };

        let mut v = start;
        while v <= end {
            allowed[(v - min) as usize] = true;
            any = true;
            match v.checked_add(step) {
                Some(next) => v = next,
                None => break,
            }
        }
    }

    if !any {
        return None;
    }

    Some(CronField {
        allowed,
        min,
        is_wildcard: expr == "*",
    })
}

fn describe_cron(min: &str, hour: &str, dom: &str, mon: &str, dow: &str) -> String {
    let date_prefix = match (mon, dom, dow) {
        ("*", "*", "*") => "每天".to_string(),
        ("*", "*", "1-5") | ("*", "*", "MON-FRI") => "工作日".to_string(),
        ("*", "*", "0,6") | ("*", "*", "6,0") | ("*", "*", "SAT,SUN") => "周末".to_string(),
        ("*", "*", d) => format!("每周({})", format_dow_zh(d)),
        ("*", d, "*") => format!("每月 {d} 日"),
        (m, d, "*") => format!("{m} 月 {d} 日"),
        (m, d, w) => format!("{m} 月 {d} 日 (周{})", format_dow_zh(w)),
    };

    let time_part = match (hour, min) {
        ("*", "*") => "每分钟".to_string(),
        ("*", m) if m.starts_with("*/") => format!("每 {} 分钟", &m[2..]),
        (h, "0") if h.starts_with("*/") => format!("每 {} 小时整点", &h[2..]),
        (h, m) if h.starts_with("*/") => format!("每 {} 小时的第 {m} 分", &h[2..]),
        (h, m) if m.starts_with("*/") => format!("{h} 点期间每 {} 分钟", &m[2..]),
        ("*", m) => format!("每小时第 {m} 分"),
        (h, m) if h.bytes().all(|b| b.is_ascii_digit()) && m.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{:02}:{:02}", h.parse::<u32>().unwrap_or(0), m.parse::<u32>().unwrap_or(0))
        }
        (h, "0") => format!("{h} 点整"),
        (h, m) => format!("{h} 点 {m} 分"),
    };

    format!("{date_prefix} {time_part}")
}

fn format_dow_zh(dow: &str) -> String {
    match dow.to_ascii_uppercase().as_str() {
        "0" | "7" | "SUN" => "日".to_string(),
        "1" | "MON" => "一".to_string(),
        "2" | "TUE" => "二".to_string(),
        "3" | "WED" => "三".to_string(),
        "4" | "THU" => "四".to_string(),
        "5" | "FRI" => "五".to_string(),
        "6" | "SAT" => "六".to_string(),
        _ => dow.to_string(),
    }
}

fn next_cron_occurrence(
    min_f: &CronField,
    hour_f: &CronField,
    dom_f: &CronField,
    mon_f: &CronField,
    dow_f: &CronField,
    now: DateTime<Local>,
) -> Option<NaiveDateTime> {
    // Start from the next whole minute in local time.
    let mut candidate = now
        .naive_local()
        .with_second(0)?
        .with_nanosecond(0)?
        + Duration::minutes(1);
    let limit = candidate + Duration::days(366 * 4);

    while candidate <= limit {
        if !mon_f.matches(candidate.month()) {
            let (ny, nm) = if candidate.month() == 12 {
                (candidate.year() + 1, 1)
            } else {
                (candidate.year(), candidate.month() + 1)
            };
            candidate = NaiveDate::from_ymd_opt(ny, nm, 1)?.and_hms_opt(0, 0, 0)?;
            continue;
        }

        let dom_match = dom_f.matches(candidate.day());
        let dow_num = candidate.weekday().num_days_from_sunday();
        let dow_match = dow_f.matches(dow_num);
        let day_ok = match (dom_f.is_wildcard, dow_f.is_wildcard) {
            (true, true) => true,
            (false, true) => dom_match,
            (true, false) => dow_match,
            (false, false) => dom_match || dow_match,
        };
        if !day_ok {
            candidate = (candidate.date() + Duration::days(1)).and_hms_opt(0, 0, 0)?;
            continue;
        }

        if !hour_f.matches(candidate.hour()) {
            candidate = candidate
                .with_minute(0)?
                + Duration::hours(1);
            continue;
        }

        if !min_f.matches(candidate.minute()) {
            candidate += Duration::minutes(1);
            continue;
        }

        return Some(candidate);
    }

    None
}

// ---------------------------------------------------------------------------
// 7. Radix Literals (`0x`, `0b`, `0o`)
// ---------------------------------------------------------------------------

fn detect_radix(input: &str) -> Option<SmartInsight> {
    let token = strip_token_wrappers(input);
    if token.len() < 3 || token.len() > 140 || token.contains('\n') {
        return None;
    }

    let (radix, pill, digits_raw) = if let Some(rest) = token.strip_prefix("0x").or_else(|| token.strip_prefix("0X")) {
        (16, "HEX→DEC", rest)
    } else if let Some(rest) = token.strip_prefix("0b").or_else(|| token.strip_prefix("0B")) {
        (2, "BIN→DEC", rest)
    } else {
        let rest = token.strip_prefix("0o").or_else(|| token.strip_prefix("0O"))?;
        (8, "OCT→DEC", rest)
    };

    let cleaned: String = digits_raw.chars().filter(|&c| c != '_').collect();
    if cleaned.is_empty() {
        return None;
    }

    let value = u128::from_str_radix(&cleaned, radix).ok()?;
    let dec = value.to_string();

    let subtitle = match radix {
        16 => {
            if value <= 0xFFFF_FFFF {
                format!("BIN 0b{value:b} · OCT 0o{value:o} · 点击复制")
            } else {
                format!("OCT 0o{value:o} · 点击复制十进制")
            }
        }
        2 => format!("HEX 0x{value:X} · OCT 0o{value:o} · 点击复制"),
        8 => format!("HEX 0x{value:X} · BIN 0b{value:b} · 点击复制"),
        _ => "点击复制十进制".to_string(),
    };

    // If a 6-digit hex literal like `0x007AFF` is selected, also show its color swatch!
    let color_swatch = if radix == 16 && cleaned.len() == 6 {
        u32::try_from(value).ok()
    } else {
        None
    };

    Some(SmartInsight {
        kind: InsightKind::Radix,
        tone: InsightTone::Blue,
        badge: "进制".to_string(),
        primary: dec.clone(),
        subtitle: truncate_single_line(&subtitle, 46),
        pill: pill.to_string(),
        copy_text: dec,
        color_swatch,
        editor_json: None,
    })
}

// ---------------------------------------------------------------------------
// 8. Unicode Escape (`\uXXXX` / `\u{XXXXX}`)
// ---------------------------------------------------------------------------

fn detect_unicode_escape(input: &str) -> Option<SmartInsight> {
    let token = strip_token_wrappers(input);
    if !token.contains("\\u") {
        return None;
    }
    // Avoid triggering on full JSON objects/arrays where the JSON editor is the right view.
    if (token.starts_with('{') && token.ends_with('}'))
        || (token.starts_with('[') && token.ends_with(']'))
    {
        return None;
    }

    let (decoded, count) = decode_unicode_escapes(token)?;
    if count == 0 || decoded == token {
        return None;
    }

    Some(SmartInsight {
        kind: InsightKind::UnicodeEscape,
        tone: InsightTone::Blue,
        badge: "解码".to_string(),
        primary: truncate_single_line(&decoded, 44),
        subtitle: format!("解码 {count} 处 Unicode 转义 · 点击复制"),
        pill: "Unicode".to_string(),
        copy_text: decoded,
        color_swatch: None,
        editor_json: None,
    })
}

fn decode_unicode_escapes(input: &str) -> Option<(String, usize)> {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let mut decoded_count = 0;

    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && bytes[i + 1] == b'u' {
            // Rust-style `\u{1F600}`
            if i + 2 < bytes.len() && bytes[i + 2] == b'{' {
                let rest = &input[i + 3..];
                if let Some(close) = rest.find('}') {
                    let hex = &rest[..close];
                    if (1..=6).contains(&hex.len())
                        && let Ok(code) = u32::from_str_radix(hex, 16)
                        && let Some(ch) = char::from_u32(code)
                    {
                        out.push(ch);
                        decoded_count += 1;
                        i += 3 + close + 1;
                        continue;
                    }
                }
            }

            // Standard `\uXXXX` (including UTF-16 surrogate pairs)
            if i + 6 <= bytes.len()
                && let Ok(u1) = u16::from_str_radix(&input[i + 2..i + 6], 16)
            {
                if (0xD800..=0xDBFF).contains(&u1)
                    && i + 12 <= bytes.len()
                    && &bytes[i + 6..i + 8] == b"\\u"
                    && let Ok(u2) = u16::from_str_radix(&input[i + 8..i + 12], 16)
                    && (0xDC00..=0xDFFF).contains(&u2)
                {
                    let combined =
                        0x10000 + (((u1 as u32 - 0xD800) << 10) | (u2 as u32 - 0xDC00));
                    let ch = char::from_u32(combined)?;
                    out.push(ch);
                    decoded_count += 1;
                    i += 12;
                    continue;
                }
                if let Some(ch) = char::from_u32(u1 as u32) {
                    out.push(ch);
                    decoded_count += 1;
                    i += 6;
                    continue;
                }
            }
        }

        let ch = input[i..].chars().next()?;
        out.push(ch);
        i += ch.len_utf8();
    }

    Some((out, decoded_count))
}

// ---------------------------------------------------------------------------
// 9. URL Percent-Encoded Decoder
// ---------------------------------------------------------------------------

fn detect_url_encoded(input: &str) -> Option<SmartInsight> {
    let token = strip_token_wrappers(input);
    if !token.contains('%') || token.contains('\n') {
        return None;
    }

    let (decoded, pct_count) = decode_percent_utf8(token)?;
    if pct_count == 0 || decoded == token {
        return None;
    }

    let editor_json = try_pretty_json_object_or_array(&decoded);
    let pill = if editor_json.is_some() {
        "URL·JSON"
    } else {
        "URL"
    };

    Some(SmartInsight {
        kind: InsightKind::UrlEncoded,
        tone: InsightTone::Blue,
        badge: "解码".to_string(),
        primary: truncate_single_line(&decoded, 44),
        subtitle: format!("URL 解码 ({pct_count} 处转义) · 点击复制"),
        pill: pill.to_string(),
        copy_text: decoded,
        color_swatch: None,
        editor_json,
    })
}

fn decode_percent_utf8(input: &str) -> Option<(String, usize)> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut count = 0;

    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            count += 1;
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }

    let text = String::from_utf8(out).ok()?;
    Some((text, count))
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 10. Base64 / Base64URL Decoder (strict heuristics to avoid word false positives)
// ---------------------------------------------------------------------------

fn detect_base64(input: &str) -> Option<SmartInsight> {
    let token = strip_token_wrappers(input);
    if token.len() < 8 || token.contains('\n') || token.contains(' ') {
        return None;
    }

    // Must only contain standard Base64 or Base64URL characters.
    let is_valid_chars = token.bytes().all(|b| {
        b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'-' | b'_' | b'=')
    });
    if !is_valid_chars || token.len() % 4 == 1 {
        return None;
    }

    let decoded_bytes = decode_base64_flex(token, false)?;
    if decoded_bytes.is_empty() || decoded_bytes.contains(&0) {
        return None;
    }
    let decoded = String::from_utf8(decoded_bytes).ok()?;
    if !is_readable_text(&decoded) {
        return None;
    }

    let editor_json = try_pretty_json_object_or_array(&decoded);

    // Anti-false-positive gate: plain English words like "username" or "component" are valid
    // Base64 strings of length 8/12. Require strong Base64 evidence unless it decoded to JSON.
    if editor_json.is_none() {
        let has_padding = token.ends_with('=');
        let has_b64_sym = token.bytes().any(|b| matches!(b, b'+' | b'/' | b'-' | b'_'));
        let has_upper = token.bytes().any(|b| b.is_ascii_uppercase());
        let has_lower = token.bytes().any(|b| b.is_ascii_lowercase());
        let has_digit = token.bytes().any(|b| b.is_ascii_digit());
        let mixed_strong = has_upper && has_lower && has_digit;

        if !has_padding && !has_b64_sym && !mixed_strong {
            return None;
        }
    }

    let pill = if editor_json.is_some() {
        "Base64·JSON"
    } else {
        "Base64"
    };

    Some(SmartInsight {
        kind: InsightKind::Base64,
        tone: InsightTone::Blue,
        badge: "解码".to_string(),
        primary: truncate_single_line(&decoded, 44),
        subtitle: format!("Base64 解码 ({} 字节) · 点击复制", decoded.len()),
        pill: pill.to_string(),
        copy_text: decoded,
        color_swatch: None,
        editor_json,
    })
}

fn try_pretty_json_object_or_array(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return None;
    }
    let val: Value = serde_json::from_str(trimmed).ok()?;
    if matches!(val, Value::Object(_) | Value::Array(_)) {
        serde_json::to_string_pretty(&val).ok()
    } else {
        None
    }
}

fn is_readable_text(s: &str) -> bool {
    let total = s.chars().count();
    if total == 0 {
        return false;
    }
    let readable = s
        .chars()
        .filter(|&c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .count();
    readable * 10 >= total * 9
}

fn decode_base64_flex(input: &str, url_safe_only: bool) -> Option<Vec<u8>> {
    let trimmed = input.trim_end_matches('=');
    let pad_len = input.len() - trimmed.len();
    if pad_len > 2 || trimmed.contains('=') {
        return None;
    }
    if pad_len > 0 && !input.len().is_multiple_of(4) {
        return None;
    }
    if trimmed.len() % 4 == 1 {
        return None;
    }

    let mut out = Vec::with_capacity((trimmed.len() * 3) / 4);
    let bytes = trimmed.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let remain = bytes.len() - i;
        let v0 = b64_val(bytes[i], url_safe_only)?;
        let v1 = b64_val(bytes[i + 1], url_safe_only)?;
        out.push((v0 << 2) | (v1 >> 4));

        if remain >= 3 {
            let v2 = b64_val(bytes[i + 2], url_safe_only)?;
            out.push((v1 << 4) | (v2 >> 2));
            if remain >= 4 {
                let v3 = b64_val(bytes[i + 3], url_safe_only)?;
                out.push((v2 << 6) | v3);
                i += 4;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    Some(out)
}

fn b64_val(b: u8, url_safe_only: bool) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        b'+' if !url_safe_only => Some(62),
        b'/' if !url_safe_only => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_unix_timestamp_and_reverse_datetime() {
        let ts = detect_smart_insight("1732619278000").expect("timestamp insight");
        assert_eq!(ts.kind, InsightKind::Timestamp);
        assert_eq!(ts.pill, "毫秒");

        let dt = detect_smart_insight("2024-11-26T11:07:58Z").expect("iso datetime insight");
        assert_eq!(dt.kind, InsightKind::DateTime);
        assert_eq!(dt.primary, "1732619278");
        assert_eq!(dt.copy_text, "1732619278");

        let local_dt = detect_smart_insight("2026-11-26 19:48:00").expect("local datetime");
        assert_eq!(local_dt.kind, InsightKind::DateTime);
        assert!(local_dt.primary.parse::<i64>().is_ok());

        let date_only = detect_smart_insight("2026-11-26").expect("date only");
        assert_eq!(date_only.kind, InsightKind::DateTime);
        assert_eq!(date_only.pill, "零点→秒");
    }

    #[test]
    fn detects_jwt_with_optional_bearer_prefix() {
        // Header: {"alg":"HS256","typ":"JWT"}
        // Payload: {"sub":"u_1024","exp":4102444800}
        let jwt = "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiJ1XzEwMjQiLCJleHAiOjQxMDI0NDQ4MDB9.sig123";
        let insight = detect_smart_insight(jwt).expect("jwt insight");
        assert_eq!(insight.kind, InsightKind::Jwt);
        assert_eq!(insight.pill, "HS256");
        assert_eq!(insight.tone, InsightTone::Green);
        assert!(insight.primary.contains("有效"));
        assert!(insight.primary.contains("sub: u_1024"));
        assert!(insight.editor_json.as_deref().unwrap().contains("\"header\""));
        assert!(insight.copy_text.contains("\"u_1024\""));

        // Expired JWT: {"sub":"old","exp":1000000000}
        let expired = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJvbGQiLCJleHAiOjEwMDAwMDAwMDB9.sig";
        let expired_insight = detect_smart_insight(expired).expect("expired jwt");
        assert_eq!(expired_insight.tone, InsightTone::Amber);
        assert!(expired_insight.primary.contains("已过期"));
    }

    #[test]
    fn detects_hex_rgb_and_hsl_colors() {
        let hex = detect_smart_insight("#007AFF").expect("hex color");
        assert_eq!(hex.kind, InsightKind::Color);
        assert_eq!(hex.primary, "rgb(0, 122, 255)");
        assert_eq!(hex.color_swatch, Some(0x007AFF));

        let rgb = detect_smart_insight("rgb(0, 122, 255)").expect("rgb color");
        assert_eq!(rgb.kind, InsightKind::Color);
        assert_eq!(rgb.primary, "#007AFF");
        assert_eq!(rgb.color_swatch, Some(0x007AFF));

        let hsl = detect_smart_insight("hsl(211, 100%, 50%)").expect("hsl color");
        assert_eq!(hsl.kind, InsightKind::Color);
        assert_eq!(hsl.pill, "HSL→HEX");
        assert!(hsl.color_swatch.is_some());
    }

    #[test]
    fn detects_cidr_subnet() {
        let cidr = detect_smart_insight("10.0.1.42/24").expect("cidr");
        assert_eq!(cidr.kind, InsightKind::Cidr);
        assert_eq!(cidr.primary, "10.0.1.1 – 10.0.1.254");
        assert_eq!(cidr.pill, "254 主机");
        assert!(cidr.subtitle.contains("255.255.255.0"));
    }

    #[test]
    fn detects_cron_and_rejects_plain_numbers() {
        let cron = detect_smart_insight("*/15 9-18 * * 1-5").expect("cron");
        assert_eq!(cron.kind, InsightKind::Cron);
        assert!(cron.primary.contains("工作日"));
        assert!(cron.primary.contains("每 15 分钟"));
        assert!(cron.subtitle.contains("下次:"));

        assert!(detect_smart_insight("1 2 3 4 5").is_none());
    }

    #[test]
    fn detects_radix_unicode_url_and_base64() {
        let hex_num = detect_smart_insight("0x1F4").expect("hex radix");
        assert_eq!(hex_num.kind, InsightKind::Radix);
        assert_eq!(hex_num.primary, "500");

        let bin_num = detect_smart_insight("0b1010_0101").expect("bin radix");
        assert_eq!(bin_num.kind, InsightKind::Radix);
        assert_eq!(bin_num.primary, "165");

        let uni = detect_smart_insight(r#""\u4f60\u597d\uD83D\uDE00""#).expect("unicode escape");
        assert_eq!(uni.kind, InsightKind::UnicodeEscape);
        assert_eq!(uni.copy_text, "你好😀");

        let url = detect_smart_insight("https://example.com/%E4%B8%AD%E6%96%87?k=1").expect("url");
        assert_eq!(url.kind, InsightKind::UrlEncoded);
        assert_eq!(url.copy_text, "https://example.com/中文?k=1");

        // Base64 for "Hello, RPanel!" -> "SGVsbG8sIFJQYW5lbCE="
        let b64 = detect_smart_insight("SGVsbG8sIFJQYW5lbCE=").expect("base64");
        assert_eq!(b64.kind, InsightKind::Base64);
        assert_eq!(b64.copy_text, "Hello, RPanel!");

        // Base64 for JSON `{"env":"prod"}` -> "eyJlbnYiOiJwcm9kIn0="
        let b64_json = detect_smart_insight("eyJlbnYiOiJwcm9kIn0=").expect("base64 json");
        assert_eq!(b64_json.kind, InsightKind::Base64);
        assert_eq!(b64_json.pill, "Base64·JSON");
        assert!(b64_json.editor_json.is_some());
    }

    #[test]
    fn avoids_false_positives_on_regular_words_and_json() {
        assert!(detect_smart_insight("username").is_none());
        assert!(detect_smart_insight("localhost").is_none());
        assert!(detect_smart_insight("component").is_none());
        assert!(detect_smart_insight(r#"{"name": "RPanel"}"#).is_none());
        assert!(detect_smart_insight("13800138000").is_none());
    }
}
