use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use gpui_kit::component::StyledExt;
use gpui_kit::component::input::{EditorState, InputEvent, Position};
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Window, actions, div,
    px, rgb,
};
use input_hook::{DismissWatchGuard, HookControl};
use json_engine::{InsightKind, SourceFormat, ValidationStatus};

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
        TogglePinAction,
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
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-p", TogglePinAction, Some("RPanel")),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-p", TogglePinAction, Some("RPanel")),
    ]);
}

/// Restrained Apple-inspired light surface palette.
pub mod palette {
    use json_engine::InsightTone;

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
    pub const ACCENT_BORDER: u32 = 0xC6DFFF;
    pub const STATUS_OK: u32 = 0x34C759;
    pub const STATUS_ERR: u32 = 0xFF3B30;
    pub const STATUS_MUTED: u32 = 0xAEAEB2;
    pub const ERR_TINT: u32 = 0xFFF2F1;
    pub const ERR_BORDER: u32 = 0xFFD6D3;
    pub const ERR_TEXT: u32 = 0xD70015;
    pub const TS_TINT: u32 = 0xEBF9EE;
    pub const TS_BORDER: u32 = 0xCBEFD3;
    pub const TS_ACCENT: u32 = 0x1D8F3A;
    pub const PURPLE_TINT: u32 = 0xF4EEFF;
    pub const PURPLE_BORDER: u32 = 0xDFCEFF;
    pub const PURPLE_ACCENT: u32 = 0x6E40C9;
    pub const AMBER_TINT: u32 = 0xFFF7E6;
    pub const AMBER_BORDER: u32 = 0xFBE0A6;
    pub const AMBER_ACCENT: u32 = 0xB25E00;

    /// Returns `(tint_bg, border, accent_fg)` for a given [`InsightTone`].
    pub const fn insight_colors(tone: InsightTone) -> (u32, u32, u32) {
        match tone {
            InsightTone::Green => (TS_TINT, TS_BORDER, TS_ACCENT),
            InsightTone::Blue => (ACCENT_TINT, ACCENT_BORDER, ACCENT),
            InsightTone::Purple => (PURPLE_TINT, PURPLE_BORDER, PURPLE_ACCENT),
            InsightTone::Amber => (AMBER_TINT, AMBER_BORDER, AMBER_ACCENT),
            InsightTone::Neutral => (PILL_BG, BORDER_SUBTLE, INK_PRIMARY),
        }
    }
}

/// Grace period after opening before window deactivation triggers auto-hide.
const FOCUS_LOSS_GRACE: Duration = Duration::from_millis(260);

pub struct FloatingPanel {
    focus_handle: FocusHandle,
    editor: Entity<EditorState>,
    doc: PanelDocumentState,
    last_applied_text: Option<String>,
    rounded: bool,
    opened_at: Instant,
    ever_activated: bool,
    pinned: Arc<AtomicBool>,
    hook_control: Option<Arc<HookControl>>,
    dismiss_watch: Option<DismissWatchGuard>,
    _subscriptions: Vec<Subscription>,
}

