use std::time::{Duration, Instant};

use gpui_kit::component::StyledExt;
use gpui_kit::component::input::{EditorState, InputEvent, Position};
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Window, actions, div,
    px, rgb,
};
use input_hook::{DismissWatchGuard, WindowRoundStyle};
use json_engine::{SourceFormat, ValidationStatus};

use crate::state::{FeedbackKind, PanelDocumentState, PanelFeedback};
use crate::ui::editor::{
    create_json_editor_state, render_json_editor, sync_validation_diagnostics,
};
use crate::ui::format_engine_error;
use crate::ui::status_bar::render_status_bar;
use crate::ui::toolbar::{ToolbarCallbacks, render_toolbar};

actions!(
    rpanel,
    [
        HidePanel,
        CopyAndHide,
        FormatPrettyAction,
        FormatMinifyAction,
    ]
);

/// Register keyboard shortcuts scoped to the floating JSON panel.
pub fn register_keybindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", HidePanel, Some("RPanel")),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-enter", CopyAndHide, Some("RPanel")),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-enter", CopyAndHide, Some("RPanel")),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-e", FormatPrettyAction, Some("RPanel")),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-e", FormatPrettyAction, Some("RPanel")),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-m", FormatMinifyAction, Some("RPanel")),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-m", FormatMinifyAction, Some("RPanel")),
    ]);
}

/// Restrained Apple-inspired light surface palette.
pub mod palette {
    pub const SURFACE_BG: u32 = 0xF5F5F7;
    pub const HEADER_BG: u32 = 0xFBFBFD;
    pub const CARD_BG: u32 = 0xFFFFFF;
    pub const BORDER_OUTER: u32 = 0xD2D2D7;
    pub const BORDER_SUBTLE: u32 = 0xE5E5EA;
    pub const PILL_BG: u32 = 0xECECF1;
    pub const HOVER_BG: u32 = 0xEAEAF0;
    pub const ACTIVE_BG: u32 = 0xDFDFE6;
    pub const INK_PRIMARY: u32 = 0x1D1D1F;
    pub const INK_SECONDARY: u32 = 0x6E6E73;
    pub const INK_TERTIARY: u32 = 0x8E8E93;
    pub const LINE_NUMBER: u32 = 0xAEAEB2;
    pub const ACCENT: u32 = 0x007AFF;
    pub const ACCENT_HOVER: u32 = 0x006AE0;
    pub const ACCENT_ACTIVE: u32 = 0x005EC4;
    pub const ACCENT_TINT: u32 = 0xE8F2FF;
    pub const STATUS_OK: u32 = 0x34C759;
    pub const STATUS_ERR: u32 = 0xFF3B30;
    pub const STATUS_MUTED: u32 = 0xAEAEB2;
    pub const ERR_TINT: u32 = 0xFFF2F1;
    pub const ERR_BORDER: u32 = 0xFFD6D3;
    pub const ERR_TEXT: u32 = 0xD70015;
    pub const TS_TINT: u32 = 0xEBF9EE;
    pub const TS_BORDER: u32 = 0xCBEFD3;
    pub const TS_ACCENT: u32 = 0x1D8F3A;
}

/// Grace period after opening before window deactivation triggers auto-hide.
const FOCUS_LOSS_GRACE: Duration = Duration::from_millis(260);

pub struct FloatingPanel {
    focus_handle: FocusHandle,
    editor: Entity<EditorState>,
    doc: PanelDocumentState,
    last_applied_text: Option<String>,
    round_style: WindowRoundStyle,
    opened_at: Instant,
    ever_activated: bool,
    auto_hide_on_blur: bool,
    _dismiss_watch: Option<DismissWatchGuard>,
    _subscriptions: Vec<Subscription>,
}

