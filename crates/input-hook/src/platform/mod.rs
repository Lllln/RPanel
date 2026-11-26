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
