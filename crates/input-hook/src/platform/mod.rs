#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(windows)]
pub mod windows;

use crate::clipboard::AutoCopyBaseline;
use crate::{CaptureResult, GlobalHotkeyHandle, HookConfig, HookError, HookHandle};

pub fn start_platform_hook(config: HookConfig) -> Result<HookHandle, HookError> {
    #[cfg(target_os = "macos")]
    {
        macos::start_hook(config)
    }

    #[cfg(windows)]
    {
        windows::start_hook(config)
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = config;
        Err(HookError::UnsupportedPlatform)
    }
}

pub fn start_platform_global_hotkey() -> Result<GlobalHotkeyHandle, HookError> {
    #[cfg(target_os = "macos")]
    {
        macos::hotkey::start_global_hotkey()
    }

    #[cfg(windows)]
    {
        windows::hotkey::start_global_hotkey()
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Err(HookError::UnsupportedPlatform)
    }
}

pub fn current_cursor_position() -> Option<crate::ScreenPoint> {
    #[cfg(target_os = "macos")]
    {
        macos::current_cursor_position()
    }

    #[cfg(windows)]
    {
        windows::current_cursor_position()
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}

pub fn try_capture_selection_non_intrusive() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        macos::try_capture_selection_non_intrusive()
    }

    #[cfg(windows)]
    {
        windows::try_capture_selection_non_intrusive()
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}

pub fn capture_selection() -> Result<CaptureResult, HookError> {
    capture_selection_with_baseline(None)
}

pub fn capture_selection_with_baseline(
    baseline: Option<AutoCopyBaseline>,
) -> Result<CaptureResult, HookError> {
    #[cfg(target_os = "macos")]
    {
        macos::capture_selection_with_baseline(baseline)
    }

    #[cfg(windows)]
    {
        windows::capture_selection_with_baseline(baseline)
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = baseline;
        Err(HookError::UnsupportedPlatform)
    }
}

pub fn clipboard_sequence_number() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        macos::clipboard_sequence_number()
    }

    #[cfg(windows)]
    {
        windows::clipboard_sequence_number()
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}

/// Match an executable name, macOS bundle ID, or localized app name against known
/// terminal emulators and SSH/shell clients where `Ctrl+C` sends `SIGINT` or where
/// `Ctrl+Shift+C` / `Ctrl+Insert` / `Cmd+Shift+C` is used for copying.
pub(crate) fn is_known_terminal_identifier(identifier: &str) -> bool {
    let lower = identifier.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }

    // Normalize path/extension (e.g. `C:\Program Files\WindTerm\WindTerm.exe` -> `windterm`).
    let basename = lower
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(&lower);
    let stem = basename.strip_suffix(".exe").unwrap_or(basename);

    const EXACT_EXES: &[&str] = &[
        "windterm",
        "windowsterminal",
        "wt",
        "cmd",
        "powershell",
        "pwsh",
        "conhost",
        "openconsole",
        "putty",
        "kitty",
        "kitty_portable",
        "superputty",
        "xshell",
        "xshellcore",
        "xshellplus",
        "mobaxterm",
        "mobaxterm_personal",
        "securecrt",
        "finalshell",
        "tabby",
        "wezterm",
        "wezterm-gui",
        "alacritty",
        "mintty",
        "conemu",
        "conemu64",
        "conemuc",
        "conemuc64",
        "cmder",
        "hyper",
        "electerm",
        "termius",
        "warp",
        "rio",
        "ghostty",
        "fluentterminal",
        "ttermpro",
        "smartty",
        "wsl",
        "bash",
        "git-bash",
        "mremoteng",
        "royalts",
        "bvssh",
        "terminal",
        "iterm",
        "iterm2",
        "xterminal",
    ];

    if EXACT_EXES.contains(&stem) {
        return true;
    }

    const SUBSTRINGS: &[&str] = &[
        "windterm",
        "windowsterminal",
        "iterm",
        "apple.terminal",
        "alacritty",
        "wezterm",
        "ghostty",
        "tabby",
        "finalshell",
        "xshell",
        "mobaxterm",
        "securecrt",
        "electerm",
        "termius",
        "warp-stable",
        "putty",
        "mintty",
        "conemu",
        "xterminal",
        "royaltsx",
    ];

    SUBSTRINGS.iter().any(|needle| lower.contains(needle))
}

/// Match Win32 window class names belonging to terminal / console windows.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn is_known_terminal_window_class(class_name: &str) -> bool {
    matches!(
        class_name.trim(),
        "ConsoleWindowClass"
            | "CASCADIA_HOSTING_WINDOW_CLASS"
            | "PuTTY"
            | "mintty"
            | "VirtualConsoleClass"
            | "VirtualConsoleClassGhost"
    )
}