impl FloatingPanel {
    pub fn new(
        initial_raw: Option<String>,
        auto_hide_on_blur: bool,
        round_style: WindowRoundStyle,
        dismiss_watch: Option<DismissWatchGuard>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (source_format, initial_text) = match initial_raw {
            Some(raw) if !raw.trim().is_empty() => {
                let converted = json_engine::convert_to_json(&raw);
                (converted.source_format, converted.formatted)
            }
            _ => (SourceFormat::Json, String::new()),
        };

        let doc = PanelDocumentState::from_text(source_format, &initial_text);
        let editor = create_json_editor_state(&initial_text, window, cx);
        let initial_validation = doc.validation.clone();
        editor.update(cx, |state, cx| {
            sync_validation_diagnostics(state, &initial_validation, cx);
            state.focus(window, cx);
        });

        let editor_sub = cx.subscribe(&editor, |this, editor, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let text = editor.read(cx).value().to_string();
                if this.last_applied_text.as_deref() == Some(text.as_str()) {
                    this.last_applied_text = None;
                } else {
                    this.doc.feedback = None;
                }
                this.doc.refresh_metrics(&text);
                let validation = this.doc.validation.clone();
                let editor_weak = editor.downgrade();
                cx.defer(move |cx| {
                    let _ = editor_weak.update(cx, |state, cx| {
                        sync_validation_diagnostics(state, &validation, cx);
                    });
                });
                cx.notify();
            }
        });

        let ever_activated = window.is_window_active();
        let activation_sub = cx.observe_window_activation(window, |this, window, _cx| {
            if window.is_window_active() {
                this.ever_activated = true;
            } else if this.auto_hide_on_blur
                && (this.ever_activated || this.opened_at.elapsed() >= FOCUS_LOSS_GRACE)
            {
                window.remove_window();
            }
        });

        Self {
            focus_handle: cx.focus_handle(),
            editor,
            doc,
            last_applied_text: None,
            round_style,
            opened_at: Instant::now(),
            ever_activated,
            auto_hide_on_blur,
            _dismiss_watch: dismiss_watch,
            _subscriptions: vec![editor_sub, activation_sub],
        }
    }

    fn current_text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    fn replace_editor_text(
        &mut self,
        new_text: String,
        feedback: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.doc.refresh_metrics(&new_text);
        self.doc.feedback = Some(PanelFeedback::info(feedback));
        self.last_applied_text = Some(new_text.clone());
        let validation = self.doc.validation.clone();
        self.editor.update(cx, |state, cx| {
            state.replace_all(new_text, window, cx);
            sync_validation_diagnostics(state, &validation, cx);
        });
        cx.notify();
    }

    fn set_error_feedback(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.doc.feedback = Some(PanelFeedback::error(message));
        cx.notify();
    }

    fn dismiss_feedback(&mut self, cx: &mut Context<Self>) {
        self.doc.feedback = None;
        cx.notify();
    }

    fn apply_pretty(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.current_text(cx);
        match json_engine::format_pretty(&text) {
            Ok(pretty) => self.replace_editor_text(pretty, "已格式化", window, cx),
            Err(err) => {
                self.set_error_feedback(format!("格式化失败：{}", format_engine_error(&err)), cx);
            }
        }
    }

    fn apply_minify(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.current_text(cx);
        match json_engine::minify(&text) {
            Ok(compact) => self.replace_editor_text(compact, "已压缩", window, cx),
            Err(err) => {
                self.set_error_feedback(format!("压缩失败：{}", format_engine_error(&err)), cx);
            }
        }
    }

    fn apply_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.current_text(cx);
        match json_engine::escape_json_string(&text) {
            Ok(escaped) => self.replace_editor_text(escaped, "已转义", window, cx),
            Err(err) => {
                self.set_error_feedback(format!("转义失败：{}", format_engine_error(&err)), cx);
            }
        }
    }

    fn apply_unescape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.current_text(cx);
        match json_engine::unescape_json_string(&text) {
            Ok(unescaped) => {
                let output = json_engine::format_pretty(&unescaped).unwrap_or(unescaped);
                self.replace_editor_text(output, "已去转义", window, cx);
            }
            Err(err) => {
                self.set_error_feedback(format!("去转义失败：{}", format_engine_error(&err)), cx);
            }
        }
    }

    fn reload_from_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match input_hook::clipboard::read_clipboard_text() {
            Ok(raw) => {
                let converted = json_engine::convert_to_json(&raw);
                self.doc.source_format = converted.source_format;
                self.replace_editor_text(converted.formatted, "已载入", window, cx);
            }
            Err(_) => {
                self.set_error_feedback("剪贴板为空，无法载入内容", cx);
            }
        }
    }

    fn copy_and_hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.current_text(cx);
        if let Err(err) = input_hook::clipboard::write_clipboard_text(&text) {
            self.set_error_feedback(format!("复制失败：{err}"), cx);
            return;
        }
        window.remove_window();
    }

    fn copy_detected_timestamp(&mut self, cx: &mut Context<Self>) {
        let Some(ts) = &self.doc.detected_timestamp else {
            return;
        };
        let formatted = ts.local_datetime.clone();
        if let Err(err) = input_hook::clipboard::write_clipboard_text(&formatted) {
            self.set_error_feedback(format!("复制时间失败：{err}"), cx);
            return;
        }
        self.doc.feedback = Some(PanelFeedback::info("已复制时间"));
        cx.notify();
    }

    fn jump_to_error(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let ValidationStatus::Invalid(err) = &self.doc.validation {
            let line = err.line.saturating_sub(1) as u32;
            let col = err.column.saturating_sub(1) as u32;
            let pos = Position::new(line, col);
            self.editor.update(cx, |state, cx| {
                state.set_cursor_position(pos, window, cx);
                state.focus(window, cx);
            });
        }
    }
}

