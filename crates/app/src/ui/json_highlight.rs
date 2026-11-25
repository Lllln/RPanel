use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory, Rope,
};
use gpui_kit::{Context, HighlightStyle, SharedString, Window, rgb};

/// Restrained Apple-inspired light-mode JSON syntax colors.
mod syntax_colors {
    pub const KEY: u32 = 0x7440A0;
    pub const STRING: u32 = 0x1E7A4C;
    pub const NUMBER: u32 = 0x9C5219;
    pub const KEYWORD: u32 = 0x0066CC;
    pub const PUNCTUATION: u32 = 0x6E6E73;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Key,
    String,
    Number,
    Keyword,
    Punctuation,
}

impl TokenKind {
    fn highlight_style(self) -> HighlightStyle {
        let color = match self {
            Self::Key => syntax_colors::KEY,
            Self::String => syntax_colors::STRING,
            Self::Number => syntax_colors::NUMBER,
            Self::Keyword => syntax_colors::KEYWORD,
            Self::Punctuation => syntax_colors::PUNCTUATION,
        };
        HighlightStyle {
            color: Some(rgb(color).into()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    range: Range<usize>,
    kind: TokenKind,
}

/// Pure-Rust JSON syntax highlighter and multi-line brace/bracket folder.
///
/// Replaces `tree-sitter` C grammar highlighting to guarantee memory safety and zero
/// `0xC0000005` crashes on Windows/VDI while keeping crisp syntax colors and folding.
#[derive(Default)]
pub struct JsonHighlighter {
    tokens: Vec<Token>,
    folds: Vec<FoldRange>,
}

/// Factory installing [`JsonHighlighter`] on GPUI `EditorState`.
pub fn json_highlighter_factory() -> InputHighlighterFactory {
    Rc::new(|_language| Some(Box::new(JsonHighlighter::default())))
}

impl JsonHighlighter {
    fn reparse(&mut self, source: &str) {
        self.tokens.clear();
        self.folds.clear();
        tokenize_and_fold(source, &mut self.tokens, &mut self.folds);
    }

    fn styles_for_range(&self, range: &Range<usize>) -> Vec<(Range<usize>, HighlightStyle)> {
        if range.start >= range.end {
            return Vec::new();
        }

        let mut runs = Vec::new();
        let mut cursor = range.start;
        let first_idx = self.tokens.partition_point(|tok| tok.range.end <= range.start);

        for tok in &self.tokens[first_idx..] {
            if tok.range.start >= range.end {
                break;
            }
            let seg_start = tok.range.start.max(cursor);
            let seg_end = tok.range.end.min(range.end);
            if seg_start > cursor {
                runs.push((cursor..seg_start, HighlightStyle::default()));
            }
            if seg_end > seg_start {
                runs.push((seg_start..seg_end, tok.kind.highlight_style()));
                cursor = seg_end;
            }
        }

        if cursor < range.end {
            runs.push((cursor..range.end, HighlightStyle::default()));
        }

        runs
    }
}

impl InputHighlighter for JsonHighlighter {
    fn language(&self) -> SharedString {
        SharedString::from("json")
    }

    fn update(
        &mut self,
        _edit: Option<InputEdit>,
        text: &Rope,
        folding: bool,
        _window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        let source = text.to_string();
        self.reparse(&source);
        if folding {
            let folds = self.folds.clone();
            let entity = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = entity.update(cx, |state, cx| {
                    state.apply_highlighter_fold_candidates(folds, cx);
                });
            });
        }
    }

    fn update_batch(
        &mut self,
        edits: &[(InputEdit, Rope)],
        folding: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        if let Some((_, latest_text)) = edits.last() {
            self.update(None, latest_text, folding, window, cx);
        }
    }

    fn styles(
        &self,
        range: &Range<usize>,
        _resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        self.styles_for_range(range)
    }

    fn fold_ranges(&self, text: &Rope) -> Vec<FoldRange> {
        if !self.folds.is_empty() || text.len() == 0 {
            return self.folds.clone();
        }
        let mut tokens = Vec::new();
        let mut folds = Vec::new();
        tokenize_and_fold(&text.to_string(), &mut tokens, &mut folds);
        folds
    }
}

fn tokenize_and_fold(src: &str, tokens: &mut Vec<Token>, folds: &mut Vec<FoldRange>) {
    let bytes = src.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut line = 0usize;
    let mut stack: Vec<(u8, usize)> = Vec::new();

    while i < len {
        let b = bytes[i];
        match b {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b' ' | b'\t' | b'\r' => {
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < len {
                    match bytes[i] {
                        b'\\' => {
                            i += 1;
                            if i < len {
                                if bytes[i] == b'\n' {
                                    line += 1;
                                }
                                i += 1;
                            }
                        }
                        b'"' => {
                            i += 1;
                            break;
                        }
                        b'\n' => {
                            line += 1;
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
                let end = i;

                // Look ahead past whitespace to check if this string is an object key (`"key":`).
                let mut look = end;
                while look < len && matches!(bytes[look], b' ' | b'\t' | b'\r' | b'\n') {
                    look += 1;
                }
                let kind = if look < len && bytes[look] == b':' {
                    TokenKind::Key
                } else {
                    TokenKind::String
                };
                tokens.push(Token {
                    range: start..end,
                    kind,
                });
            }
            b'{' | b'[' => {
                stack.push((b, line));
                tokens.push(Token {
                    range: i..i + 1,
                    kind: TokenKind::Punctuation,
                });
                i += 1;
            }
            b'}' | b']' => {
                let expected = if b == b'}' { b'{' } else { b'[' };
                if let Some((open, start_line)) = stack.last().copied()
                    && open == expected
                {
                    stack.pop();
                    if line > start_line {
                        folds.push(FoldRange::new(start_line, line));
                    }
                }
                tokens.push(Token {
                    range: i..i + 1,
                    kind: TokenKind::Punctuation,
                });
                i += 1;
            }
            b':' | b',' => {
                tokens.push(Token {
                    range: i..i + 1,
                    kind: TokenKind::Punctuation,
                });
                i += 1;
            }
            b'-' | b'0'..=b'9' => {
                let start = i;
                if bytes[i] == b'-' {
                    i += 1;
                }
                while i < len && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if i < len && bytes[i] == b'.' {
                    i += 1;
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                if i < len && matches!(bytes[i], b'e' | b'E') {
                    i += 1;
                    if i < len && matches!(bytes[i], b'+' | b'-') {
                        i += 1;
                    }
                    while i < len && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                if i > start {
                    tokens.push(Token {
                        range: start..i,
                        kind: TokenKind::Number,
                    });
                }
            }
            b't' if bytes[i..].starts_with(b"true") && is_word_boundary(bytes, i + 4) => {
                tokens.push(Token {
                    range: i..i + 4,
                    kind: TokenKind::Keyword,
                });
                i += 4;
            }
            b'f' if bytes[i..].starts_with(b"false") && is_word_boundary(bytes, i + 5) => {
                tokens.push(Token {
                    range: i..i + 5,
                    kind: TokenKind::Keyword,
                });
                i += 5;
            }
            b'n' if bytes[i..].starts_with(b"null") && is_word_boundary(bytes, i + 4) => {
                tokens.push(Token {
                    range: i..i + 4,
                    kind: TokenKind::Keyword,
                });
                i += 4;
            }
            _ => {
                // Advance by one full UTF-8 character so offsets always stay on char boundaries.
                let ch_len = src[i..].chars().next().map_or(1, char::len_utf8);
                i += ch_len;
            }
        }
    }

    folds.sort_by_key(|f| (f.start_line, f.end_line));
}

fn is_word_boundary(bytes: &[u8], idx: usize) -> bool {
    idx >= bytes.len() || !(bytes[idx].is_ascii_alphanumeric() || bytes[idx] == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_keys_values_numbers_keywords_and_folds() {
        let src = "{\n  \"name\": \"中文JSON\",\n  \"count\": -42.5e1,\n  \"ok\": true,\n  \"empty\": null,\n  \"items\": [\n    1,\n    2\n  ]\n}";
        let mut hl = JsonHighlighter::default();
        hl.reparse(src);

        assert!(hl.tokens.iter().any(|t| t.kind == TokenKind::Key && &src[t.range.clone()] == "\"name\""));
        assert!(hl.tokens.iter().any(|t| t.kind == TokenKind::String && &src[t.range.clone()] == "\"中文JSON\""));
        assert!(hl.tokens.iter().any(|t| t.kind == TokenKind::Number && &src[t.range.clone()] == "-42.5e1"));
        assert!(hl.tokens.iter().any(|t| t.kind == TokenKind::Keyword && &src[t.range.clone()] == "true"));
        assert!(hl.tokens.iter().any(|t| t.kind == TokenKind::Keyword && &src[t.range.clone()] == "null"));

        assert_eq!(
            hl.folds,
            vec![FoldRange::new(0, 9), FoldRange::new(5, 8)]
        );

        // Verify styles cover the entire requested byte range continuously without overlap.
        let runs = hl.styles_for_range(&(0..src.len()));
        let mut cursor = 0usize;
        for (r, _) in &runs {
            assert_eq!(r.start, cursor);
            assert!(r.end > r.start);
            cursor = r.end;
        }
        assert_eq!(cursor, src.len());
    }

    #[test]
    fn handles_incomplete_invalid_json_safely() {
        let broken = "{\n  \"unterminated: \"abc\\\"def\n  中文垃圾,";
        let mut hl = JsonHighlighter::default();
        hl.reparse(broken);
        let runs = hl.styles_for_range(&(0..broken.len()));
        assert!(!runs.is_empty());
        assert_eq!(runs.first().unwrap().0.start, 0);
        assert_eq!(runs.last().unwrap().0.end, broken.len());
    }
}
