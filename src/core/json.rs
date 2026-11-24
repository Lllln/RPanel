use std::{fmt, ops::Range};

use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, SeqAccess, Visitor},
};

pub const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_DEPTH: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Validate,
    Format,
    Compact,
}

#[derive(Debug)]
pub enum Output {
    Formatted { text: String, repaired: bool },
    Compacted(String),
}

#[derive(Debug)]
pub struct Report {
    pub issue: Option<Issue>,
    pub folds: Vec<Fold>,
    pub output: Option<Output>,
}

#[derive(Debug)]
pub enum Issue {
    InputLimit,
    FormatLimit,
    Syntax(Diagnostic),
}

#[derive(Debug)]
pub struct Diagnostic {
    pub byte: usize,
    pub line: usize,
    pub column: usize,
    pub message: String,
    pub excerpt: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Fold {
    pub open: usize,
    pub close: usize,
}

impl Fold {
    pub fn interior(self) -> Range<usize> {
        self.open + 1..self.close
    }
}

pub fn process(source: &str, operation: Operation) -> Report {
    if source.len() > MAX_BYTES {
        return Report {
            issue: Some(Issue::InputLimit),
            folds: Vec::new(),
            output: None,
        };
    }
    let validation = validate(source);
    if operation == Operation::Format {
        return format(source, validation);
    }
    let (issue, output) = match validation {
        Ok(()) if operation == Operation::Compact => {
            let mut compact = String::with_capacity(source.len());
            for token in tokens(source) {
                compact.push_str(&source[token.bytes]);
            }
            (None, Some(Output::Compacted(compact)))
        }
        Ok(()) => (None, None),
        Err(error) => (Some(Issue::Syntax(Diagnostic::new(source, error))), None),
    };
    Report {
        issue,
        folds: fold_ranges(source),
        output,
    }
}

fn format(source: &str, validation: serde_json::Result<()>) -> Report {
    let mut issue = None;
    let repaired = match validation {
        Ok(()) => None,
        Err(error) => {
            issue = Some(Issue::Syntax(Diagnostic::new(source, error)));
            repair_newlines(source).filter(|candidate| validate(candidate).is_ok())
        }
    };

    if (issue.is_none() || repaired.is_some())
        && let Some(text) = pretty(repaired.as_deref().unwrap_or(source))
    {
        return Report {
            issue: None,
            folds: fold_ranges(&text),
            output: Some(Output::Formatted {
                text,
                repaired: repaired.is_some(),
            }),
        };
    }
    Report {
        issue: issue.or(Some(Issue::FormatLimit)),
        folds: fold_ranges(source),
        output: None,
    }
}

fn validate(source: &str) -> serde_json::Result<()> {
    serde_json::from_str::<CheckedValue>(source).map(|_| ())
}

// 逐项校验，不构造会合并重复键的 Value。保留 serde_json 的字符串检查、
// 任意精度数字语法和默认递归限制，输出直接使用原始文本中的记号。
struct CheckedValue;

impl<'de> Deserialize<'de> for CheckedValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(Self)
    }
}

