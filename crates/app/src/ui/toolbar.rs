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

fn source_format_label(format: SourceFormat) -> &'static str {
    match format {
        SourceFormat::Json => "JSON",
        SourceFormat::Json5 => "JSON5",
        SourceFormat::TableTsv => "TSV 表格",
        SourceFormat::TableCsv => "CSV 表格",
        SourceFormat::KeyValue => "键值对",
        SourceFormat::QueryString => "查询参数",
        SourceFormat::PlainText => "纯文本",
    }
}

/// Restrained Apple-style top toolbar with drag handle, detected format pill, and quick actions.
pub fn render_toolbar(
    source_format: SourceFormat,
    info_feedback: Option<&str>,
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

    #[cfg(target_os = "macos")]
    let (tip_pretty, tip_minify, tip_copy) = (
        "格式化 JSON (⌘E)",
        "压缩为单行 JSON (⌘M)",
        "复制 JSON 并关闭面板 (⌘↵)",
    );
    #[cfg(not(target_os = "macos"))]
    let (tip_pretty, tip_minify, tip_copy) = (
        "格式化 JSON (Ctrl+E)",
        "压缩为单行 JSON (Ctrl+M)",
        "复制 JSON 并关闭面板 (Ctrl+Enter)",
    );

    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .h(px(44.0))
        .px(px(14.0))
        .bg(rgb(palette::HEADER_BG))
        .rounded_tl(top_corner_radius)
        .rounded_tr(top_corner_radius)
        .border_b_1()
        .border_color(rgb(palette::BORDER_SUBTLE))
        .child(
            // Left identity + format pill doubles as a smooth window drag region.
            // `min_w_0` + `overflow_hidden` ensures status pills can never squeeze the action buttons.
            div()
                .flex()
                .items_center()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .h_full()
                .gap(px(8.0))
                .window_control_area(WindowControlArea::Drag)
                .child(
                    div()
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .text_sm()
                        .font_semibold()
                        .text_color(rgb(palette::INK_PRIMARY))
                        .child("RPanel"),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .px(px(7.0))
                        .py(px(2.0))
                        .rounded(px(6.0))
                        .bg(rgb(palette::PILL_BG))
                        .text_xs()
                        .text_color(rgb(palette::INK_SECONDARY))
                        .child(source_format_label(source_format)),
                )
                .children(info_feedback.map(|msg| {
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .px(px(7.0))
                        .py(px(2.0))
                        .rounded(px(6.0))
                        .bg(rgb(palette::ACCENT_TINT))
                        .text_xs()
                        .text_color(rgb(palette::ACCENT))
                        .truncate()
                        .child(msg.to_string())
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .flex_shrink_0()
                .gap(px(4.0))
                .child(
                    Button::new("btn-pretty")
                        .custom(apple_subtle)
                        .xsmall()
                        .label("格式化")
                        .tooltip(tip_pretty)
                        .on_click(callbacks.on_pretty),
                )
                .child(
                    Button::new("btn-minify")
                        .custom(apple_subtle)
                        .xsmall()
                        .label("压缩")
                        .tooltip(tip_minify)
                        .on_click(callbacks.on_minify),
                )
                .child(
                    Button::new("btn-escape")
                        .custom(apple_subtle)
                        .xsmall()
                        .label("转义")
                        .tooltip("转义为 JSON 字符串")
                        .on_click(callbacks.on_escape),
                )
                .child(
                    Button::new("btn-unescape")
                        .custom(apple_subtle)
                        .xsmall()
                        .label("去转义")
                        .tooltip("反转义 JSON 字符串")
                        .on_click(callbacks.on_unescape),
                )
                .child(
                    Button::new("btn-reload")
                        .custom(apple_subtle)
                        .xsmall()
                        .label("剪贴板")
                        .tooltip("从剪贴板重新读取并智能转换")
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
                        .label("复制")
                        .tooltip(tip_copy)
                        .on_click(callbacks.on_copy_hide),
                )
                .child(
                    Button::new("btn-close")
                        .custom(apple_subtle)
                        .xsmall()
                        .label("×")
                        .tooltip("关闭面板 (Esc)")
                        .on_click(callbacks.on_close),
                ),
        )
}
