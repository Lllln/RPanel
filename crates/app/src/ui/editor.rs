use gpui_kit::base::input::{Diagnostic, DiagnosticSeverity};
use gpui_kit::component::input::{Editor, EditorState, GutterStyle, Position, RopeExt, TabSize};
use gpui_kit::{App, AppContext, Context, Entity, IntoElement, Styled, Window, px, rgb};
use json_engine::ValidationStatus;

use crate::ui::json_highlight::json_highlighter_factory;
use crate::ui::panel::palette;

/// Create a JSON-highlighted code editor state with line numbers, folding, and 2-space indent.
pub fn create_json_editor_state(
    initial_text: &str,
    window: &mut Window,
    cx: &mut App,
) -> Entity<EditorState> {
    let initial = initial_text.to_string();
    cx.new(|cx| {
        let mut state = EditorState::new(window, cx)
            .language("json")
            .line_number(true)
            .folding(true)
            .soft_wrap(false)
            .tab_size(TabSize {
                tab_size: 2,
                hard_tabs: false,
            })
            .placeholder("Paste or type JSON, JSON5, CSV/TSV, HTTP headers, or URL query...");
        state.set_highlighter_factory(json_highlighter_factory(), cx);
        state.set_value(initial, window, cx);
        state
    })
}

/// Update inline error diagnostics inside the code editor based on real-time JSON validation.
pub fn sync_validation_diagnostics(
    state: &mut EditorState,
    status: &ValidationStatus,
    cx: &mut Context<EditorState>,
) {
    let text = state.text().clone();
    if let Some(diagnostics) = state.diagnostics_mut() {
        diagnostics.reset(&text);
        if let ValidationStatus::Invalid(err) = status {
            let total_lines = text.lines_len();
            if total_lines > 0 {
                let line_idx = err.line.saturating_sub(1).min(total_lines - 1);
                let line_len = text.slice_line(line_idx).len_chars();
                if line_len > 0 {
                    let start_col = err.column.saturating_sub(1).min(line_len - 1);
                    let end_col = (start_col + 1).min(line_len);
                    let start = Position::new(line_idx as u32, start_col as u32);
                    let end = Position::new(line_idx as u32, end_col as u32);
                    if start < end {
                        diagnostics.push(
                            Diagnostic::new(start..end, err.message.clone())
                                .with_severity(DiagnosticSeverity::Error)
                                .with_source("json"),
                        );
                    }
                }
            }
        }
    }
    cx.notify();
}

/// Render the styled JSON code editor with restrained Apple-inspired gutter typography.
pub fn render_json_editor(state: &Entity<EditorState>) -> impl IntoElement {
    let gutter = GutterStyle::default()
        .with_gap(px(14.0))
        .with_line_number_color(rgb(palette::LINE_NUMBER).into())
        .with_active_line_number_color(rgb(palette::INK_SECONDARY).into());

    Editor::new(state)
        .size_full()
        .h_full()
        .appearance(false)
        .bordered(false)
        .gutter_style(gutter)
}
