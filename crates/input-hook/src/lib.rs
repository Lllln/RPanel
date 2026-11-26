//! Cross-platform global right-button long-press interceptor and selection capture.

pub mod clipboard;
pub mod platform;
pub mod state_machine;
pub mod worker;

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

use crossbeam_channel::Receiver;
use thiserror::Error;

/// Magic tag embedded in synthetic mouse/keyboard events to prevent hook recursion.
/// ASCII "RPANEL01" packed as a 64-bit marker.
pub const RPANEL_INJECTED_TAG: u64 = 0x5250_414E_454C_0001;

/// Screen coordinate in platform global coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenPoint {
    pub x: i32,
    pub y: i32,
}

impl ScreenPoint {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub fn distance_squared(self, other: Self) -> i64 {
        let dx = i64::from(self.x) - i64::from(other.x);
        let dy = i64::from(self.y) - i64::from(other.y);
        dx * dx + dy * dy
    }
}

/// Configuration for right-button long-press detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookConfig {
    /// Minimum hold duration before treating right-press as long-press (default: 300ms).
    pub hold_duration: Duration,
    /// Maximum pointer movement in pixels before cancelling long-press (default: 8px).
    pub max_move_px: i32,
}

impl Default for HookConfig {
    fn default() -> Self {
        Self {
            hold_duration: Duration::from_millis(300),
            max_move_px: 8,
        }
    }
}

/// Runtime shared controls that can be mutated safely from tray/UI threads.
#[derive(Debug)]
pub struct HookControl {
    enabled: AtomicBool,
    hold_ms: AtomicU64,
}

impl HookControl {
    pub fn new(config: HookConfig) -> Self {
        Self {
            enabled: AtomicBool::new(true),
            hold_ms: AtomicU64::new(config.hold_duration.as_millis() as u64),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn hold_duration(&self) -> Duration {
        Duration::from_millis(self.hold_ms.load(Ordering::Relaxed).max(100))
    }

    pub fn set_hold_duration(&self, duration: Duration) {
        self.hold_ms
            .store((duration.as_millis() as u64).max(100), Ordering::Relaxed);
    }
}

/// Outcome of attempting to capture the currently selected text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureResult {
    pub text: Option<String>,
    pub source: CaptureSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSource {
    Accessibility,
    ClipboardShortcut,
    None,
}

/// Events emitted from the global mouse hook to the desktop app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookEvent {
    LongPressTriggered {
        position: ScreenPoint,
        captured: CaptureResult,
    },
    PermissionRequired,
}

#[derive(Debug, Error)]
pub enum HookError {
    #[error("Accessibility permission is required on macOS to intercept global mouse events")]
    AccessibilityPermissionRequired,
    #[error("Failed to install OS input hook: {0}")]
    InstallFailed(String),
    #[error("Clipboard operation failed: {0}")]
    ClipboardFailed(String),
    #[error("Platform is not supported by input-hook")]
    UnsupportedPlatform,
}

/// Handle owning the background hook thread and shared runtime controls.
pub struct HookHandle {
    pub events: Receiver<HookEvent>,
    pub control: Arc<HookControl>,
    stop_signal: Option<Box<dyn FnOnce() + Send>>,
}

impl HookHandle {
    pub fn new(
        events: Receiver<HookEvent>,
        control: Arc<HookControl>,
        stop_signal: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self {
            events,
            control,
            stop_signal: Some(Box::new(stop_signal)),
        }
    }

    pub fn stop(mut self) {
        if let Some(stop) = self.stop_signal.take() {
            stop();
        }
    }
}

impl Drop for HookHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop_signal.take() {
            stop();
        }
    }
}

/// Start the platform global right-click long-press hook.
pub fn start_hook(config: HookConfig) -> Result<HookHandle, HookError> {
    platform::start_platform_hook(config)
}

/// Capture selected text using platform best-effort strategy.
pub fn capture_selection() -> Result<CaptureResult, HookError> {
    platform::capture_selection()
}

/// Convert a raw OS hook screen coordinate into logical pixels for window placement.
/// On macOS `CGEvent` coordinates are already in logical points; on Windows this converts
/// physical pixels using the target monitor's DPI scale factor.
pub fn to_logical_point(point: ScreenPoint) -> ScreenPoint {
    platform::to_logical_point(point)
}

/// Returns `true` when running inside a Windows Remote Desktop / VDI session (`SM_REMOTESESSION`).
pub fn is_remote_desktop_session() -> bool {
    platform::is_remote_desktop_session()
}

/// Install a platform native exception logger (Windows Vectored Exception Handler) that records
/// fatal SEH exceptions before Windows converts them to `0xC000041D` inside `WndProc`.
pub fn install_native_crash_handler() {
    platform::install_native_crash_handler();
}

/// How the OS window frame handles rounded corners on the current platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowRoundStyle {
    /// Windows 11 DWM native anti-aliased rounded corners (`DWMWCP_ROUND`) and border.
    NativeDwmRound,
    /// macOS or Windows DirectComposition per-pixel alpha-composited rounded corners.
    TransparentSurface,
    /// Windows 10 / Server / VDI without DWM corner rounding or DirectComposition alpha
    /// (`DXGI_ALPHA_MODE_IGNORE` flip-model swapchain): render flush square corners so
    /// unclipped white corner triangles never appear.
    SquareOpaque,
}

/// Configure OS-level window corner handling for a Win32 `HWND` based on DWM capability
/// and whether GPUI's DirectComposition alpha swapchain is active.
pub fn configure_window_rounding(
    hwnd_raw: isize,
    is_popup: bool,
    direct_composition_enabled: bool,
) -> WindowRoundStyle {
    platform::configure_window_rounding(hwnd_raw, is_popup, direct_composition_enabled)
}
