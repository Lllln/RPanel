#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod placement;
mod state;
mod tray;
mod ui;

use std::env;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::unbounded;
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    ActivationPolicy, AnyWindowHandle, App, AppContext, Global, WindowBounds, WindowDecorations,
    WindowKind, WindowOptions,
};
use input_hook::{HookConfig, HookControl, HookError, HookEvent, HookHandle, ScreenPoint};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::placement::resolve_panel_placement;
use crate::state::AppSettings;
use crate::tray::{TrayCommand, TrayController};
use crate::ui::panel::{FloatingPanel, register_keybindings};

struct AppRuntime {
    settings: AppSettings,
    active_panel: Option<AnyWindowHandle>,
    tray: Option<TrayController>,
    hook_control: Option<Arc<HookControl>>,
    _hook_handle: Option<HookHandle>,
}

impl Global for AppRuntime {}

struct LaunchArgs {
    preview: bool,
    hold_ms: u64,
}

fn parse_args() -> LaunchArgs {
    let mut preview = false;
    let mut hold_ms = 300u64;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--preview" => preview = true,
            "--hold-ms" => {
                if let Some(val) = args.next().and_then(|v| v.parse::<u64>().ok()) {
                    hold_ms = val.max(100);
                }
            }
            _ => {}
        }
    }
    LaunchArgs { preview, hold_ms }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = parse_args();
    let initial_config = HookConfig {
        hold_duration: Duration::from_millis(args.hold_ms),
        ..HookConfig::default()
    };

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        cx.set_activation_policy(ActivationPolicy::Accessory);
        Theme::change(ThemeMode::Light, None, cx);
        register_keybindings(cx);

        let (tray_tx, tray_rx) = unbounded::<TrayCommand>();
        let tray = match TrayController::init(args.hold_ms, tray_tx) {
            Ok(controller) => Some(controller),
            Err(err) => {
                warn!("System tray unavailable: {err}");
                None
            }
        };

        let (hook_handle, hook_control, hook_rx) = match input_hook::start_hook(initial_config) {
            Ok(handle) => {
                info!(
                    hold_ms = args.hold_ms,
                    "Global right-click long-press hook started"
                );
                let control = Arc::clone(&handle.control);
                let rx = handle.events.clone();
                (Some(handle), Some(control), Some(rx))
            }
            Err(HookError::AccessibilityPermissionRequired) => {
                warn!(
                    "macOS Accessibility permission required; open System Settings -> Privacy & Security -> Accessibility"
                );
                #[cfg(target_os = "macos")]
                input_hook::platform::macos::permission::open_accessibility_settings();
                (None, None, None)
            }
            Err(err) => {
                error!("Failed to start global input hook: {err}");
                (None, None, None)
            }
        };

        cx.set_global(AppRuntime {
            settings: AppSettings {
                hook_enabled: true,
                hold_duration: initial_config.hold_duration,
            },
            active_panel: None,
            tray,
            hook_control,
            _hook_handle: hook_handle,
        });

        if args.preview {
            let sample = r#"{
  "app": "RPanel",
  "engine": "GPUI + gpui-component",
  "features": ["Right-Click Hold", "Smart JSONify", "Tree-sitter Highlight"],
  "ready": true
}"#;
            open_floating_panel(Some(sample.to_string()), None, false, cx);
        }

        // Poll tray & input-hook channels on a lightweight 16ms cadence inside GPUI's async loop.
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;

                let mut pending_tray = Vec::new();
                while let Ok(cmd) = tray_rx.try_recv() {
                    pending_tray.push(cmd);
                }

                let mut pending_hooks = Vec::new();
                if let Some(rx) = &hook_rx {
                    while let Ok(ev) = rx.try_recv() {
                        pending_hooks.push(ev);
                    }
                }

                if pending_tray.is_empty() && pending_hooks.is_empty() {
                    continue;
                }

                let should_break = cx.update(|cx| {
                    for cmd in pending_tray {
                        if handle_tray_command(cmd, cx) {
                            return true;
                        }
                    }
                    for ev in pending_hooks {
                        handle_hook_event(ev, cx);
                    }
                    false
                });

                if should_break {
                    break;
                }
            }
        })
        .detach();
    });
}

fn handle_tray_command(cmd: TrayCommand, cx: &mut App) -> bool {
    match cmd {
        TrayCommand::OpenFromClipboard => {
            let text = input_hook::clipboard::read_clipboard_text().ok();
            open_floating_panel(text, None, true, cx);
            false
        }
        TrayCommand::ToggleEnabled => {
            let next = {
                let runtime = cx.global_mut::<AppRuntime>();
                runtime.settings.hook_enabled = !runtime.settings.hook_enabled;
                if let Some(ctrl) = &runtime.hook_control {
                    ctrl.set_enabled(runtime.settings.hook_enabled);
                }
                if let Some(tray) = &runtime.tray {
                    tray.sync_enabled(runtime.settings.hook_enabled);
                }
                runtime.settings.hook_enabled
            };
            info!(enabled = next, "toggled global right-click hook");
            false
        }
        TrayCommand::SetHoldMs(ms) => {
            let duration = Duration::from_millis(ms);
            {
                let runtime = cx.global_mut::<AppRuntime>();
                runtime.settings.hold_duration = duration;
                if let Some(ctrl) = &runtime.hook_control {
                    ctrl.set_hold_duration(duration);
                }
                if let Some(tray) = &runtime.tray {
                    tray.sync_threshold(ms);
                }
            }
            info!(hold_ms = ms, "updated long-press threshold");
            false
        }
        TrayCommand::Quit => {
            info!("quitting RPanel from tray menu");
            cx.quit();
            true
        }
    }
}

fn handle_hook_event(event: HookEvent, cx: &mut App) {
    match event {
        HookEvent::LongPressTriggered { position, captured } => {
            let text = captured
                .text
                .filter(|s| !s.trim().is_empty())
                .or_else(|| input_hook::clipboard::read_clipboard_text().ok());
            open_floating_panel(text, Some(position), true, cx);
        }
        HookEvent::PermissionRequired => {
            warn!("Accessibility permission required to intercept mouse events");
        }
    }
}

fn open_floating_panel(
    initial_raw: Option<String>,
    cursor: Option<ScreenPoint>,
    auto_hide_on_blur: bool,
    cx: &mut App,
) {
    // Dismiss any previously open panel before spawning at the new cursor position.
    if let Some(prev) = cx.global_mut::<AppRuntime>().active_panel.take() {
        let _ = prev.update(cx, |_, window, _| {
            window.remove_window();
        });
    }

    let (display_id, bounds) = resolve_panel_placement(cursor, cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id,
        kind: WindowKind::Floating,
        titlebar: None,
        window_decorations: Some(WindowDecorations::Client),
        is_movable: true,
        is_resizable: true,
        is_minimizable: false,
        focus: true,
        show: true,
        ..Default::default()
    };

    match gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| FloatingPanel::new(initial_raw, auto_hide_on_blur, window, cx))
    }) {
        Ok((handle, _panel)) => {
            cx.global_mut::<AppRuntime>().active_panel = Some(handle);
            cx.activate(true);
        }
        Err(err) => {
            error!("Failed to open floating JSON panel: {err:?}");
        }
    }
}