impl Focusable for FloatingPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FloatingPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let callbacks = ToolbarCallbacks {
            on_pretty: Box::new(cx.listener(|this, _, window, cx| this.apply_pretty(window, cx))),
            on_minify: Box::new(cx.listener(|this, _, window, cx| this.apply_minify(window, cx))),
            on_escape: Box::new(cx.listener(|this, _, window, cx| this.apply_escape(window, cx))),
            on_unescape: Box::new(
                cx.listener(|this, _, window, cx| this.apply_unescape(window, cx)),
            ),
            on_reload: Box::new(
                cx.listener(|this, _, window, cx| this.reload_from_clipboard(window, cx)),
            ),
            on_copy_hide: Box::new(
                cx.listener(|this, _, window, cx| this.copy_and_hide(window, cx)),
            ),
            on_close: Box::new(cx.listener(|_, _, window, _| {
                window.remove_window();
            })),
        };

        let on_jump_error = cx.listener(|this, _, window, cx| this.jump_to_error(window, cx));

        let shell = div()
            .key_context("RPanel")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &HidePanel, window, _| {
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &CopyAndHide, window, cx| {
                this.copy_and_hide(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FormatPrettyAction, window, cx| {
                this.apply_pretty(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FormatMinifyAction, window, cx| {
                this.apply_minify(window, cx);
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(palette::SURFACE_BG))
            .text_color(rgb(palette::INK_PRIMARY))
            .overflow_hidden();

        // GPUI's `overflow_hidden()` clips to a rectangle rather than rounded corners, so
        // top/bottom bars with their own background must match the shell's inner corner radius.
        let (shell, inner_radius) = match self.round_style {
            WindowRoundStyle::NativeDwmRound => (shell, px(0.0)),
            WindowRoundStyle::SquareOpaque => (
                shell.border_1().border_color(rgb(palette::BORDER_OUTER)),
                px(0.0),
            ),
            WindowRoundStyle::TransparentSurface => (
                shell
                    .border_1()
                    .border_color(rgb(palette::BORDER_OUTER))
                    .rounded(px(12.0)),
                px(11.0),
            ),
        };

        let info_feedback = self
            .doc
            .feedback
            .as_ref()
            .filter(|fb| fb.kind == FeedbackKind::Info)
            .map(|fb| fb.message.as_str());

        let error_feedback = self
            .doc
            .feedback
            .as_ref()
            .filter(|fb| fb.kind == FeedbackKind::Error)
            .map(|fb| fb.message.clone());

        let has_validation_error = matches!(self.doc.validation, ValidationStatus::Invalid(_));
        let on_banner_jump = cx.listener(|this, _, window, cx| this.jump_to_error(window, cx));
        let on_dismiss_error = cx.listener(|this, _, _, cx| this.dismiss_feedback(cx));

        let error_banner = error_feedback.map(|msg| {
            let msg_area = div()
                .id("error-banner-message")
                .flex()
                .items_center()
                .flex_1()
                .min_w_0()
                .gap(px(7.0))
                .child(
                    div()
                        .flex_shrink_0()
                        .size(px(6.0))
                        .rounded_full()
                        .bg(rgb(palette::STATUS_ERR)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(rgb(palette::ERR_TEXT))
                        .child(msg),
                );

            let msg_area = if has_validation_error {
                msg_area.cursor_pointer().on_click(on_banner_jump)
            } else {
                msg_area
            };

            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .px(px(14.0))
                .py(px(6.0))
                .bg(rgb(palette::ERR_TINT))
                .border_b_1()
                .border_color(rgb(palette::ERR_BORDER))
                .child(msg_area)
                .child(
                    div()
                        .id("error-banner-dismiss")
                        .flex_shrink_0()
                        .px(px(5.0))
                        .py(px(1.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .text_xs()
                        .text_color(rgb(palette::ERR_TEXT))
                        .hover(|s| s.bg(rgb(palette::ERR_BORDER)))
                        .on_click(on_dismiss_error)
                        .child("×"),
                )
        });

        let on_copy_timestamp =
            cx.listener(|this, _, _, cx| this.copy_detected_timestamp(cx));
        let timestamp_banner = self.doc.detected_timestamp.as_ref().map(|ts| {
            let unit_label = ts.unit.label_zh();
            let datetime = ts.local_datetime.clone();
            let tz = ts.timezone_label.clone();

            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .px(px(14.0))
                .py(px(6.0))
                .bg(rgb(palette::TS_TINT))
                .border_b_1()
                .border_color(rgb(palette::TS_BORDER))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_1()
                        .min_w_0()
                        .gap(px(8.0))
                        .child(
                            div()
                                .flex_shrink_0()
                                .size(px(6.0))
                                .rounded_full()
                                .bg(rgb(palette::STATUS_OK)),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(rgb(palette::TS_ACCENT))
                                .child(format!("时间戳 ({unit_label}) →")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(rgb(palette::INK_PRIMARY))
                                .whitespace_nowrap()
                                .child(datetime),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(palette::INK_SECONDARY))
                                .whitespace_nowrap()
                                .child(format!("({tz})")),
                        ),
                )
                .child(
                    div()
                        .id("timestamp-banner-copy")
                        .flex_shrink_0()
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded(px(5.0))
                        .bg(rgb(palette::CARD_BG))
                        .border_1()
                        .border_color(rgb(palette::TS_BORDER))
                        .cursor_pointer()
                        .text_xs()
                        .font_medium()
                        .text_color(rgb(palette::TS_ACCENT))
                        .hover(|s| s.bg(rgb(palette::TS_BORDER)))
                        .on_click(on_copy_timestamp)
                        .child("复制时间"),
                )
        });

        shell
            .child(render_toolbar(
                self.doc.source_format,
                info_feedback,
                inner_radius,
                callbacks,
                cx,
            ))
            .children(timestamp_banner)
            .children(error_banner)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(px(10.0))
                    .child(
                        div()
                            .size_full()
                            .bg(rgb(palette::CARD_BG))
                            .border_1()
                            .border_color(rgb(palette::BORDER_SUBTLE))
                            .rounded(px(10.0))
                            .overflow_hidden()
                            .child(render_json_editor(&self.editor)),
                    ),
            )
            .child(render_status_bar(
                &self.doc.validation,
                self.doc.line_count,
                self.doc.char_count,
                inner_radius,
                on_jump_error,
            ))
    }
}
