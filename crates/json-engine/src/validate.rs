/// Detailed location and message for a JSON syntax error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub line: usize,
    pub column: usize,
    pub byte_offset: usize,
    pub message: String,
}

/// Real-time validation status for the editor status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationStatus {
    Empty,
    Valid,
    Invalid(ValidationError),
}

/// Validate strict JSON syntax and return 1-based line/column error position when invalid.
pub fn validate_json(input: &str) -> ValidationStatus {
    if input.trim().is_empty() {
        return ValidationStatus::Empty;
    }

    match serde_json::from_str::<serde_json::Value>(input) {
        Ok(_) => ValidationStatus::Valid,
        Err(err) => {
            let line = err.line();
            let column = err.column();
            let byte_offset = line_col_to_byte_offset(input, line, column);
            ValidationStatus::Invalid(ValidationError {
                line,
                column,
                byte_offset,
                message: err.to_string(),
            })
        }
    }
}

fn line_col_to_byte_offset(text: &str, target_line: usize, target_col: usize) -> usize {
    if target_line == 0 {
        return 0;
    }

    let mut current_line = 1usize;
    let mut line_start = 0usize;
    for (idx, ch) in text.char_indices() {
        if current_line == target_line {
            let rest = &text[line_start..];
            let col_chars = target_col.saturating_sub(1);
            return line_start
                + rest
                    .char_indices()
                    .nth(col_chars)
                    .map(|(offset, _)| offset)
                    .unwrap_or(rest.len());
        }
        if ch == '\n' {
            current_line += 1;
            line_start = idx + 1;
        }
    }

    text.len()
}
