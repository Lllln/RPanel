#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod placement;
mod state;
mod tray;
mod ui;

use std::backtrace::Backtrace;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Sender, unbounded};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    ActivationPolicy, AnyWindowHandle, App, AppContext, Entity, Global, QuitMode, Render, Result,
    Styled, Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowKind,
    WindowOptions, rgb, transparent_black,
};
use input_hook::{
    HookConfig, HookControl, HookError, HookEvent, HookHandle, ScreenPoint, SingleInstanceGuard,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

use crate::placement::{resolve_panel_placement, resolve_quick_menu_placement};
use crate::state::AppSettings;
use crate::tray::{TrayCommand, TrayController};
use crate::ui::panel::{FloatingPanel, register_keybindings};
use crate::ui::quick_menu::{
    OpenEditorRequest, QuickActionMenu, register_quick_menu_keybindings,
};

#[derive(Clone)]
struct TrackedWindow {
    handle: AnyWindowHandle,
    opened_at: Instant,
    dismiss_on_outside_click: bool,
    pinned: Arc<AtomicBool>,
}

struct AppRuntime {
    settings: AppSettings,
    active_menu: Option<TrackedWindow>,
    active_panel: Option<TrackedWindow>,
    open_editor_tx: Sender<OpenEditorRequest>,
    tray: Option<TrayController>,
    hook_control: Option<Arc<HookControl>>,
    _hook_handle: Option<HookHandle>,
    _single_instance: SingleInstanceGuard,
}

impl Global for AppRuntime {}

struct LaunchArgs {
    preview: bool,
    preview_menu: bool,
    preview_text: Option<String>,
    hold_ms: u64,
}

fn parse_args() -> LaunchArgs {
    let mut preview = false;
    let mut preview_menu = false;
    let mut preview_text = None;
    let mut hold_ms = 300u64;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--preview" => preview = true,
            "--preview-menu" => preview_menu = true,
            "--preview-text" => {
                preview_text = args.next();
            }
            "--hold-ms" => {
                if let Some(val) = args.next().and_then(|v| v.parse::<u64>().ok()) {
                    hold_ms = val.max(100);
                }
            }
            _ => {}
        }
    }
    LaunchArgs {
        preview,
        preview_menu,
        preview_text,
        hold_ms,
    }
}

fn install_panic_logger() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backtrace = Backtrace::force_capture();
        let report = format!(
            "=== RPanel Panic (unix={ts}) ===\n{info}\n\nBacktrace:\n{backtrace}\n\n"
        );
        let log_path = env::temp_dir().join("rpanel-crash.log");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = file.write_all(report.as_bytes());
        }
        eprintln!("{report}");
        eprintln!("Crash log saved to: {}", log_path.display());
        default_hook(info);
    }));
}

