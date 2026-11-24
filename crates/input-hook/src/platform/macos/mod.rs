pub mod ax_selection;
pub mod keyboard;
pub mod permission;

use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

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
        if let Err(err) = post_right_mouse_event(CGEventType::RightMouseUp, up_at) {
            warn!("failed to replay macOS right-up: {err}");
        }
    }

    fn replay_drag_start(&self, down_at: ScreenPoint, current: ScreenPoint) {
        if let Err(err) = post_right_mouse_event(CGEventType::RightMouseDown, down_at) {
            warn!("failed to replay macOS right-drag-start down: {err}");
            return;
        }
        if let Err(err) = post_right_mouse_event(CGEventType::RightMouseDragged, current) {
            warn!("failed to replay macOS right-drag-start move: {err}");
        }
    }

    fn capture_selection(&self) -> CaptureResult {
        capture_or_none(capture_selection())
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

    let action = {
        let mut sm = state.lock().unwrap_or_else(|e| e.into_inner());
        if !control.is_enabled() && sm.is_idle() {
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

/// Capture currently selected text on macOS:
/// 1. Try silent `AXUIElement` direct selection reading.
/// 2. Fall back to clipboard backup + tagged `Cmd+C` + restore.
pub fn capture_selection() -> Result<CaptureResult, HookError> {
    if let Some(ax_result) = ax_selection::try_read_selected_text() {
        debug!("captured macOS selection via AXUIElement");
        return Ok(ax_result);
    }

    match clipboard::capture_via_clipboard_shortcut(keyboard::send_cmd_c) {
        Ok(res) => Ok(res),
        Err(err) => {
            error!("macOS Cmd+C clipboard capture failed: {err}");
            Err(err)
        }
    }
}
