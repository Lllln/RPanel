pub mod ax_selection;
pub mod keyboard;
pub mod permission;

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use core_foundation::base::TCFType;
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::runloop::{CFRunLoop, CFRunLoopRef, CFRunLoopStop, kCFRunLoopCommonModes};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CGMouseButton, CallbackResult, EventField,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::CGPoint;
use crossbeam_channel::{Sender, unbounded};
use tracing::{debug, error, info, warn};

use crate::clipboard::{AutoCopyBaseline, CopyShortcutStep};
use crate::state_machine::{LongPressStateMachine, StateAction};
use crate::worker::{PlatformActions, WorkerCommand, capture_or_none, spawn_worker};
use crate::{
    CaptureResult, HookConfig, HookControl, HookError, HookHandle, RPANEL_INJECTED_TAG,
    ScreenPoint, clipboard,
};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

struct MacActions;

impl PlatformActions for MacActions {
    fn replay_short_click(&self, down_at: ScreenPoint, up_at: ScreenPoint) {
        if let Err(err) = post_right_mouse_event(CGEventType::RightMouseDown, down_at) {
            warn!("failed to replay macOS right-down: {err}");
            return;
        }
        thread::sleep(Duration::from_millis(5));
        if let Err(err) = post_right_mouse_event(CGEventType::RightMouseUp, up_at) {
            warn!("failed to replay macOS right-up: {err}");
        }
    }

    fn replay_drag_start(&self, down_at: ScreenPoint, current: ScreenPoint) {
        if let Err(err) = post_right_mouse_event(CGEventType::RightMouseDown, down_at) {
            warn!("failed to replay macOS right-drag-start down: {err}");
            return;
        }
        thread::sleep(Duration::from_millis(5));
        if let Err(err) = post_right_mouse_event(CGEventType::RightMouseDragged, current) {
            warn!("failed to replay macOS right-drag-start move: {err}");
        }
    }

    fn replay_drag_end(&self, up_at: ScreenPoint) {
        if let Err(err) = post_right_mouse_event(CGEventType::RightMouseUp, up_at) {
            warn!("failed to replay macOS right-drag-end up: {err}");
        }
    }

    fn capture_selection(&self) -> CaptureResult {
        capture_or_none(capture_selection_with_baseline(None))
    }

    fn clipboard_sequence_number(&self) -> Option<u64> {
        clipboard_sequence_number()
    }

    fn capture_selection_with_baseline(
        &self,
        auto_copy_baseline: Option<AutoCopyBaseline>,
    ) -> CaptureResult {
        capture_or_none(capture_selection_with_baseline(auto_copy_baseline))
    }
}

/// Start the macOS `CGEventTap` global right-click long-press interceptor.
pub fn start_hook(config: HookConfig) -> Result<HookHandle, HookError> {
    if !permission::is_accessibility_trusted(true) {
        return Err(HookError::AccessibilityPermissionRequired);
    }

    let (event_tx, event_rx) = unbounded();
    let control = Arc::new(HookControl::new(config));
    let state = Arc::new(Mutex::new(LongPressStateMachine::new(config)));

    let (cmd_tx, worker_handle) = spawn_worker(Arc::clone(&state), event_tx, MacActions);
    let (ready_tx, ready_rx) = unbounded::<Result<(), HookError>>();
    let runloop_ptr = Arc::new(AtomicPtr::<c_void>::new(std::ptr::null_mut()));

    let hook_control = Arc::clone(&control);
    let hook_state = Arc::clone(&state);
    let hook_cmd_tx = cmd_tx.clone();
    let hook_runloop_ptr = Arc::clone(&runloop_ptr);

    let tap_thread = thread::Builder::new()
        .name("rpanel-macos-eventtap".into())
        .spawn(move || {
            run_event_tap_loop(
                hook_control,
                hook_state,
                hook_cmd_tx,
                hook_runloop_ptr,
                ready_tx,
            );
        })
        .map_err(|e| HookError::InstallFailed(e.to_string()))?;

    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            let _ = cmd_tx.send(WorkerCommand::Shutdown);
            let _ = worker_handle.join();
            let _ = tap_thread.join();
            return Err(err);
        }
        Err(_) => {
            let _ = cmd_tx.send(WorkerCommand::Shutdown);
            let _ = worker_handle.join();
            let _ = tap_thread.join();
            return Err(HookError::InstallFailed(
                "macOS CGEventTap thread exited before signaling readiness".into(),
            ));
        }
    }

    let stop_cmd_tx = cmd_tx;
    let stop_runloop_ptr = runloop_ptr;
    Ok(HookHandle::new(event_rx, control, move || {
        let rl = stop_runloop_ptr.swap(std::ptr::null_mut(), Ordering::AcqRel);
        if !rl.is_null() {
            // SAFETY: `rl` is a valid CFRunLoopRef owned by the tap thread until it exits.
            unsafe { CFRunLoopStop(rl as CFRunLoopRef) };
        }
        let _ = stop_cmd_tx.send(WorkerCommand::Shutdown);
        let _ = tap_thread.join();
        let _ = worker_handle.join();
    }))
}