impl<'de> Visitor<'de> for CheckedValue {
    type Value = Self;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_unit<E>(self) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_bool<E>(self, _: bool) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_i64<E>(self, _: i64) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_u64<E>(self, _: u64) -> Result<Self, E> {
        Ok(self)
    }
    fn visit_str<E>(self, _: &str) -> Result<Self, E> {
        Ok(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut values: A) -> Result<Self, A::Error> {
        while values.next_element::<Self>()?.is_some() {}
        Ok(self)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut fields: A) -> Result<Self, A::Error> {
        while fields.next_entry::<String, Self>()?.is_some() {}
        Ok(self)
    }
}

impl Diagnostic {
    fn new(source: &str, error: serde_json::Error) -> Self {
        let line_start = match error.line() {
            0 | 1 => 0,
            line => source
                .match_indices('\n')
                .nth(line - 2)
                .map_or(source.len(), |(i, _)| i + 1),
        };
        let byte = if error.is_eof() {
            source.len()
        } else {
            source.floor_char_boundary(
                source
                    .len()
                    .min(line_start + error.column().saturating_sub(1)),
            )
        };
        let prefix = &source[..byte];
        let line = prefix.bytes().filter(|&byte| byte == b'\n').count() + 1;
        let column = prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        let excerpt_start = prefix
            .char_indices()
            .rev()
            .nth(23)
            .map_or(0, |(offset, _)| offset);
        let excerpt_end = source[byte..]
            .char_indices()
            .nth(24)
            .map_or(source.len(), |(offset, _)| byte + offset);
        let excerpt = source[excerpt_start..excerpt_end]
            .replace('\r', "\\r")
            .replace('\n', "\\n");
        let message = error.to_string();
        let message = message
            .split_once(" at line ")
            .map_or(message.as_str(), |(reason, _)| reason)
            .to_owned();
        Self {
            byte,
            line,
            column,
            message,
            excerpt,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Key,
    StringValue,
    Number,
    Boolean,
    Null,
    ObjectOpen,
    ObjectClose,
    ArrayOpen,
    ArrayClose,
    Colon,
    Comma,
    Other,
}

#[derive(Debug)]
pub struct Token {
    pub kind: Kind,
    pub bytes: Range<usize>,
}

pub fn tokens(source: &str) -> impl Iterator<Item = Token> + '_ {
    let bytes = source.as_bytes();
    let mut offset = 0;
    std::iter::from_fn(move || {
        while bytes.get(offset).is_some_and(|&byte| whitespace(byte)) {
            offset += 1;
        }
        let start = offset;
        let first = *bytes.get(offset)?;
        offset += 1;
        let kind = match first {
            b'{' => Kind::ObjectOpen,
            b'}' => Kind::ObjectClose,
            b'[' => Kind::ArrayOpen,
            b']' => Kind::ArrayClose,
            b':' => Kind::Colon,
            b',' => Kind::Comma,
            b'"' => {
                offset = string_end(bytes, offset);
                if bytes[offset..].iter().find(|&&byte| !whitespace(byte)) == Some(&b':') {
                    Kind::Key
                } else {
                    Kind::StringValue
                }
            }
            b'-' | b'0'..=b'9' => {
                while bytes.get(offset).is_some_and(|byte| {
                    matches!(byte, b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                }) {
                    offset += 1;
                }
                Kind::Number
            }
            b'a'..=b'z' | b'A'..=b'Z' => {
                while bytes.get(offset).is_some_and(u8::is_ascii_alphabetic) {
                    offset += 1;
                }
                match &source[start..offset] {
                    "true" | "false" => Kind::Boolean,
                    "null" => Kind::Null,
                    _ => Kind::Other,
                }
            }
            _ => {
                offset = source.ceil_char_boundary(offset);
                Kind::Other
            }
        };
        Some(Token {
            kind,
            bytes: start..offset,
        })
    })
}

fn whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

fn string_end(bytes: &[u8], mut offset: usize) -> usize {
    while let Some(&byte) = bytes.get(offset) {
        offset += 1;
        match byte {
            b'"' => break,
            b'\\' => offset = (offset + 1).min(bytes.len()),
            _ => {}
        }
    }
    offset
}

fn repair_newlines(source: &str) -> Option<String> {
    let mut repaired = String::new();
    let mut copied = 0;
    let (mut quoted, mut escaped) = (false, false);
    for (offset, byte) in source.bytes().enumerate() {
        if escaped {
            // 反斜杠紧接真实换行时无法确定原意，保留原始错误。
            if matches!(byte, b'\r' | b'\n') {
                return None;
            }
            escaped = false;
            continue;
        }
        match byte {
            b'"' => quoted = !quoted,
            b'\\' if quoted => escaped = true,
            b'\r' | b'\n' if quoted => {
                repaired.push_str(&source[copied..offset]);
                repaired.push_str(if byte == b'\r' { "\\r" } else { "\\n" });
                copied = offset + 1;
                if repaired.len() + source.len() - copied > MAX_BYTES {
                    return None;
                }
            }
            _ => {}
        }
    }
    if quoted || copied == 0 {
        return None;
    }
    repaired.push_str(&source[copied..]);
    Some(repaired)
}

fn pretty(source: &str) -> Option<String> {
    use Kind::*;
    const INDENT: &[u8] = &[b' '; MAX_DEPTH * 2];
    let mut output = String::with_capacity(source.len());
    let (mut depth, mut previous) = (0, None);
    for token in tokens(source) {
        let close = matches!(token.kind, ObjectClose | ArrayClose);
        let follows_open = matches!(previous, Some(ObjectOpen | ArrayOpen));
        if close {
            depth -= 1;
        }
        let value = &source[token.bytes];
        if (close && !follows_open) || (!close && follows_open) || previous == Some(Comma) {
            if output.len() + 1 + 2 * depth + value.len() > MAX_BYTES {
                return None;
            }
            output.push('\n');
            output.push_str(std::str::from_utf8(&INDENT[..2 * depth]).expect("indent is ASCII"));
        } else if previous == Some(Colon) {
            output.push(' ');
        }
        if output.len() + value.len() > MAX_BYTES {
            return None;
        }
        output.push_str(value);
        if matches!(token.kind, ObjectOpen | ArrayOpen) {
            depth += 1;
        }
        previous = Some(token.kind);
    }
    Some(output)
}

fn fold_ranges(source: &str) -> Vec<Fold> {
    use Kind::*;
    let mut stack = Vec::new();
    let mut folds = Vec::new();
    let mut overflow = 0;
    let mut newlines = source.match_indices('\n').peekable();
    let mut last_newline = None;
    for token in tokens(source) {
        match token.kind {
            ObjectOpen | ArrayOpen if stack.len() < MAX_DEPTH => {
                stack.push((token.kind, token.bytes.start))
            }
            ObjectOpen | ArrayOpen => overflow += 1,
            ObjectClose | ArrayClose if overflow > 0 => overflow -= 1,
            ObjectClose | ArrayClose => {
                let expected = if token.kind == ObjectClose {
                    ObjectOpen
                } else {
                    ArrayOpen
                };
                let Some((kind, open)) = stack.pop() else {
                    continue;
                };
                if kind != expected {
                    stack.clear();
                    continue;
                }
                while let Some(&(line, _)) = newlines
                    .peek()
                    .filter(|&&(line, _)| line < token.bytes.start)
                {
                    last_newline = Some(line);
                    newlines.next();
                }
                if last_newline.is_some_and(|line| line > open) {
                    folds.push(Fold {
                        open,
                        close: token.bytes.start,
                    });
                }
            }
            _ => {}
        }
    }
    folds.sort_unstable();
    folds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formatted(source: &str) -> (String, bool) {
        let report = process(source, Operation::Format);
        match report.output {
            Some(Output::Formatted { text, repaired }) => (text, repaired),
            _ => panic!("expected formatted JSON: {:?}", report.issue),
        }
    }

    fn syntax(report: Report) -> Diagnostic {
        assert!(report.output.is_none());
        match report.issue {
            Some(Issue::Syntax(diagnostic)) => diagnostic,
            other => panic!("expected syntax error: {other:?}"),
        }
    }

    #[test]
    fn formats_and_compacts_without_rewriting_tokens() {
        let source = r#"{ "z" : 123456789012345678901234567890, "a": -0, "z": 1E+99999, "$serde_json::private::Number": [ "\u4E2D\n\\\"", -2.30e-2, true, null, {}, [] ] }"#;
        let expected = r#"{"z":123456789012345678901234567890,"a":-0,"z":1E+99999,"$serde_json::private::Number":["\u4E2D\n\\\"",-2.30e-2,true,null,{},[]]}"#;
        let (pretty, repaired) = formatted(source);
        assert!(!repaired);
        assert!(pretty.starts_with("{\n  \"z\":"));
        assert!(pretty.contains("\n    -2.30e-2,\n"));
        for input in [source, &pretty] {
            let report = process(input, Operation::Compact);
            assert!(matches!(report.output, Some(Output::Compacted(text)) if text == expected));
        }
    }

    #[test]
    fn accepts_scalars_and_structure_whitespace() {
        for scalar in ["\r\n  null\t", "\"字\"", "true", "-2.50e+10000"] {
            assert_eq!(formatted(scalar), (scalar.trim().to_owned(), false));
        }
        assert_eq!(
            formatted("[\r\n\r{ \"x\" : false }\r\n]").0,
            "[\n  {\n    \"x\": false\n  }\n]"
        );
    }

    #[test]
    fn repairs_only_literal_newlines_in_closed_strings() {
        let source = "{\r\n\"a\":\"one\r\ntwo\rthree\nfour\\n\\\"end\"\r\n}";
        let (pretty, repaired) = formatted(source);
        assert!(repaired);
        assert_eq!(
            pretty,
            "{\n  \"a\": \"one\\r\\ntwo\\rthree\\nfour\\n\\\"end\"\n}"
        );
        assert!(process(source, Operation::Compact).output.is_none());
    }

    #[test]
    fn ambiguous_repairs_and_other_syntax_errors_leave_original_diagnostic() {
        for source in [
            "\"open\nquote",
            "\"backslash\\\ncontinued\"",
            "[\"fix\nme\",1\n2]",
            "\"fix\nme\" null",
            "[01]",
            "[NaN]",
            r#""\uD800""#,
        ] {
            let original = syntax(process(source, Operation::Validate));
            let after = syntax(process(source, Operation::Format));
            assert_eq!(
                (original.byte, original.message),
                (after.byte, after.message)
            );
        }
    }

    #[test]
    fn reports_unicode_columns_excerpt_and_eof() {
        let source = "{\r\n\"汉字\": @\r\n}";
        let error = syntax(process(source, Operation::Validate));
        assert_eq!(error.byte, source.find('@').unwrap());
        assert_eq!((error.line, error.column), (2, 7));
        assert!(error.excerpt.contains("汉字"));
        let end = syntax(process("{\n", Operation::Validate));
        assert_eq!((end.byte, end.line, end.column), (2, 2, 1));
    }

    #[test]
    fn bounds_input_output_and_depth() {
        let oversized = " ".repeat(MAX_BYTES + 1);
        assert!(matches!(
            process(&oversized, Operation::Format).issue,
            Some(Issue::InputLimit)
        ));
        let large = format!("[{}0]", "0,".repeat(MAX_BYTES / 3));
        assert!(large.len() < MAX_BYTES);
        assert!(matches!(
            process(&large, Operation::Format).issue,
            Some(Issue::FormatLimit)
        ));
        let deep = format!("{}0{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        let problem = syntax(process(&deep, Operation::Validate));
        assert!(problem.message.contains("recursion"));
        assert!(
            process(&deep[1..deep.len() - 1], Operation::Validate)
                .issue
                .is_none()
        );
    }

    #[test]
    fn discovers_nested_folds_even_when_json_has_a_value_error() {
        let source = "{\n  \"k\": [\n    \"[{}]\", @\n  ],\n  \"inline\": {}\n}";
        let report = process(source, Operation::Validate);
        assert!(report.issue.is_some());
        assert_eq!(report.folds.len(), 2);
        assert_eq!(
            report.folds[0],
            Fold {
                open: 0,
                close: source.len() - 1
            }
        );
        assert_eq!(&source[report.folds[1].interior()], "\n    \"[{}]\", @\n  ");
    }
}
