pub mod editor;
pub mod json_highlight;
pub mod panel;
pub mod quick_menu;
pub mod status_bar;
pub mod toolbar;

use json_engine::JsonEngineError;

/// Convert a `serde_json` syntax error message into concise Chinese without redundant line/col suffixes.
pub fn localize_json_error_message(raw: &str) -> String {
    let core = strip_line_col_suffix(raw).trim();
    match core {
        "expected value" => "期望有效值".to_string(),
        "trailing comma" => "存在多余的尾随逗号".to_string(),
        "key must be a string" => "对象的键必须是双引号字符串".to_string(),
        "expected `:`" => "缺少冒号 `:`".to_string(),
        "expected `,` or `}`" => "缺少逗号 `,` 或右花括号 `}`".to_string(),
        "expected `,` or `]`" => "缺少逗号 `,` 或右方括号 `]`".to_string(),
        "EOF while parsing a value" => "内容不完整（缺少值）".to_string(),
        "EOF while parsing an object" => "对象未闭合（缺少 `}`）".to_string(),
        "EOF while parsing a list" => "数组未闭合（缺少 `]`）".to_string(),
        "EOF while parsing a string" => "字符串未闭合（缺少 `\"`）".to_string(),
        "trailing characters" => "JSON 末尾存在多余字符".to_string(),
        "invalid escape" => "无效的转义字符".to_string(),
        "invalid unicode code point" => "无效的 Unicode 编码".to_string(),
        "invalid number" => "无效的数字格式".to_string(),
        "expected ident" => "无效的标识符".to_string(),
        _ if core.starts_with("control character") => "字符串中包含未转义的控制字符".to_string(),
        _ if core.starts_with("lone leading surrogate") => "无效的 Unicode 代理字符".to_string(),
        _ if core.starts_with("number out of range") => "数字超出有效范围".to_string(),
        _ => core.to_string(),
    }
}

/// Format a [`JsonEngineError`] into a concise, localized Chinese message.
pub fn format_engine_error(err: &JsonEngineError) -> String {
    match err {
        JsonEngineError::InvalidJson {
            line,
            column,
            message,
        } => format!(
            "第 {line} 行，第 {column} 列 · {}",
            localize_json_error_message(message)
        ),
        JsonEngineError::Serialize(msg) => format!("序列化出错（{msg}）"),
        JsonEngineError::Unescape(msg) => {
            format!("不是有效的转义字符串（{}）", localize_json_error_message(msg))
        }
    }
}

fn strip_line_col_suffix(message: &str) -> &str {
    if let Some((prefix, suffix)) = message.rsplit_once(" at line ") {
        let mut parts = suffix.split_whitespace();
        let line_ok = parts
            .next()
            .is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()));
        let col_word = parts.next() == Some("column");
        let col_ok = parts
            .next()
            .is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()));
        if line_ok && col_word && col_ok && parts.next().is_none() {
            return prefix;
        }
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localizes_common_serde_json_errors() {
        assert_eq!(
            localize_json_error_message("trailing comma at line 3 column 1"),
            "存在多余的尾随逗号"
        );
        assert_eq!(
            localize_json_error_message("expected `:` at line 2 column 8"),
            "缺少冒号 `:`"
        );
        assert_eq!(
            localize_json_error_message("EOF while parsing an object at line 5 column 1"),
            "对象未闭合（缺少 `}`）"
        );
        assert_eq!(
            localize_json_error_message("custom unknown error at line 1 column 2"),
            "custom unknown error"
        );
    }
}