fn run_event_tap_loop(
    control: Arc<HookControl>,
    state: Arc<Mutex<LongPressStateMachine>>,
    cmd_tx: Sender<WorkerCommand>,
    runloop_ptr: Arc<AtomicPtr<c_void>>,
    ready_tx: Sender<Result<(), HookError>>,
) {
    let tap_port_ptr = Arc::new(AtomicPtr::<c_void>::new(std::ptr::null_mut()));
    let cb_tap_port = Arc::clone(&tap_port_ptr);

    let events_of_interest = vec![
        CGEventType::LeftMouseDown,
        CGEventType::OtherMouseDown,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::RightMouseDragged,
        CGEventType::MouseMoved,
    ];

    let tap = match CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        events_of_interest,
        move |_proxy, event_type, event| {
            handle_cg_event(event_type, event, &control, &state, &cmd_tx, &cb_tap_port)
        },
    ) {
        Ok(tap) => tap,
        Err(()) => {
            let _ = ready_tx.send(Err(HookError::InstallFailed(
                "CGEventTapCreate failed (check macOS Accessibility permission)".into(),
            )));
            return;
        }
    };

    tap_port_ptr.store(
        tap.mach_port().as_concrete_TypeRef() as *mut c_void,
        Ordering::Release,
    );

    let loop_source = match tap.mach_port().create_runloop_source(0) {
        Ok(source) => source,
        Err(()) => {
            let _ = ready_tx.send(Err(HookError::InstallFailed(
                "Failed to create CFRunLoopSource for CGEventTap".into(),
            )));
            return;
        }
    };

    let current_loop = CFRunLoop::get_current();
    runloop_ptr.store(
        current_loop.as_concrete_TypeRef() as *mut c_void,
        Ordering::Release,
    );

    // SAFETY: `kCFRunLoopCommonModes` is a valid CoreFoundation run loop mode constant.
    unsafe {
        current_loop.add_source(&loop_source, kCFRunLoopCommonModes);
    }
    tap.enable();

    info!("macOS CGEventTap active");
    let _ = ready_tx.send(Ok(()));

    CFRunLoop::run_current();
    runloop_ptr.store(std::ptr::null_mut(), Ordering::Release);
    debug!("macOS CGEventTap run loop stopped");
}

