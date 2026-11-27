use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use gpui_kit::component::StyledExt;
use gpui_kit::{
    App, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Window, actions, div,
    px, rgb,
};
use input_hook::{DismissWatchGuard, ScreenPoint};
use json_engine::SmartInsight;

use crate::ui::panel::palette;

actions!(
    rpanel_menu,
    [DismissQuickMenu, ConfirmQuickMenu, CopyQuickInsight]
);

pub fn register_quick_menu_keybindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", DismissQuickMenu, Some("RPanelQuickMenu")),
        KeyBinding::new("enter", ConfirmQuickMenu, Some("RPanelQuickMenu")),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-c", CopyQuickInsight, Some("RPanelQuickMenu")),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-c", CopyQuickInsight, Some("RPanelQuickMenu")),
    ]);
}

/// Event sent when the user clicks "查看 JSON" on the floating quick-action popup.
#[derive(Debug, Clone)]
pub struct OpenEditorRequest {
    pub cursor: Option<ScreenPoint>,
    pub captured_text: Option<String>,
    pub is_explicit_selection: bool,
}

const MENU_BLUR_GRACE: Duration = Duration::from_millis(260);

/// Minimal Apple-style floating quick-action menu near the cursor.
/// Automatically surfaces a direct geek-oriented smart insight card (timestamp, date, JWT,
/// color, CIDR, Cron, radix, Unicode/URL/Base64) when the selection matches a known pattern.
pub struct QuickActionMenu {
    focus_handle: FocusHandle,
    cursor: Option<ScreenPoint>,
    captured_text: Option<String>,
    is_explicit_selection: bool,
    smart_insight: Option<SmartInsight>,
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
        is_explicit_selection: bool,
        smart_insight: Option<SmartInsight>,
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
            is_explicit_selection,
            smart_insight,
            open_editor_tx,
            rounded,
            opened_at: Instant::now(),
            ever_activated,
            _dismiss_watch: dismiss_watch,
            _subscriptions: vec![activation_sub],
        }
    }

    fn trigger_open_editor(&mut self, window: &mut Window) {
        let decoded_json = self
            .smart_insight
            .as_mut()
            .and_then(|insight| insight.editor_json.take());
        let (editor_payload, is_explicit) = if let Some(decoded) = decoded_json {
            (Some(decoded), true)
        } else {
            (self.captured_text.take(), self.is_explicit_selection)
        };
        let _ = self.open_editor_tx.send(OpenEditorRequest {
            cursor: self.cursor,
            captured_text: editor_payload,
            is_explicit_selection: is_explicit,
        });
        window.remove_window();
    }

    fn copy_insight_and_dismiss(&mut self, window: &mut Window) {
        if let Some(insight) = &self.smart_insight {
            let _ = input_hook::clipboard::write_clipboard_text(&insight.copy_text);
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
        let has_decoded_json = self
            .smart_insight
            .as_ref()
            .is_some_and(|insight| insight.editor_json.is_some());

        let container = div()
            .key_context("RPanelQuickMenu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &DismissQuickMenu, window, _| {
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &ConfirmQuickMenu, window, _| {
                this.trigger_open_editor(window);
            }))
            .on_action(cx.listener(|this, _: &CopyQuickInsight, window, _| {
                this.copy_insight_and_dismiss(window);
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

        let insight_section = self.smart_insight.as_ref().map(|insight| {
            let (tint_bg, _border, accent_fg) = palette::insight_colors(insight.tone);
            let badge = insight.badge.clone();
            let primary = insight.primary.clone();
            let subtitle = insight.subtitle.clone();
            let pill = insight.pill.clone();
            let swatch = insight.color_swatch;

            div()
                .flex()
                .flex_col()
                .w_full()
                .gap(px(4.0))
                .child(
                    div()
                        .id("quick-menu-copy-insight")
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
                            this.copy_insight_and_dismiss(window);
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .min_w_0()
                                .flex_1()
                                .children(swatch.map(|color_hex| {
                                    div()
                                        .flex_shrink_0()
                                        .size(px(16.0))
                                        .rounded(px(4.0))
                                        .bg(rgb(color_hex))
                                        .border_1()
                                        .border_color(rgb(palette::BORDER_OUTER))
                                }))
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .px(px(5.0))
                                        .py(px(2.0))
                                        .rounded(px(5.0))
                                        .bg(rgb(tint_bg))
                                        .text_xs()
                                        .font_semibold()
                                        .text_color(rgb(accent_fg))
                                        .child(badge),
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
                                                .child(primary),
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
                                .child(pill),
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

        let json_label = if has_decoded_json {
            "查看解码 JSON"
        } else {
            "查看 JSON"
        };

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
                            .child(json_label),
                    ),
            )
            .children(has_selection.then(|| {
                div()
                    .size(px(6.0))
                    .rounded_full()
                    .bg(rgb(palette::STATUS_OK))
            }));

        container.children(insight_section).child(view_json_row)
    }
}
