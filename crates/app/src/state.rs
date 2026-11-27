use std::time::Duration;

use json_engine::{SmartInsight, SourceFormat, ValidationStatus};

/// Severity of transient user feedback shown in the floating JSON panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackKind {
    Info,
    Error,
}

/// Transient feedback produced by toolbar actions (format, minify, escape, copy, etc.).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelFeedback {
    pub kind: FeedbackKind,
    pub message: String,
}

impl PanelFeedback {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            kind: FeedbackKind::Info,
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            kind: FeedbackKind::Error,
            message: message.into(),
        }
    }
}

/// Shared document & runtime state for the floating JSON panel.
#[derive(Debug, Clone)]
pub struct PanelDocumentState {
    pub source_format: SourceFormat,
    pub validation: ValidationStatus,
    pub line_count: usize,
    pub char_count: usize,
    pub detected_insight: Option<SmartInsight>,
    pub feedback: Option<PanelFeedback>,
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
        let detected_insight = json_engine::detect_insight_in_editor(text);
        Self {
            source_format,
            validation,
            line_count,
            char_count,
            detected_insight,
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
        self.detected_insight = json_engine::detect_insight_in_editor(text);
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