/// Match Win32 window classes where global right-click interception must pass through natively
/// without swallowing/replaying (taskbar, system tray, tray overflow flyouts, active popup menus).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn is_bypass_right_click_window_class(class_name: &str) -> bool {
    matches!(
        class_name.trim(),
        "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "NotifyIconOverflowWindow"
            | "TopLevelWindowForOverflowXamlIsland"
            | "TrayNotifyWnd"
            | "TrayClockWClass"
            | "SysPager"
            | "TaskListThumbnailWnd"
            | "Xaml_WindowedPopupClass"
            | "Windows.UI.Core.CoreWindow"
            | "NotifyIconWindow"
            | "#32768"
    )
}

pub fn to_logical_point(point: crate::ScreenPoint) -> crate::ScreenPoint {
    #[cfg(windows)]
    {
        windows::to_logical_point(point)
    }

    #[cfg(not(windows))]
    {
        point
    }
}

pub fn is_remote_desktop_session() -> bool {
    #[cfg(windows)]
    {
        windows::is_remote_desktop_session()
    }

    #[cfg(not(windows))]
    {
        false
    }
}

pub fn install_native_crash_handler() {
    #[cfg(windows)]
    {
        windows::install_native_crash_handler();
    }
}

pub fn disable_dwm_window_frame(hwnd_raw: isize) {
    #[cfg(windows)]
    {
        windows::disable_dwm_window_frame(hwnd_raw);
    }

    #[cfg(not(windows))]
    {
        let _ = hwnd_raw;
    }
}

pub fn window_contains_screen_point(
    native_handle: isize,
    point: crate::ScreenPoint,
) -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        macos::window_contains_screen_point(native_handle, point)
    }

    #[cfg(windows)]
    {
        windows::window_contains_screen_point(native_handle, point)
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (native_handle, point);
        None
    }
}

#[cfg(windows)]
pub struct SingleInstanceGuardInner {
    _guard: windows::WinSingleInstanceGuard,
}

#[cfg(not(windows))]
pub struct SingleInstanceGuardInner {
    _file: std::fs::File,
}

pub fn acquire_single_instance(app_id: &str) -> Option<SingleInstanceGuardInner> {
    #[cfg(windows)]
    {
        windows::acquire_single_instance(app_id).map(|guard| SingleInstanceGuardInner {
            _guard: guard,
        })
    }

    #[cfg(not(windows))]
    {
        use std::fs::{OpenOptions, TryLockError};

        let sanitized: String = app_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let lock_path = std::env::temp_dir().join(format!("{sanitized}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .ok()?;

        match file.try_lock() {
            Ok(()) => Some(SingleInstanceGuardInner { _file: file }),
            Err(TryLockError::WouldBlock) => None,
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_common_windows_and_macos_terminals() {
        assert!(is_known_terminal_identifier("WindTerm.exe"));
        assert!(is_known_terminal_identifier(
            r"C:\Tools\WindTerm\WindTerm.exe"
        ));
        assert!(is_known_terminal_identifier("com.kingtoolbox.WindTerm"));
        assert!(is_known_terminal_identifier("WindowsTerminal.exe"));
        assert!(is_known_terminal_identifier("Xshell.exe"));
        assert!(is_known_terminal_identifier("MobaXterm_Personal.exe"));
        assert!(is_known_terminal_identifier("putty.exe"));
        assert!(is_known_terminal_identifier("mintty.exe"));
        assert!(is_known_terminal_identifier("com.googlecode.iterm2"));
        assert!(is_known_terminal_identifier("com.apple.Terminal"));
        assert!(is_known_terminal_identifier("dev.warp.Warp-Stable"));

        assert!(is_known_terminal_window_class("ConsoleWindowClass"));
        assert!(is_known_terminal_window_class(
            "CASCADIA_HOSTING_WINDOW_CLASS"
        ));
        assert!(is_known_terminal_window_class("PuTTY"));
        assert!(is_known_terminal_window_class("mintty"));
    }

    #[test]
    fn does_not_misclassify_regular_editors_and_browsers() {
        assert!(!is_known_terminal_identifier("notepad.exe"));
        assert!(!is_known_terminal_identifier("chrome.exe"));
        assert!(!is_known_terminal_identifier("Code.exe"));
        assert!(!is_known_terminal_identifier("com.apple.TextEdit"));
        assert!(!is_known_terminal_identifier("com.google.Chrome"));
        assert!(!is_known_terminal_identifier("com.microsoft.VSCode"));
        assert!(!is_known_terminal_window_class("Notepad"));
        assert!(!is_known_terminal_window_class("Chrome_WidgetWin_1"));
    }

    #[test]
    fn bypasses_windows_tray_taskbar_and_popup_menu_classes() {
        assert!(is_bypass_right_click_window_class("Shell_TrayWnd"));
        assert!(is_bypass_right_click_window_class("Shell_SecondaryTrayWnd"));
        assert!(is_bypass_right_click_window_class("NotifyIconOverflowWindow"));
        assert!(is_bypass_right_click_window_class(
            "TopLevelWindowForOverflowXamlIsland"
        ));
        assert!(is_bypass_right_click_window_class("#32768"));

        assert!(!is_bypass_right_click_window_class("Notepad"));
        assert!(!is_bypass_right_click_window_class("Chrome_WidgetWin_1"));
    }
}
