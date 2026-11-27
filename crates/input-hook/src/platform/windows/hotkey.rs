//! Native Windows global hotkey registration (`Ctrl + Alt + Space`) via Win32 `RegisterHotKey`.
//!
//! Runs a lightweight dedicated message-loop thread without installing any low-level keyboard hook.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use std::thread;

use crossbeam_channel::unbounded;
use tracing::{debug, info};
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetMessageW, MSG, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
};

use crate::{GlobalHotkeyEvent, GlobalHotkeyHandle, HookError};

const HOTKEY_ID: i32 = 1;

/// Register the global `Ctrl + Alt + Space` hotkey on a dedicated Win32 message thread.
pub fn start_global_hotkey() -> Result<GlobalHotkeyHandle, HookError> {
    let (tx, rx) = unbounded();
    let enabled = Arc::new(AtomicBool::new(true));
    let (ready_tx, ready_rx) = unbounded::<Result<(), HookError>>();
    let thread_id = Arc::new(AtomicU32::new(0));

    let loop_enabled = Arc::clone(&enabled);
    let loop_tid = Arc::clone(&thread_id);

    let handle = thread::Builder::new()
        .name("rpanel-win-hotkey".into())
        .spawn(move || {
            // SAFETY: Querying current Win32 thread ID and registering a thread-owned hotkey.
            let tid = unsafe { GetCurrentThreadId() };
            loop_tid.store(tid, Ordering::Release);

            let reg_result = unsafe {
                RegisterHotKey(
                    None,
                    HOTKEY_ID,
                    MOD_CONTROL | MOD_ALT | MOD_NOREPEAT,
                    u32::from(VK_SPACE.0),
                )
            };
            if let Err(err) = reg_result {
                loop_tid.store(0, Ordering::Release);
                let _ = ready_tx.send(Err(HookError::InstallFailed(format!(
                    "RegisterHotKey(Ctrl+Alt+Space) failed: {err}"
                ))));
                return;
            }

            info!("Windows global hotkey registered (Ctrl+Alt+Space, tid={tid})");
            let _ = ready_tx.send(Ok(()));

            let mut msg = MSG::default();
            // SAFETY: Standard Win32 message loop for thread-owned WM_HOTKEY notifications.
            unsafe {
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    if msg.message == WM_HOTKEY
                        && msg.wParam.0 == HOTKEY_ID as usize
                        && loop_enabled.load(Ordering::Relaxed)
                    {
                        let position = super::current_cursor_position();
                        let _ = tx.send(GlobalHotkeyEvent { position });
                    }
                }
                let _ = UnregisterHotKey(None, HOTKEY_ID);
            }

            loop_tid.store(0, Ordering::Release);
            debug!("Windows global hotkey thread stopped");
        })
        .map_err(|e| HookError::InstallFailed(e.to_string()))?;

    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            let _ = handle.join();
            return Err(err);
        }
        Err(_) => {
            let _ = handle.join();
            return Err(HookError::InstallFailed(
                "Windows hotkey thread exited before signaling readiness".into(),
            ));
        }
    }

    let stop_tid = thread_id;
    Ok(GlobalHotkeyHandle::new(rx, enabled, move || {
        let tid = stop_tid.swap(0, Ordering::AcqRel);
        if tid != 0 {
            // SAFETY: Posts WM_QUIT to the dedicated hotkey thread message queue.
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        let _ = handle.join();
    }))
}
