use gpui_kit::{
    App, ClickEvent, InteractiveElement, IntoElement, ParentElement, Pixels,
    StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use json_engine::ValidationStatus;

use crate::ui::panel::palette;

/// Bottom real-time JSON validation status bar with clickable error location jump.
pub fn render_status_bar(
    status: &ValidationStatus,
    line_count: usize,
    char_count: usize,
    bottom_corner_radius: Pixels,
    on_jump_error: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (dot_color, text_color, label, is_error) = match status {
        ValidationStatus::Empty => (
            palette::STATUS_MUTED,
            palette::INK_SECONDARY,
            "Empty · Paste or type content".to_string(),
            false,
        ),
        ValidationStatus::Valid => (
            palette::STATUS_OK,
            palette::INK_SECONDARY,
            format!("Valid JSON · {line_count} lines · {char_count} chars"),
            false,
        ),
        ValidationStatus::Invalid(err) => (
            palette::STATUS_ERR,
            palette::STATUS_ERR,
            format!("Line {}, Col {} · {}", err.line, err.column, err.message),
            true,
        ),
    };

    let status_pill = div()
        .id("status-validation-pill")
        .flex()
        .items_center()
        .gap(px(7.0))
        .max_w(px(440.0))
        .overflow_hidden()
        .child(div().size(px(7.0)).rounded_full().bg(rgb(dot_color)))
        .child(
            div()
                .text_xs()
                .text_color(rgb(text_color))
                .truncate()
                .child(label),
        );

    let status_pill = if is_error {
        status_pill
            .cursor_pointer()
            .on_click(on_jump_error)
    } else {
        status_pill
    };

    div()
        .flex()
        .items_center()
        .justify_between()
        .h(px(32.0))
        .px(px(14.0))
        .bg(rgb(palette::HEADER_BG))
        .rounded_bl(bottom_corner_radius)
        .rounded_br(bottom_corner_radius)
        .border_t_1()
        .border_color(rgb(palette::BORDER_SUBTLE))
        .child(status_pill)
        .child(
            div()
                .text_xs()
                .text_color(rgb(palette::INK_TERTIARY))
                .child("⌘/Ctrl+Enter Copy · Esc Hide"),
        )
}
