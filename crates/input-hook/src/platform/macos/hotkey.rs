//! Native macOS global hotkey registration (`Ctrl + Option + Space`) via Carbon HIToolbox.
//!
//! Unlike `CGEventTap` for keyboard events, Carbon's `RegisterEventHotKey` requires **zero**
//! extra macOS Input Monitoring permissions and dispatches on the app's main runloop.

use std::ffi::c_void;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crossbeam_channel::{Sender, unbounded};
use tracing::{debug, info};

use crate::{GlobalHotkeyEvent, GlobalHotkeyHandle, HookError};

const EVENT_CLASS_KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const EVENT_HOTKEY_PRESSED: u32 = 5;
const HOTKEY_SIGNATURE: u32 = u32::from_be_bytes(*b"RPNL");
const HOTKEY_ID: u32 = 1;
const KVK_SPACE: u32 = 49;
const OPTION_KEY: u32 = 1 << 11;
const CONTROL_KEY: u32 = 1 << 12;
const EVENT_NOT_HANDLED_ERR: i32 = -9874;

#[repr(C)]
#[derive(Clone, Copy)]
struct EventTypeSpec {
    event_class: u32,
    event_kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct EventHotKeyID {
    signature: u32,
    id: u32,
}

type EventHandlerProc = unsafe extern "C" fn(
    next_handler: *mut c_void,
    event: *mut c_void,
    user_data: *mut c_void,
) -> i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> *mut c_void;
    fn InstallEventHandler(
        target: *mut c_void,
        handler: EventHandlerProc,
        num_types: usize,
        list: *const EventTypeSpec,
        user_data: *mut c_void,
        out_ref: *mut *mut c_void,
    ) -> i32;
    fn RemoveEventHandler(handler_ref: *mut c_void) -> i32;
    fn RegisterEventHotKey(
        hot_key_code: u32,
        hot_key_modifiers: u32,
        hot_key_id: EventHotKeyID,
        target: *mut c_void,
        options: u32,
        out_ref: *mut *mut c_void,
    ) -> i32;
    fn UnregisterEventHotKey(hot_key_ref: *mut c_void) -> i32;
}

struct HotkeyCallbackState {
    enabled: Arc<AtomicBool>,
    tx: Sender<GlobalHotkeyEvent>,
}

unsafe extern "C" fn hotkey_handler(
    _next_handler: *mut c_void,
    _event: *mut c_void,
    user_data: *mut c_void,
) -> i32 {
    if user_data.is_null() {
        return EVENT_NOT_HANDLED_ERR;
    }
    // SAFETY: `user_data` is a leaked `Box<HotkeyCallbackState>` kept alive until `RemoveEventHandler`.
    let state = unsafe { &*(user_data as *const HotkeyCallbackState) };
    if !state.enabled.load(Ordering::Relaxed) {
        return EVENT_NOT_HANDLED_ERR;
    }
    let position = super::current_cursor_position();
    let _ = state.tx.send(GlobalHotkeyEvent { position });
    0
}

/// Register the global `Ctrl + Option + Space` hotkey on the current macOS application target.
/// Must be called on the main UI thread (where AppKit/Carbon pumps events).
pub fn start_global_hotkey() -> Result<GlobalHotkeyHandle, HookError> {
    let (tx, rx) = unbounded();
    let enabled = Arc::new(AtomicBool::new(true));

    let callback_state = Box::new(HotkeyCallbackState {
        enabled: Arc::clone(&enabled),
        tx,
    });
    let user_data = Box::into_raw(callback_state) as *mut c_void;

    let spec = EventTypeSpec {
        event_class: EVENT_CLASS_KEYBOARD,
        event_kind: EVENT_HOTKEY_PRESSED,
    };
    let mut handler_ref: *mut c_void = std::ptr::null_mut();
    let mut hotkey_ref: *mut c_void = std::ptr::null_mut();

    // SAFETY: Valid Carbon HIToolbox registration calls on the application event target.
    unsafe {
        let target = GetApplicationEventTarget();
        if target.is_null() {
            drop(Box::from_raw(user_data as *mut HotkeyCallbackState));
            return Err(HookError::InstallFailed(
                "GetApplicationEventTarget returned null".into(),
            ));
        }

        let status = InstallEventHandler(
            target,
            hotkey_handler,
            1,
            &spec,
            user_data,
            &mut handler_ref,
        );
        if status != 0 {
            drop(Box::from_raw(user_data as *mut HotkeyCallbackState));
            return Err(HookError::InstallFailed(format!(
                "InstallEventHandler failed with OSStatus {status}"
            )));
        }

        let hot_key_id = EventHotKeyID {
            signature: HOTKEY_SIGNATURE,
            id: HOTKEY_ID,
        };
        let status = RegisterEventHotKey(
            KVK_SPACE,
            CONTROL_KEY | OPTION_KEY,
            hot_key_id,
            target,
            0,
            &mut hotkey_ref,
        );
        if status != 0 {
            let _ = RemoveEventHandler(handler_ref);
            drop(Box::from_raw(user_data as *mut HotkeyCallbackState));
            return Err(HookError::InstallFailed(format!(
                "RegisterEventHotKey(Ctrl+Option+Space) failed with OSStatus {status}"
            )));
        }
    }

    info!("macOS global hotkey registered (Ctrl+Option+Space)");

    let hotkey_addr = hotkey_ref as usize;
    let handler_addr = handler_ref as usize;
    let state_addr = user_data as usize;

    Ok(GlobalHotkeyHandle::new(rx, enabled, move || {
        // SAFETY: Pointers were produced by `RegisterEventHotKey`, `InstallEventHandler`, and
        // `Box::into_raw` above, and are released once in reverse registration order.
        unsafe {
            if hotkey_addr != 0 {
                let _ = UnregisterEventHotKey(hotkey_addr as *mut c_void);
            }
            if handler_addr != 0 {
                let _ = RemoveEventHandler(handler_addr as *mut c_void);
            }
            if state_addr != 0 {
                drop(Box::from_raw(state_addr as *mut HotkeyCallbackState));
            }
        }
        debug!("macOS global hotkey unregistered");
    }))
}
