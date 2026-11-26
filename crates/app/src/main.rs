#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod placement;
mod state;
mod tray;
mod ui;

use std::backtrace::Backtrace;
use std::env;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Sender, unbounded};
use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{
    ActivationPolicy, AnyWindowHandle, App, AppContext, Entity, Global, QuitMode, Render, Result,
    Styled, Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowKind,
    WindowOptions, rgb, transparent_black,
};
use input_hook::{
    HookConfig, HookControl, HookError, HookEvent, HookHandle, ScreenPoint, WindowRoundStyle,
};
#[cfg(windows)]
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

struct AppRuntime {
    settings: AppSettings,
    active_menu: Option<AnyWindowHandle>,
    active_panel: Option<AnyWindowHandle>,
    open_editor_tx: Sender<OpenEditorRequest>,
    tray: Option<TrayController>,
    hook_control: Option<Arc<HookControl>>,
    _hook_handle: Option<HookHandle>,
}

impl Global for AppRuntime {}

struct LaunchArgs {
    preview: bool,
    preview_menu: bool,
    hold_ms: u64,
}

fn parse_args() -> LaunchArgs {
    let mut preview = false;
    let mut preview_menu = false;
    let mut hold_ms = 300u64;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--preview" => preview = true,
            "--preview-menu" => preview_menu = true,
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
            });

            if args.preview_menu {
                open_quick_menu(
                    None,
                    Some(r#"{"app":"RPanel","mode":"quick-menu"}"#.to_string()),
                    cx,
                );
            } else if args.preview {
                let sample = r#"{
  "app": "RPanel",
  "engine": "GPUI + gpui-component",
  "features": ["Right-Click Hold", "Smart JSONify", "Pure-Rust JSON Highlight"],
  "ready": true
}"#;
                open_floating_panel(Some(sample.to_string()), None, false, cx);
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
            open_quick_menu(Some(position), selected, cx);
        }
        HookEvent::PermissionRequired => {
            warn!("Accessibility permission required to intercept mouse events");
        }
    }
}

fn open_quick_menu(cursor: Option<ScreenPoint>, captured_text: Option<String>, cx: &mut App) {
    let prev_menu = cx.global_mut::<AppRuntime>().active_menu.take();
    dismiss_window(prev_menu, cx);

    let open_editor_tx = cx.global::<AppRuntime>().open_editor_tx.clone();
    let (display_id, bounds) = resolve_quick_menu_placement(cursor, cx);

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id,
        kind: WindowKind::PopUp,
        titlebar: None,
        window_decorations: Some(WindowDecorations::Client),
        window_background: initial_window_background(true),
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        focus: true,
        show: true,
        ..Default::default()
    };

    match open_rounded_window(
        options,
        true,
        crate::ui::panel::palette::CARD_BG,
        cx,
        |round_style, window, cx| {
            cx.new(|cx| {
                QuickActionMenu::new(
                    cursor,
                    captured_text,
                    open_editor_tx,
                    round_style,
                    window,
                    cx,
                )
            })
        },
    ) {
        Ok((handle, _menu)) => {
            cx.global_mut::<AppRuntime>().active_menu = Some(handle);
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
    let (prev_menu, prev_panel) = {
        let runtime = cx.global_mut::<AppRuntime>();
        (runtime.active_menu.take(), runtime.active_panel.take())
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
        window_background: initial_window_background(false),
        is_movable: true,
        is_resizable: true,
        is_minimizable: false,
        focus: true,
        show: true,
        ..Default::default()
    };

    match open_rounded_window(
        options,
        false,
        crate::ui::panel::palette::SURFACE_BG,
        cx,
        |round_style, window, cx| {
            cx.new(|cx| FloatingPanel::new(initial_raw, auto_hide_on_blur, round_style, window, cx))
        },
    ) {
        Ok((handle, _panel)) => {
            cx.global_mut::<AppRuntime>().active_panel = Some(handle);
            cx.activate(true);
        }
        Err(err) => {
            error!("Failed to open floating JSON panel: {err:?}");
        }
    }
}

fn dismiss_window(handle: Option<AnyWindowHandle>, cx: &mut App) {
    if let Some(handle) = handle {
        let _ = handle.update(cx, |_, window, _| {
            window.remove_window();
        });
    }
}

fn is_direct_composition_disabled() -> bool {
    env::var("GPUI_DISABLE_DIRECT_COMPOSITION").is_ok_and(|value| value == "true" || value == "1")
}

fn initial_window_background(is_popup: bool) -> WindowBackgroundAppearance {
    if cfg!(windows) {
        if is_popup && !is_direct_composition_disabled() {
            WindowBackgroundAppearance::Transparent
        } else {
            WindowBackgroundAppearance::Opaque
        }
    } else {
        WindowBackgroundAppearance::Transparent
    }
}

/// Configure OS-level rounded corner strategy for the given GPUI window handle.
fn configure_os_window_rounding(window: &Window, is_popup: bool) -> WindowRoundStyle {
    #[cfg(windows)]
    {
        let direct_composition_enabled = !is_direct_composition_disabled();
        if let Ok(handle) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::Win32(win32) = handle.as_raw()
        {
            return input_hook::configure_window_rounding(
                win32.hwnd.get(),
                is_popup,
                direct_composition_enabled,
            );
        }
        return if direct_composition_enabled {
            WindowRoundStyle::TransparentSurface
        } else {
            WindowRoundStyle::SquareOpaque
        };
    }
    #[cfg(not(windows))]
    {
        let _ = (window, is_popup);
        WindowRoundStyle::TransparentSurface
    }
}

/// Open a GPUI window with OS-matched rounded corners and matching `Root` surface background
/// so unclipped white corner triangles never appear on Windows 10/11, VDI, or macOS.
fn open_rounded_window<V: Render>(
    options: WindowOptions,
    is_popup: bool,
    opaque_root_bg: u32,
    cx: &mut App,
    build: impl FnOnce(WindowRoundStyle, &mut Window, &mut App) -> Entity<V>,
) -> Result<(AnyWindowHandle, Entity<V>)> {
    let mut built = None;
    let initial_bg = options.window_background;
    let window = cx.open_window(options, |window, cx| {
        let round_style = configure_os_window_rounding(window, is_popup);
        let appearance = match round_style {
            WindowRoundStyle::TransparentSurface => WindowBackgroundAppearance::Transparent,
            WindowRoundStyle::NativeDwmRound | WindowRoundStyle::SquareOpaque => {
                WindowBackgroundAppearance::Opaque
            }
        };
        if appearance != initial_bg {
            window.set_background_appearance(appearance);
            // Re-assert DWM attributes (such as DWMNCRP_DISABLED) after updating composition attributes.
            let _ = configure_os_window_rounding(window, is_popup);
        }
        info!(
            ?round_style,
            is_popup,
            direct_composition_disabled = is_direct_composition_disabled(),
            "configured window rounding"
        );
        let view = build(round_style, window, cx);
        built = Some(view.clone());
        cx.new(|cx| {
            let root = gpui_kit::base::Root::new(view, window, cx);
            match round_style {
                WindowRoundStyle::TransparentSurface => root.bg(transparent_black()),
                WindowRoundStyle::NativeDwmRound | WindowRoundStyle::SquareOpaque => {
                    root.bg(rgb(opaque_root_bg))
                }
            }
        })
    })?;
    Ok((
        window.into(),
        built.expect("open_rounded_window ran its build closure"),
    ))
}