impl FloatingPanel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        initial_raw: Option<String>,
        pinned: Arc<AtomicBool>,
        rounded: bool,
        hook_control: Option<Arc<HookControl>>,
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
            } else if !this.is_pinned()
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
            rounded,
            opened_at: Instant::now(),
            ever_activated,
            pinned,
            hook_control,
            dismiss_watch,
            _subscriptions: vec![editor_sub, activation_sub],
        }
    }

    fn is_pinned(&self) -> bool {
        self.pinned.load(Ordering::Acquire)
    }

    fn toggle_pin(&mut self, cx: &mut Context<Self>) {
        let next = !self.is_pinned();
        self.pinned.store(next, Ordering::Release);
        if next {
            // Pause global mouse-down watch while pinned so outside clicks never dismiss the panel.
            self.dismiss_watch = None;
            self.doc.feedback = Some(PanelFeedback::info("已固定"));
        } else {
            if self.dismiss_watch.is_none() {
                self.dismiss_watch = self
                    .hook_control
                    .as_ref()
                    .map(|ctrl| ctrl.acquire_dismiss_watch());
            }
            self.doc.feedback = Some(PanelFeedback::info("已取消固定"));
        }
        cx.notify();
    }

    fn handle_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_pinned() {
            #[cfg(target_os = "macos")]
            let msg = "已固定 · 按 ⌘P 取消或点击 ×";
            #[cfg(not(target_os = "macos"))]
            let msg = "已固定 · 按 Ctrl+P 取消或点击 ×";
            self.doc.feedback = Some(PanelFeedback::info(msg));
            cx.notify();
            return;
        }
        window.remove_window();
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
        if self.is_pinned() {
            self.doc.feedback = Some(PanelFeedback::info("已复制"));
            cx.notify();
            return;
        }
        window.remove_window();
    }

    fn copy_detected_insight(&mut self, cx: &mut Context<Self>) {
        let Some(insight) = &self.doc.detected_insight else {
            return;
        };
        let copy_text = insight.copy_text.clone();
        let feedback = if insight.kind == InsightKind::Timestamp {
            "已复制时间"
        } else {
            "已复制识别结果"
        };
        if let Err(err) = input_hook::clipboard::write_clipboard_text(&copy_text) {
            self.set_error_feedback(format!("复制失败：{err}"), cx);
            return;
        }
        self.doc.feedback = Some(PanelFeedback::info(feedback));
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
        let pinned = self.is_pinned();
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
            on_toggle_pin: Box::new(cx.listener(|this, _, _, cx| this.toggle_pin(cx))),
            on_close: Box::new(cx.listener(|_, _, window, _| {
                window.remove_window();
            })),
        };

        let on_jump_error = cx.listener(|this, _, window, cx| this.jump_to_error(window, cx));

        let shell = div()
            .key_context("RPanel")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &HidePanel, window, cx| {
                this.handle_escape(window, cx);
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
            .on_action(cx.listener(|this, _: &TogglePinAction, _, cx| {
                this.toggle_pin(cx);
            }))
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(palette::SURFACE_BG))
            .text_color(rgb(palette::INK_PRIMARY))
            .overflow_hidden();

        let shell = shell
            .border_1()
            .border_color(rgb(palette::BORDER_OUTER));

        // GPUI's `overflow_hidden()` clips to a rectangle rather than rounded corners, so
        // top/bottom bars with their own background must match the shell's inner corner radius.
        let (shell, inner_radius) = if self.rounded {
            (shell.rounded(px(12.0)), px(11.0))
        } else {
            (shell, px(0.0))
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

        let on_copy_insight = cx.listener(|this, _, _, cx| this.copy_detected_insight(cx));
        let insight_banner = self.doc.detected_insight.as_ref().map(|insight| {
            let (tint_bg, border, accent_fg) = palette::insight_colors(insight.tone);
            let badge = insight.badge.clone();
            let pill = insight.pill.clone();
            let primary = insight.primary.clone();
            let swatch = insight.color_swatch;
            let copy_label = if insight.kind == InsightKind::Timestamp {
                "复制时间"
            } else {
                "复制结果"
            };

            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.0))
                .px(px(14.0))
                .py(px(6.0))
                .bg(rgb(tint_bg))
                .border_b_1()
                .border_color(rgb(border))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_1()
                        .min_w_0()
                        .gap(px(8.0))
                        .children(swatch.map(|color_hex| {
                            div()
                                .flex_shrink_0()
                                .size(px(12.0))
                                .rounded(px(3.0))
                                .bg(rgb(color_hex))
                                .border_1()
                                .border_color(rgb(palette::BORDER_OUTER))
                        }))
                        .children(swatch.is_none().then(|| {
                            div()
                                .flex_shrink_0()
                                .size(px(6.0))
                                .rounded_full()
                                .bg(rgb(accent_fg))
                        }))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(rgb(accent_fg))
                                .child(format!("{badge} ({pill}) →")),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .text_xs()
                                .font_semibold()
                                .text_color(rgb(palette::INK_PRIMARY))
                                .whitespace_nowrap()
                                .truncate()
                                .child(primary),
                        ),
                )
                .child(
                    div()
                        .id("insight-banner-copy")
                        .flex_shrink_0()
                        .px(px(8.0))
                        .py(px(2.0))
                        .rounded(px(5.0))
                        .bg(rgb(palette::CARD_BG))
                        .border_1()
                        .border_color(rgb(border))
                        .cursor_pointer()
                        .text_xs()
                        .font_medium()
                        .text_color(rgb(accent_fg))
                        .hover(|s| s.bg(rgb(border)))
                        .on_click(on_copy_insight)
                        .child(copy_label),
                )
        });

        shell
            .child(render_toolbar(
                self.doc.source_format,
                info_feedback,
                pinned,
                inner_radius,
                callbacks,
                cx,
            ))
            .children(insight_banner)
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
                pinned,
                inner_radius,
                on_jump_error,
            ))
    }
}