fn handle_cg_event(
    event_type: CGEventType,
    event: &CGEvent,
    control: &HookControl,
    state: &Mutex<LongPressStateMachine>,
    cmd_tx: &Sender<WorkerCommand>,
    tap_port_ptr: &AtomicPtr<c_void>,
) -> CallbackResult {
    match event_type {
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            let port = tap_port_ptr.load(Ordering::Acquire);
            if !port.is_null() {
                warn!("macOS CGEventTap was disabled by OS; re-enabling");
                // SAFETY: `port` is the live CFMachPortRef created on this thread.
                unsafe { CGEventTapEnable(port as CFMachPortRef, true) };
            }
            return CallbackResult::Keep;
        }
        _ => {}
    }

    // Never re-intercept synthetic events posted by RPanel itself.
    let user_data = event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA);
    if user_data == RPANEL_INJECTED_TAG as i64 {
        return CallbackResult::Keep;
    }

    let loc = event.location();
    let pos = ScreenPoint::new(loc.x.round() as i32, loc.y.round() as i32);
    let now = Instant::now();

    if matches!(event_type, CGEventType::LeftMouseDown) {
        let _ = cmd_tx.send(WorkerCommand::LeftMouseDown { at: now });
    }

    if matches!(
        event_type,
        CGEventType::LeftMouseDown | CGEventType::RightMouseDown | CGEventType::OtherMouseDown
    ) && control.is_dismiss_watch_active()
    {
        let _ = cmd_tx.send(WorkerCommand::GlobalMouseDown {
            position: pos,
            at: now,
        });
    }

    if matches!(
        event_type,
        CGEventType::LeftMouseDown | CGEventType::OtherMouseDown
    ) {
        return CallbackResult::Keep;
    }

    let action = {
        let mut sm = state.lock().unwrap_or_else(|e| e.into_inner());
        if !control.is_enabled() && sm.is_idle() {
            return CallbackResult::Keep;
        }
        // Pass through right-clicks in the macOS top menu bar (status/tray icons).
        if matches!(event_type, CGEventType::RightMouseDown)
            && sm.is_idle()
            && is_in_macos_menu_bar(pos)
        {
            return CallbackResult::Keep;
        }
        sm.set_hold_duration(control.hold_duration());

        match event_type {
            CGEventType::RightMouseDown => sm.on_right_down(pos, now),
            CGEventType::RightMouseDragged | CGEventType::MouseMoved => sm.on_mouse_move(pos),
            CGEventType::RightMouseUp => sm.on_right_up(pos, now),
            _ => StateAction::PassThrough,
        }
    };

    dispatch_action(action, now, cmd_tx)
}

/// Returns `true` when `pos` lies within the top status/menu bar strip of the primary display.
fn is_in_macos_menu_bar(pos: ScreenPoint) -> bool {
    let bounds = core_graphics::display::CGDisplay::main().bounds();
    let px = f64::from(pos.x);
    let py = f64::from(pos.y);
    px >= bounds.origin.x
        && px < bounds.origin.x + bounds.size.width
        && py >= bounds.origin.y
        && py < bounds.origin.y + 38.0
}

fn dispatch_action(
    action: StateAction,
    now: Instant,
    cmd_tx: &Sender<WorkerCommand>,
) -> CallbackResult {
    match action {
        StateAction::PassThrough => CallbackResult::Keep,
        StateAction::Swallow => CallbackResult::Drop,
        StateAction::SwallowAndArmTimer {
            generation,
            hold_duration,
        } => {
            let _ = cmd_tx.send(WorkerCommand::ArmTimer {
                generation,
                deadline: now + hold_duration,
            });
            CallbackResult::Drop
        }
        StateAction::ReplayShortClick { down_at, up_at } => {
            let _ = cmd_tx.send(WorkerCommand::ReplayShortClick { down_at, up_at });
            CallbackResult::Drop
        }
        StateAction::ReplayDragStart { down_at, current } => {
            let _ = cmd_tx.send(WorkerCommand::ReplayDragStart { down_at, current });
            CallbackResult::Drop
        }
        StateAction::ReplayDragEnd { up_at } => {
            let _ = cmd_tx.send(WorkerCommand::ReplayDragEnd { up_at });
            CallbackResult::Drop
        }
        StateAction::TriggerLongPress { position } => {
            let _ = cmd_tx.send(WorkerCommand::TriggerLongPress { position });
            CallbackResult::Drop
        }
    }
}

fn post_right_mouse_event(event_type: CGEventType, pos: ScreenPoint) -> Result<(), HookError> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| HookError::InstallFailed("Failed to create CGEventSource".into()))?;
    let point = CGPoint::new(f64::from(pos.x), f64::from(pos.y));
    let event = CGEvent::new_mouse_event(source, event_type, point, CGMouseButton::Right)
        .map_err(|_| HookError::InstallFailed("Failed to create synthetic CGEvent".into()))?;
    event.set_integer_value_field(
        EventField::EVENT_SOURCE_USER_DATA,
        RPANEL_INJECTED_TAG as i64,
    );
    event.post(CGEventTapLocation::HID);
    Ok(())
}

/// Query the lock-free `NSPasteboard.generalPasteboard().changeCount` on macOS.
pub fn clipboard_sequence_number() -> Option<u64> {
    objc2::rc::autoreleasepool(|_| unsafe {
        let cls = objc2::runtime::AnyClass::get(c"NSPasteboard")?;
        let pasteboard: *mut objc2::runtime::AnyObject = objc2::msg_send![cls, generalPasteboard];
        if pasteboard.is_null() {
            return None;
        }
        let count: isize = objc2::msg_send![pasteboard, changeCount];
        Some(count as u64)
    })
}

