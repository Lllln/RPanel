use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use gpui_kit::component::StyledExt;
use gpui_kit::{
    App, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding,
    ParentElement, Render, StatefulInteractiveElement, Styled, Subscription, Window, actions, div,
    px, rgb,
};
use input_hook::ScreenPoint;

use crate::ui::panel::palette;

actions!(rpanel_menu, [DismissQuickMenu, ConfirmQuickMenu]);

pub fn register_quick_menu_keybindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", DismissQuickMenu, Some("RPanelQuickMenu")),
        KeyBinding::new("enter", ConfirmQuickMenu, Some("RPanelQuickMenu")),
    ]);
}

/// Event sent when the user clicks "查看 JSON" on the floating quick-action popup.
#[derive(Debug, Clone)]
pub struct OpenEditorRequest {
    pub cursor: Option<ScreenPoint>,
    pub captured_text: Option<String>,
}

const MENU_BLUR_GRACE: Duration = Duration::from_millis(260);

/// Minimal Apple-style floating quick-action menu showing "查看 JSON" near the cursor.
pub struct QuickActionMenu {
    focus_handle: FocusHandle,
    cursor: Option<ScreenPoint>,
    captured_text: Option<String>,
    open_editor_tx: Sender<OpenEditorRequest>,
    opened_at: Instant,
    _subscriptions: Vec<Subscription>,
}

impl QuickActionMenu {
    pub fn new(
        cursor: Option<ScreenPoint>,
        captured_text: Option<String>,
        open_editor_tx: Sender<OpenEditorRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        let activation_sub = cx.observe_window_activation(window, |this, window, _cx| {
            if !window.is_window_active() && this.opened_at.elapsed() >= MENU_BLUR_GRACE {
                window.remove_window();
            }
        });

        Self {
            focus_handle,
            cursor,
            captured_text,
            open_editor_tx,
            opened_at: Instant::now(),
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

        div()
            .key_context("RPanelQuickMenu")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &DismissQuickMenu, window, _| {
                window.remove_window();
            }))
            .on_action(cx.listener(|this, _: &ConfirmQuickMenu, window, _| {
                this.trigger_open_editor(window);
            }))
            .size_full()
            .p(px(2.0))
            .child(
                div()
                    .size_full()
                    .bg(rgb(palette::CARD_BG))
                    .border_1()
                    .border_color(rgb(palette::BORDER_OUTER))
                    .rounded(px(12.0))
                    .p(px(5.0))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .id("quick-menu-view-json")
                            .flex()
                            .items_center()
                            .justify_between()
                            .size_full()
                            .px(px(10.0))
                            .rounded(px(8.0))
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
                            })),
                    ),
            )
    }
}
