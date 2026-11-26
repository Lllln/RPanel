use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use gpui_kit::component::StyledExt;
use gpui_kit::{
    App, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Window, actions, div,
    px, rgb,
};
use input_hook::{DismissWatchGuard, ScreenPoint};
use json_engine::ParsedTimestamp;

use crate::ui::panel::palette;

actions!(
    rpanel_menu,
    [DismissQuickMenu, ConfirmQuickMenu, CopyQuickTimestamp]
);

pub fn register_quick_menu_keybindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", DismissQuickMenu, Some("RPanelQuickMenu")),
        KeyBinding::new("enter", ConfirmQuickMenu, Some("RPanelQuickMenu")),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-c", CopyQuickTimestamp, Some("RPanelQuickMenu")),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-c", CopyQuickTimestamp, Some("RPanelQuickMenu")),
    ]);
}

/// Event sent when the user clicks "查看 JSON" on the floating quick-action popup.
#[derive(Debug, Clone)]
pub struct OpenEditorRequest {
    pub cursor: Option<ScreenPoint>,
    pub captured_text: Option<String>,
}

const MENU_BLUR_GRACE: Duration = Duration::from_millis(260);

/// Minimal Apple-style floating quick-action menu near the cursor.
/// Automatically shows a direct local date-time conversion row when the selection is a timestamp.
pub struct QuickActionMenu {
    focus_handle: FocusHandle,
    cursor: Option<ScreenPoint>,
    captured_text: Option<String>,
    parsed_timestamp: Option<ParsedTimestamp>,
    open_editor_tx: Sender<OpenEditorRequest>,
    rounded: bool,
    opened_at: Instant,
    ever_activated: bool,
    _dismiss_watch: Option<DismissWatchGuard>,
    _subscriptions: Vec<Subscription>,
}

impl QuickActionMenu {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cursor: Option<ScreenPoint>,
        captured_text: Option<String>,
        parsed_timestamp: Option<ParsedTimestamp>,
        open_editor_tx: Sender<OpenEditorRequest>,
        rounded: bool,
        dismiss_watch: Option<DismissWatchGuard>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        let ever_activated = window.is_window_active();
        let activation_sub = cx.observe_window_activation(window, |this, window, _cx| {
            if window.is_window_active() {
                this.ever_activated = true;
            } else if this.ever_activated || this.opened_at.elapsed() >= MENU_BLUR_GRACE {
                window.remove_window();
            }
        });

        Self {
            focus_handle,
            cursor,
            captured_text,
            parsed_timestamp,
            open_editor_tx,
            rounded,
            opened_at: Instant::now(),
            ever_activated,
            _dismiss_watch: dismiss_watch,
            _subscriptions: vec![activation_sub],
        }
    }

    fn trigger_open_editor(&mut self, window: &mut Window) {
        let _ = self.open_editor_tx.send(OpenEditorRequest {
            cursor: self.cursor,
            captured_text: self.captured_text.take(),
        });
        window.remove_window();
    }

    fn copy_timestamp_and_dismiss(&mut self, window: &mut Window) {
        if let Some(ts) = &self.parsed_timestamp {
            let _ = input_hook::clipboard::write_clipboard_text(&ts.local_datetime);
            window.remove_window();
        }
    }
}

impl Focusable for QuickActionMenu {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for QuickActionMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_selection = self
            .captured_text
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());

        let container = div()
            .key_context("RPanelQuickMenu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &DismissQuickMenu, window, _| {
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &ConfirmQuickMenu, window, _| {
                this.trigger_open_editor(window);
            }))
            .on_action(cx.listener(|this, _: &CopyQuickTimestamp, window, _| {
                this.copy_timestamp_and_dismiss(window);
            }))
            .size_full()
            .bg(rgb(palette::CARD_BG))
            .p(px(5.0))
            .flex()
            .flex_col()
            .justify_center()
            .overflow_hidden();

        let container = container
            .border_1()
            .border_color(rgb(palette::BORDER_OUTER));
        let container = if self.rounded {
            container.rounded(px(12.0))
        } else {
            container
        };

        let timestamp_section = self.parsed_timestamp.as_ref().map(|ts| {
            let datetime = ts.local_datetime.clone();
            let subtitle = format!("本地时间 ({}) · 点击复制", ts.timezone_label);
            let unit_label = ts.unit.label_zh();

            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(px(4.0))
                .child(
                    div()
                        .id("quick-menu-copy-timestamp")
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .w_full()
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(7.0))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(palette::SURFACE_BG)))
                        .active(|style| style.bg(rgb(palette::ACTIVE_BG)))
                        .on_click(cx.listener(|this, _, window, _| {
                            this.copy_timestamp_and_dismiss(window);
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .min_w_0()
                                .flex_1()
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .px(px(5.0))
                                        .py(px(2.0))
                                        .rounded(px(5.0))
                                        .bg(rgb(palette::TS_TINT))
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(rgb(palette::TS_ACCENT))
                                        .child("时间"),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .min_w_0()
                                        .flex_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .text_color(rgb(palette::INK_PRIMARY))
                                                .whitespace_nowrap()
                                                .truncate()
                                                .child(datetime),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(rgb(palette::INK_TERTIARY))
                                                .whitespace_nowrap()
                                                .truncate()
                                                .child(subtitle),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .px(px(6.0))
                                .py(px(2.0))
                                .rounded(px(5.0))
                                .bg(rgb(palette::PILL_BG))
                                .text_xs()
                                .text_color(rgb(palette::INK_SECONDARY))
                                .child(unit_label),
                        ),
                )
                .child(
                    div()
                        .h(px(1.0))
                        .mx(px(6.0))
                        .mb(px(4.0))
                        .bg(rgb(palette::BORDER_SUBTLE)),
                )
        });

        let view_json_row = div()
            .id("quick-menu-view-json")
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .h(px(36.0))
            .px(px(10.0))
            .rounded(px(7.0))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(palette::SURFACE_BG)))
            .active(|style| style.bg(rgb(palette::ACTIVE_BG)))
            .on_click(cx.listener(|this, _, window, _| {
                this.trigger_open_editor(window);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .px(px(5.0))
                            .py(px(1.0))
                            .rounded(px(5.0))
                            .bg(rgb(palette::ACCENT_TINT))
                            .text_xs()
                            .font_semibold()
                            .text_color(rgb(palette::ACCENT))
                            .child("{}"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .text_color(rgb(palette::INK_PRIMARY))
                            .child("查看 JSON"),
                    ),
            )
            .children(has_selection.then(|| {
                div()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(rgb(palette::STATUS_OK))
            }));

        container.children(timestamp_section).child(view_json_row)
    }
}