/// Detect whether the frontmost macOS application is a known terminal emulator / SSH client.
fn is_foreground_terminal() -> bool {
    objc2::rc::autoreleasepool(|_| unsafe {
        let Some(cls) = objc2::runtime::AnyClass::get(c"NSWorkspace") else {
            return false;
        };
        let workspace: *mut objc2::runtime::AnyObject = objc2::msg_send![cls, sharedWorkspace];
        if workspace.is_null() {
            return false;
        }
        let app: *mut objc2::runtime::AnyObject =
            objc2::msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return false;
        }

        let bundle_id: *mut objc2_foundation::NSString = objc2::msg_send![app, bundleIdentifier];
        if !bundle_id.is_null()
            && crate::platform::is_known_terminal_identifier(&(*bundle_id).to_string())
        {
            return true;
        }

        let name: *mut objc2_foundation::NSString = objc2::msg_send![app, localizedName];
        !name.is_null() && crate::platform::is_known_terminal_identifier(&(*name).to_string())
    })
}

/// Capture currently selected text on macOS:
/// 1. Try silent `AXUIElement` direct selection reading.
/// 2. Fall back to isolated `Cmd+C` (and for terminal/SSH apps like WindTerm, also `Cmd+Shift+C`
///    and `Ctrl+Shift+C`) using lock-free `NSPasteboard.changeCount` polling.
/// 3. Finally fall back to recent auto-copy-on-select when a terminal already copied on mouse-up.
pub fn capture_selection_with_baseline(
    auto_copy_baseline: Option<AutoCopyBaseline>,
) -> Result<CaptureResult, HookError> {
    if let Some(ax_result) = ax_selection::try_read_selected_text() {
        debug!("captured macOS selection via AXUIElement");
        return Ok(ax_result);
    }

    let is_terminal = is_foreground_terminal();
    debug!(is_terminal, "macOS AX selection unavailable; falling back to keyboard copy");

    let result = if is_terminal {
        let steps = [
            CopyShortcutStep {
                label: "cmd+c",
                timeout: Duration::from_millis(220),
                trigger: &keyboard::send_cmd_c,
            },
            CopyShortcutStep {
                label: "cmd+shift+c",
                timeout: Duration::from_millis(160),
                trigger: &keyboard::send_cmd_shift_c,
            },
            CopyShortcutStep {
                label: "ctrl+shift+c",
                timeout: Duration::from_millis(160),
                trigger: &keyboard::send_ctrl_shift_c,
            },
        ];
        clipboard::capture_via_shortcuts(&steps, auto_copy_baseline)
    } else {
        let steps = [CopyShortcutStep {
            label: "cmd+c",
            timeout: Duration::from_millis(260),
            trigger: &keyboard::send_cmd_c,
        }];
        clipboard::capture_via_shortcuts(&steps, auto_copy_baseline)
    };

    match result {
        Ok(res) => Ok(res),
        Err(err) => {
            error!("macOS keyboard clipboard capture failed: {err}");
            Err(err)
        }
    }
}

/// Check whether a Quartz global screen point (top-left origin) lies inside the `NSWindow`
/// owning `ns_view_raw`.
pub fn window_contains_screen_point(ns_view_raw: isize, point: ScreenPoint) -> Option<bool> {
    if ns_view_raw == 0 {
        return None;
    }
    // SAFETY: `ns_view_raw` is a live `NSView*` obtained from GPUI's AppKit window handle on the
    // main thread; we check for null before messaging `frame`.
    let frame: objc2_foundation::NSRect = unsafe {
        let ns_view = ns_view_raw as *mut objc2::runtime::AnyObject;
        let ns_window: *mut objc2::runtime::AnyObject = objc2::msg_send![ns_view, window];
        if ns_window.is_null() {
            return None;
        }
        objc2::msg_send![ns_window, frame]
    };

    let primary_height = core_graphics::display::CGDisplay::main().bounds().size.height;
    let left = frame.origin.x;
    let right = frame.origin.x + frame.size.width;
    let top = primary_height - (frame.origin.y + frame.size.height);
    let bottom = primary_height - frame.origin.y;
    let px = f64::from(point.x);
    let py = f64::from(point.y);
    Some(px >= left && px < right && py >= top && py < bottom)
}