fn main() {
    install_panic_logger();
    input_hook::install_native_crash_handler();

    // Disable DirectComposition automatically inside Windows RDP/VDI sessions unless overridden.
    if input_hook::is_remote_desktop_session()
        && env::var_os("GPUI_DISABLE_DIRECT_COMPOSITION").is_none()
    {
        // SAFETY: Called on the main thread before spawning any background threads or GPUI runtime.
        unsafe {
            env::set_var("GPUI_DISABLE_DIRECT_COMPOSITION", "1");
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // Enforce single instance per user session so duplicate global right-click hooks
    // can never race with each other and break right-click across the OS.
    let Some(single_instance) = input_hook::acquire_single_instance("com.rpanel.app.single-instance")
    else {
        warn!("Another RPanel instance is already running; exiting duplicate launch");
        return;
    };

    let args = parse_args();
    let initial_config = HookConfig {
        hold_duration: Duration::from_millis(args.hold_ms),
        ..HookConfig::default()
    };

    gpui_kit::application()
        .with_quit_mode(QuitMode::Explicit)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.set_activation_policy(ActivationPolicy::Accessory);
            Theme::change(ThemeMode::Light, None, cx);
            register_keybindings(cx);
            register_quick_menu_keybindings(cx);

            let (tray_tx, tray_rx) = unbounded::<TrayCommand>();
            let (open_editor_tx, open_editor_rx) = unbounded::<OpenEditorRequest>();

            let tray = match TrayController::init(args.hold_ms, tray_tx) {
                Ok(controller) => Some(controller),
                Err(err) => {
                    warn!("System tray unavailable: {err}");
                    None
                }
            };

            let (hook_handle, hook_control, hook_rx) = match input_hook::start_hook(initial_config)
            {
                Ok(handle) => {
                    info!(
                        hold_ms = args.hold_ms,
                        "Global right-click long-press hook started (tray resident)"
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
                active_menu: None,
                active_panel: None,
                open_editor_tx,
                tray,
                hook_control,
                _hook_handle: hook_handle,
                _single_instance: single_instance,
            });

            if args.preview_menu {
                let text = args
                    .preview_text
                    .unwrap_or_else(|| r#"{"app":"RPanel","mode":"quick-menu"}"#.to_string());
                open_quick_menu(None, Some(text), cx);
            } else if args.preview {
                let sample = args.preview_text.unwrap_or_else(|| {
                    r#"{
  "app": "RPanel",
  "engine": "GPUI + gpui-component",
  "features": ["Right-Click Hold", "Smart JSONify", "Geek Smart Cards", "Pin Floating Panel"],
  "ready": true
}"#
                    .to_string()
                });
                open_floating_panel(Some(sample), None, false, cx);
            }

            // Poll tray, quick-menu, and input-hook channels on a lightweight 16ms cadence.
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

                    let mut pending_opens = Vec::new();
                    while let Ok(req) = open_editor_rx.try_recv() {
                        pending_opens.push(req);
                    }

                    if pending_tray.is_empty()
                        && pending_hooks.is_empty()
                        && pending_opens.is_empty()
                    {
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
                        for req in pending_opens {
                            open_floating_panel(req.captured_text, req.cursor, true, cx);
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
            // Explicitly unhook the global mouse hook before terminating the UI event loop.
            let runtime = cx.global_mut::<AppRuntime>();
            runtime._hook_handle.take();
            runtime.tray.take();
            cx.quit();
            true
        }
    }
}

fn handle_hook_event(event: HookEvent, cx: &mut App) {
    match event {
        HookEvent::LongPressTriggered { position, captured } => {
            // Keep only genuine captured selection; clicking "查看 JSON" opens the editor
            // with this selection auto-converted (or empty if nothing was selected).
            let selected = captured.text.filter(|s| !s.trim().is_empty());
            info!(
                source = ?captured.source,
                captured_chars = selected.as_ref().map(|s| s.chars().count()).unwrap_or(0),
                "long-press triggered"
            );
            open_quick_menu(Some(position), selected, cx);
        }
        HookEvent::GlobalMouseDown { position, at } => {
            handle_global_mouse_down(position, at, cx);
        }
        HookEvent::PermissionRequired => {
            warn!("Accessibility permission required to intercept mouse events");
        }
    }
}

fn handle_global_mouse_down(position: ScreenPoint, at: Instant, cx: &mut App) {
    let (menu, panel) = {
        let runtime = cx.global::<AppRuntime>();
        (runtime.active_menu.clone(), runtime.active_panel.clone())
    };

    if let Some(tracked) = menu
        && should_clear_after_outside_click(&tracked, position, at, cx)
    {
        let runtime = cx.global_mut::<AppRuntime>();
        if runtime
            .active_menu
            .as_ref()
            .is_some_and(|current| current.handle == tracked.handle)
        {
            runtime.active_menu = None;
        }
    }

    if let Some(tracked) = panel
        && should_clear_after_outside_click(&tracked, position, at, cx)
    {
        let runtime = cx.global_mut::<AppRuntime>();
        if runtime
            .active_panel
            .as_ref()
            .is_some_and(|current| current.handle == tracked.handle)
        {
            runtime.active_panel = None;
        }
    }
}

/// Inspect `tracked` against a global mouse-down at `position`.
/// Closes the window if the click occurred after it opened and lies outside its OS bounds.
/// Returns `true` when the tracked slot should be cleared (dismissed or already closed).
fn should_clear_after_outside_click(
    tracked: &TrackedWindow,
    position: ScreenPoint,
    at: Instant,
    cx: &mut App,
) -> bool {
    if !tracked.dismiss_on_outside_click
        || tracked.pinned.load(Ordering::Acquire)
        || at <= tracked.opened_at
    {
        return false;
    }

    tracked
        .handle
        .update(cx, |_, window, _| {
            let Some(raw_handle) = native_window_handle(window) else {
                return false;
            };
            if input_hook::window_contains_screen_point(raw_handle, position) == Some(false) {
                window.remove_window();
                true
            } else {
                false
            }
        })
        .unwrap_or(true)
}

fn native_window_handle(window: &Window) -> Option<isize> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        #[cfg(target_os = "macos")]
        RawWindowHandle::AppKit(appkit) => Some(appkit.ns_view.as_ptr() as isize),
        #[cfg(windows)]
        RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
        _ => None,
    }
}

fn open_quick_menu(cursor: Option<ScreenPoint>, captured_text: Option<String>, cx: &mut App) {
    let (prev_menu, open_editor_tx, dismiss_watch) = {
        let runtime = cx.global_mut::<AppRuntime>();
        let prev = runtime.active_menu.take();
        let tx = runtime.open_editor_tx.clone();
        let watch = runtime
            .hook_control
            .as_ref()
            .map(|ctrl| ctrl.acquire_dismiss_watch());
        (prev, tx, watch)
    };
    dismiss_window(prev_menu, cx);

    let smart_insight = captured_text
        .as_deref()
        .and_then(json_engine::detect_smart_insight);
    let (display_id, bounds) =
        resolve_quick_menu_placement(cursor, smart_insight.is_some(), cx);

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id,
        kind: WindowKind::PopUp,
        titlebar: None,
        window_decorations: Some(WindowDecorations::Client),
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        focus: true,
        show: true,
        ..Default::default()
    };

    match open_floating_window(
        options,
        crate::ui::panel::palette::CARD_BG,
        cx,
        |rounded, window, cx| {
            cx.new(|cx| {
                QuickActionMenu::new(
                    cursor,
                    captured_text,
                    smart_insight,
                    open_editor_tx,
                    rounded,
                    dismiss_watch,
                    window,
                    cx,
                )
            })
        },
    ) {
        Ok((handle, _menu)) => {
            cx.global_mut::<AppRuntime>().active_menu = Some(TrackedWindow {
                handle,
                opened_at: Instant::now(),
                dismiss_on_outside_click: true,
                pinned: Arc::new(AtomicBool::new(false)),
            });
            cx.activate(true);
        }
        Err(err) => {
            error!("Failed to open quick action menu: {err:?}");
        }
    }
}

