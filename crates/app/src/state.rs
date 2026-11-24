use std::time::Duration;

use json_engine::{SourceFormat, ValidationStatus};

/// Shared document & runtime state for the floating JSON panel.
#[derive(Debug, Clone)]
pub struct PanelDocumentState {
    pub source_format: SourceFormat,
    pub validation: ValidationStatus,
    pub line_count: usize,
    pub char_count: usize,
    pub feedback: Option<String>,
}

impl PanelDocumentState {
    pub fn from_text(source_format: SourceFormat, text: &str) -> Self {
        let validation = json_engine::validate_json(text);
        let line_count = if text.is_empty() {
            0
        } else {
            text.lines().count().max(1)
        };
        let char_count = text.chars().count();
        Self {
            source_format,
            validation,
            line_count,
            char_count,
            feedback: None,
        }
    }

    pub fn refresh_metrics(&mut self, text: &str) {
        self.validation = json_engine::validate_json(text);
        self.line_count = if text.is_empty() {
            0
        } else {
            text.lines().count().max(1)
        };
        self.char_count = text.chars().count();
    }
}

/// Configurable runtime settings synchronized with the tray menu and input hook.
#[derive(Debug, Clone, Copy)]
pub struct AppSettings {
    pub hook_enabled: bool,
    pub hold_duration: Duration,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            hook_enabled: true,
            hold_duration: Duration::from_millis(300),
        }
    }
}
