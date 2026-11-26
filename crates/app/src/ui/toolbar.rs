use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_kit::component::{Sizable, StyledExt};
use gpui_kit::{
    App, ClickEvent, InteractiveElement, IntoElement, ParentElement, Pixels, Styled, Window,
    WindowControlArea, div, px, rgb,
};
use json_engine::SourceFormat;

use crate::ui::panel::palette;

pub type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

pub struct ToolbarCallbacks {
    pub on_pretty: ClickHandler,
    pub on_minify: ClickHandler,
    pub on_escape: ClickHandler,
    pub on_unescape: ClickHandler,
    pub on_reload: ClickHandler,
    pub on_copy_hide: ClickHandler,
    pub on_close: ClickHandler,
}

/// Restrained Apple-style top toolbar with drag handle, detected format pill, and quick actions.
pub fn render_toolbar(
    source_format: SourceFormat,
    feedback: Option<&str>,
    top_corner_radius: Pixels,
    callbacks: ToolbarCallbacks,
    cx: &App,
) -> impl IntoElement {
    let apple_primary = ButtonCustomVariant::new(cx)
        .color(rgb(palette::ACCENT).into())
        .foreground(rgb(0xFFFFFF).into())
        .hover(rgb(palette::ACCENT_HOVER).into())
        .active(rgb(palette::ACCENT_ACTIVE).into());

    let apple_subtle = ButtonCustomVariant::new(cx)
        .color(rgb(palette::SURFACE_BG).into())
        .foreground(rgb(palette::INK_PRIMARY).into())
        .hover(rgb(palette::HOVER_BG).into())
        .active(rgb(palette::ACTIVE_BG).into());

    div()
        .flex()
        .items_center()
        .justify_between()
        .h(px(44.0))
        .px(px(14.0))
        .bg(rgb(palette::HEADER_BG))
        .rounded_tl(top_corner_radius)
        .rounded_tr(top_corner_radius)
        .border_b_1()
        .border_color(rgb(palette::BORDER_SUBTLE))
        .child(
            // Left identity + format pill doubles as a smooth window drag region.
            div()
                .flex()
                .items_center()
                .flex_1()
                .h_full()
                .gap(px(8.0))
                .window_control_area(WindowControlArea::Drag)
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .text_color(rgb(palette::INK_PRIMARY))
                        .child("RPanel"),
                )
                .child(
                    div()
                        .px(px(7.0))
                        .py(px(2.0))
                        .rounded(px(6.0))
                        .bg(rgb(palette::PILL_BG))
                        .text_xs()
                        .text_color(rgb(palette::INK_SECONDARY))
                        .child(source_format.display_label()),
                )
                .children(feedback.map(|msg| {
                    div()
                        .px(px(7.0))
                        .py(px(2.0))
                        .rounded(px(6.0))
                        .bg(rgb(palette::ACCENT_TINT))
                        .text_xs()
                        .text_color(rgb(palette::ACCENT))
                        .child(msg.to_string())
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(
                    Button::new("btn-pretty")
                        .custom(apple_subtle)
                        .xsmall()
                        .label("Pretty")
                        .tooltip("Format JSON (Ctrl/Cmd+Shift+F)")
                        .on_click(callbacks.on_pretty),
                )
                .child(
                    Button::new("btn-minify")
                        .custom(apple_subtle)
                        .xsmall()
                        .label("Minify")
                        .tooltip("Compact single-line JSON (Ctrl/Cmd+Shift+M)")
                        .on_click(callbacks.on_minify),
                )
                .child(
                    Button::new("btn-escape")
                        .custom(apple_subtle)
                        .xsmall()
                        .label("Escape")
                        .tooltip("Escape into JSON string literal")
                        .on_click(callbacks.on_escape),
                )
                .child(
                    Button::new("btn-unescape")
                        .custom(apple_subtle)
                        .xsmall()
                        .label("Unescape")
                        .tooltip("Unescape JSON string literal")
                        .on_click(callbacks.on_unescape),
                )
                .child(
                    Button::new("btn-reload")
                        .custom(apple_subtle)
                        .xsmall()
                        .label("Clipboard")
                        .tooltip("Reload & smart-convert from clipboard")
                        .on_click(callbacks.on_reload),
                )
                .child(
                    div()
                        .w(px(1.0))
                        .h(px(16.0))
                        .mx(px(4.0))
                        .bg(rgb(palette::BORDER_SUBTLE)),
                )
                .child(
                    Button::new("btn-copy-hide")
                        .custom(apple_primary)
                        .xsmall()
                        .label("Copy")
                        .tooltip("Copy JSON and hide panel (Ctrl/Cmd+Enter)")
                        .on_click(callbacks.on_copy_hide),
                )
                .child(
                    Button::new("btn-close")
                        .custom(apple_subtle)
                        .xsmall()
                        .label("×")
                        .tooltip("Hide panel (Esc)")
                        .on_click(callbacks.on_close),
                ),
        )
}
