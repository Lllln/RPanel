use std::time::{Duration, Instant};

use gpui_kit::component::input::{EditorState, InputEvent, Position};
use gpui_kit::{
    App, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Render, Styled, Subscription, Window, actions, div, px, rgb,
};
use input_hook::WindowRoundStyle;
use json_engine::{SourceFormat, ValidationStatus};

use crate::state::PanelDocumentState;
use crate::ui::editor::{
    create_json_editor_state, render_json_editor, sync_validation_diagnostics,
};
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
        KeyBinding::new("cmd-shift-f", FormatPrettyAction, Some("RPanel")),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-f", FormatPrettyAction, Some("RPanel")),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-shift-m", FormatMinifyAction, Some("RPanel")),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-shift-m", FormatMinifyAction, Some("RPanel")),
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
}

/// Grace period after opening before window deactivation triggers auto-hide.
const FOCUS_LOSS_GRACE: Duration = Duration::from_millis(260);

pub struct FloatingPanel {
    focus_handle: FocusHandle,
    editor: Entity<EditorState>,
    doc: PanelDocumentState,
    round_style: WindowRoundStyle,
    opened_at: Instant,
    auto_hide_on_blur: bool,
    _subscriptions: Vec<Subscription>,
}

impl FloatingPanel {
    pub fn new(
        initial_raw: Option<String>,
        auto_hide_on_blur: bool,
        round_style: WindowRoundStyle,
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

        let activation_sub = cx.observe_window_activation(window, |this, window, _cx| {
            if this.auto_hide_on_blur
                && !window.is_window_active()
                && this.opened_at.elapsed() >= FOCUS_LOSS_GRACE
            {
                window.remove_window();
            }
        });

        let bounds_sub = cx.observe_window_bounds(window, |this, window, _cx| {
            this.round_style = crate::apply_os_window_rounding(window, 10);
        });

        Self {
            focus_handle: cx.focus_handle(),
            editor,
            doc,
            round_style,
            opened_at: Instant::now(),
            auto_hide_on_blur,
            _subscriptions: vec![editor_sub, activation_sub, bounds_sub],
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
        self.doc.feedback = Some(feedback.into());
        let validation = self.doc.validation.clone();
        self.editor.update(cx, |state, cx| {
            state.replace_all(new_text, window, cx);
            sync_validation_diagnostics(state, &validation, cx);
        });
        cx.notify();
    }

    fn apply_pretty(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.current_text(cx);
        match json_engine::format_pretty(&text) {
            Ok(pretty) => self.replace_editor_text(pretty, "Formatted", window, cx),
            Err(err) => {
                self.doc.feedback = Some(format!("Format failed: {err}"));
                cx.notify();
            }
        }
    }

    fn apply_minify(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.current_text(cx);
        match json_engine::minify(&text) {
            Ok(compact) => self.replace_editor_text(compact, "Minified", window, cx),
            Err(err) => {
                self.doc.feedback = Some(format!("Minify failed: {err}"));
                cx.notify();
            }
        }
    }

    fn apply_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.current_text(cx);
        match json_engine::escape_json_string(&text) {
            Ok(escaped) => self.replace_editor_text(escaped, "Escaped", window, cx),
            Err(err) => {
                self.doc.feedback = Some(format!("Escape failed: {err}"));
                cx.notify();
            }
        }
    }

    fn apply_unescape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.current_text(cx);
        match json_engine::unescape_json_string(&text) {
            Ok(unescaped) => {
                let output = json_engine::format_pretty(&unescaped).unwrap_or(unescaped);
                self.replace_editor_text(output, "Unescaped", window, cx);
            }
            Err(err) => {
                self.doc.feedback = Some(format!("Unescape failed: {err}"));
                cx.notify();
            }
        }
    }

    fn reload_from_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match input_hook::clipboard::read_clipboard_text() {
            Ok(raw) => {
                let converted = json_engine::convert_to_json(&raw);
                self.doc.source_format = converted.source_format;
                self.replace_editor_text(converted.formatted, "Reloaded", window, cx);
            }
            Err(_) => {
                self.doc.feedback = Some("Clipboard is empty".into());
                cx.notify();
            }
        }
    }

    fn copy_and_hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.current_text(cx);
        if let Err(err) = input_hook::clipboard::write_clipboard_text(&text) {
            self.doc.feedback = Some(format!("Copy failed: {err}"));
            cx.notify();
            return;
        }
        window.remove_window();
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

        let shell = match self.round_style {
            WindowRoundStyle::NativeDwmRound => shell,
            WindowRoundStyle::RegionClipped => shell
                .border_1()
                .border_color(rgb(palette::BORDER_OUTER))
                .rounded(px(10.0)),
            WindowRoundStyle::TransparentSurface => shell
                .border_1()
                .border_color(rgb(palette::BORDER_OUTER))
                .rounded(px(12.0)),
        };

        shell
            .child(render_toolbar(
                self.doc.source_format,
                self.doc.feedback.as_deref(),
                callbacks,
                cx,
            ))
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
                on_jump_error,
            ))
    }
}
