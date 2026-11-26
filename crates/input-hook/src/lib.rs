//! Cross-platform global right-button long-press interceptor and selection capture.

pub mod clipboard;
pub mod platform;
pub mod state_machine;
pub mod worker;

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

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
    dismiss_watch_count: AtomicUsize,
}

/// RAII guard that keeps global mouse-down dismissal observation active while a popup/panel is open.
#[derive(Debug)]
pub struct DismissWatchGuard {
    control: Arc<HookControl>,
}

impl Drop for DismissWatchGuard {
    fn drop(&mut self) {
        self.control
            .dismiss_watch_count
            .fetch_sub(1, Ordering::AcqRel);
    }
}

impl HookControl {
    pub fn new(config: HookConfig) -> Self {
        Self {
            enabled: AtomicBool::new(true),
            hold_ms: AtomicU64::new(config.hold_duration.as_millis() as u64),
            dismiss_watch_count: AtomicUsize::new(0),
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

    /// Returns `true` while at least one popup/panel wants to receive global mouse-down events
    /// for click-outside auto-dismissal.
    pub fn is_dismiss_watch_active(&self) -> bool {
        self.dismiss_watch_count.load(Ordering::Acquire) > 0
    }

    /// Increment the active dismiss-watch reference count and return an RAII guard that
    /// decrements it automatically when the popup/panel closes.
    pub fn acquire_dismiss_watch(self: &Arc<Self>) -> DismissWatchGuard {
        self.dismiss_watch_count.fetch_add(1, Ordering::AcqRel);
        DismissWatchGuard {
            control: Arc::clone(self),
        }
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
    /// Emitted (without swallowing) on global mouse-down while a popup/panel is open so the UI
    /// can dismiss itself reliably whenever the user clicks outside its bounds.
    GlobalMouseDown {
        position: ScreenPoint,
        at: Instant,
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

/// Disable the Windows DWM non-client frame/caption rendering (`DWMNCRP_DISABLED`) and native
/// corner clipping (`DWMWCP_DONOTROUND`) for a transparent DirectComposition `HWND`, so Windows 10
/// never shows its top ~30px caption strip behind transparent rounded top corners.
pub fn disable_dwm_window_frame(hwnd_raw: isize) {
    platform::disable_dwm_window_frame(hwnd_raw);
}

/// Check whether a raw OS hook screen coordinate lies inside a native window's bounding rectangle.
///
/// - On macOS, `native_handle` is the `NSView*` pointer from `AppKitWindowHandle::ns_view`.
/// - On Windows, `native_handle` is the `HWND` value from `Win32WindowHandle::hwnd`.
///
/// Returns `Some(true)` if inside, `Some(false)` if outside, or `None` if the handle could not
/// be queried (e.g. window already closed).
pub fn window_contains_screen_point(native_handle: isize, point: ScreenPoint) -> Option<bool> {
    platform::window_contains_screen_point(native_handle, point)
}

/// Process-lifetime guard ensuring only a single instance of RPanel runs per user session.
/// Dropping this guard releases the OS kernel lock (and the OS also releases it automatically on crash).
pub struct SingleInstanceGuard {
    _inner: platform::SingleInstanceGuardInner,
}

/// Attempt to acquire a per-session single-instance lock for `app_id`.
/// Returns `Some(guard)` if this process is the primary instance, or `None` if another instance
/// is already running.
pub fn acquire_single_instance(app_id: &str) -> Option<SingleInstanceGuard> {
    platform::acquire_single_instance(app_id).map(|inner| SingleInstanceGuard { _inner: inner })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_instance_guard_blocks_duplicate_until_dropped() {
        let id = format!(
            "com.rpanel.test.single-instance.{}",
            std::process::id()
        );
        let first = acquire_single_instance(&id);
        assert!(first.is_some(), "first acquisition should succeed");

        let second = acquire_single_instance(&id);
        assert!(second.is_none(), "second acquisition should be blocked while first is held");

        drop(first);

        let third = acquire_single_instance(&id);
        assert!(third.is_some(), "acquisition should succeed again after guard is dropped");
    }

    #[test]
    fn dismiss_watch_guard_tracks_active_window_count() {
        let control = Arc::new(HookControl::new(HookConfig::default()));
        assert!(!control.is_dismiss_watch_active());

        let g1 = control.acquire_dismiss_watch();
        assert!(control.is_dismiss_watch_active());

        let g2 = control.acquire_dismiss_watch();
        assert!(control.is_dismiss_watch_active());

        drop(g1);
        assert!(control.is_dismiss_watch_active());

        drop(g2);
        assert!(!control.is_dismiss_watch_active());
    }
}
