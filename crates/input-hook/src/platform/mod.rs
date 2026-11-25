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

pub fn apply_rounded_window(
    hwnd_raw: isize,
    radius_logical_px: i32,
    scale_factor: f32,
) -> crate::WindowRoundStyle {
    #[cfg(windows)]
    {
        windows::apply_rounded_window(hwnd_raw, radius_logical_px, scale_factor)
    }

    #[cfg(not(windows))]
    {
        let _ = (hwnd_raw, radius_logical_px, scale_factor);
        crate::WindowRoundStyle::TransparentSurface
    }
}
