#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(windows)]
pub mod windows;

use crate::{CaptureResult, HookConfig, HookError, HookHandle};

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

pub fn capture_selection() -> Result<CaptureResult, HookError> {
    #[cfg(target_os = "macos")]
    {
        macos::capture_selection()
    }

    #[cfg(windows)]
    {
        windows::capture_selection()
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Err(HookError::UnsupportedPlatform)
    }
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

pub fn configure_window_rounding(
    hwnd_raw: isize,
    is_popup: bool,
    direct_composition_enabled: bool,
) -> crate::WindowRoundStyle {
    #[cfg(windows)]
    {
        windows::configure_window_rounding(hwnd_raw, is_popup, direct_composition_enabled)
    }

    #[cfg(not(windows))]
    {
        let _ = (hwnd_raw, is_popup, direct_composition_enabled);
        crate::WindowRoundStyle::TransparentSurface
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