fn open_floating_panel(
    initial_raw: Option<String>,
    cursor: Option<ScreenPoint>,
    auto_hide_on_blur: bool,
    cx: &mut App,
) {
    let initial_pinned = !auto_hide_on_blur;
    let pinned = Arc::new(AtomicBool::new(initial_pinned));

    let (prev_menu, prev_panel, hook_control, dismiss_watch) = {
        let runtime = cx.global_mut::<AppRuntime>();
        let prev_menu = runtime.active_menu.take();
        let prev_panel = runtime.active_panel.take();
        let hook_control = runtime.hook_control.clone();
        let watch = if !initial_pinned {
            hook_control.as_ref().map(|ctrl| ctrl.acquire_dismiss_watch())
        } else {
            None
        };
        (prev_menu, prev_panel, hook_control, watch)
    };
    dismiss_window(prev_menu, cx);
    dismiss_window(prev_panel, cx);

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

    let panel_pinned = Arc::clone(&pinned);
    match open_floating_window(
        options,
        crate::ui::panel::palette::SURFACE_BG,
        cx,
        |rounded, window, cx| {
            cx.new(|cx| {
                FloatingPanel::new(
                    initial_raw,
                    panel_pinned,
                    rounded,
                    hook_control,
                    dismiss_watch,
                    window,
                    cx,
                )
            })
        },
    ) {
        Ok((handle, _panel)) => {
            cx.global_mut::<AppRuntime>().active_panel = Some(TrackedWindow {
                handle,
                opened_at: Instant::now(),
                dismiss_on_outside_click: true,
                pinned,
            });
            cx.activate(true);
        }
        Err(err) => {
            error!("Failed to open floating JSON panel: {err:?}");
        }
    }
}

fn dismiss_window(tracked: Option<TrackedWindow>, cx: &mut App) {
    if let Some(tracked) = tracked {
        let _ = tracked.handle.update(cx, |_, window, _| {
            window.remove_window();
        });
    }
}

fn supports_transparent_surface() -> bool {
    !cfg!(windows)
        || !env::var("GPUI_DISABLE_DIRECT_COMPOSITION")
            .is_ok_and(|value| value == "true" || value == "1")
}

/// Open a frameless floating GPUI window:
/// - On macOS and Windows desktop (DirectComposition enabled), uses a transparent surface and
///   disables Windows DWM non-client frame rendering so all 4 rounded corners are cleanly cut.
/// - On Windows RDP/VDI (`GPUI_DISABLE_DIRECT_COMPOSITION=1`), falls back to an opaque square
///   surface because `DXGI_ALPHA_MODE_IGNORE` cannot render transparent corners.
fn open_floating_window<V: Render>(
    mut options: WindowOptions,
    opaque_root_bg: u32,
    cx: &mut App,
    build: impl FnOnce(bool, &mut Window, &mut App) -> Entity<V>,
) -> Result<(AnyWindowHandle, Entity<V>)> {
    let transparent = supports_transparent_surface();
    options.window_background = if transparent {
        WindowBackgroundAppearance::Transparent
    } else {
        WindowBackgroundAppearance::Opaque
    };

    let mut built = None;
    let window = cx.open_window(options, |window, cx| {
        if transparent && let Some(hwnd) = native_window_handle(window) {
            input_hook::disable_dwm_window_frame(hwnd);
        }
        let view = build(transparent, window, cx);
        built = Some(view.clone());
        cx.new(|cx| {
            let root = gpui_kit::base::Root::new(view, window, cx);
            if transparent {
                root.bg(transparent_black())
            } else {
                root.bg(rgb(opaque_root_bg))
            }
        })
    })?;
    Ok((
        window.into(),
        built.expect("open_floating_window ran its build closure"),
    ))
}
