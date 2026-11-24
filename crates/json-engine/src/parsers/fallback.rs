use serde_json::{Value, json};

use crate::pipeline::SourceFormat;

/// Fallback wrapper turning arbitrary plain text into an editable JSON object.
pub fn wrap_plain_text(input: &str) -> (SourceFormat, Value) {
    (
        SourceFormat::PlainText,
        json!({
            "text": input
        }),
    )
}
