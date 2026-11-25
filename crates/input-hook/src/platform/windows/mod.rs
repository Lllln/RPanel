pub mod keyboard;

use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Sender, unbounded};
use tracing::{debug, error, info, warn};
use windows::Win32::Foundation::{
    EXCEPTION_ACCESS_VIOLATION, EXCEPTION_ILLEGAL_INSTRUCTION, EXCEPTION_PRIV_INSTRUCTION,
    EXCEPTION_STACK_OVERFLOW, HINSTANCE, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, GetSystemMetrics, HC_ACTION, MSG,
    MSLLHOOKSTRUCT, PostThreadMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_REMOTESESSION,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, WH_MOUSE_LL, WM_MOUSEMOVE, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP,
};

use crate::state_machine::{LongPressStateMachine, StateAction};
use crate::worker::{PlatformActions, WorkerCommand, capture_or_none, spawn_worker};
use crate::{
    CaptureResult, HookConfig, HookControl, HookError, HookHandle, RPANEL_INJECTED_TAG,
    ScreenPoint, clipboard,
};

struct WinHookContext {
    control: Arc<HookControl>,
    state: Arc<Mutex<LongPressStateMachine>>,
    cmd_tx: Sender<WorkerCommand>,
}

thread_local! {
    static HOOK_CTX: RefCell<Option<WinHookContext>> = const { RefCell::new(None) };
}

struct WinActions;

impl PlatformActions for WinActions {
    fn replay_short_click(&self, down_at: ScreenPoint, up_at: ScreenPoint) {
        let inputs = [
            mouse_input(
                down_at,
                MOUSEEVENTF_MOVE
                    | MOUSEEVENTF_ABSOLUTE
                    | MOUSEEVENTF_VIRTUALDESK
                    | MOUSEEVENTF_RIGHTDOWN,
            ),
            mouse_input(
                up_at,
                MOUSEEVENTF_MOVE
                    | MOUSEEVENTF_ABSOLUTE
                    | MOUSEEVENTF_VIRTUALDESK
                    | MOUSEEVENTF_RIGHTUP,
            ),
        ];
        send_mouse_inputs(&inputs, "short click");
    }

    fn replay_drag_start(&self, down_at: ScreenPoint, current: ScreenPoint) {
        let inputs = [
            mouse_input(
                down_at,
                MOUSEEVENTF_MOVE
                    | MOUSEEVENTF_ABSOLUTE
                    | MOUSEEVENTF_VIRTUALDESK
                    | MOUSEEVENTF_RIGHTDOWN,
            ),
            mouse_input(
                current,
                MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
            ),
        ];
        send_mouse_inputs(&inputs, "drag start");
    }

    fn capture_selection(&self) -> CaptureResult {
        capture_or_none(capture_selection())
    }
}

/// Start the Windows `SetWindowsHookExW(WH_MOUSE_LL)` global right-click long-press interceptor.
pub fn start_hook(config: HookConfig) -> Result<HookHandle, HookError> {
    let (event_tx, event_rx) = unbounded();
    let control = Arc::new(HookControl::new(config));
    let state = Arc::new(Mutex::new(LongPressStateMachine::new(config)));

    let (cmd_tx, worker_handle) = spawn_worker(Arc::clone(&state), event_tx, WinActions);
    let (ready_tx, ready_rx) = unbounded::<Result<(), HookError>>();
    let hook_tid = Arc::new(AtomicU32::new(0));

    let thread_control = Arc::clone(&control);
    let thread_state = Arc::clone(&state);
    let thread_cmd_tx = cmd_tx.clone();
    let thread_tid = Arc::clone(&hook_tid);

    let hook_thread = thread::Builder::new()
        .name("rpanel-win-mouse-hook".into())
        .spawn(move || {
            run_win_hook_loop(
                thread_control,
                thread_state,
                thread_cmd_tx,
                thread_tid,
                ready_tx,
            );
        })
        .map_err(|e| HookError::InstallFailed(e.to_string()))?;

    match ready_rx.recv() {
        Ok(Ok(())) => {}
        Ok(Err(err)) => {
            let _ = cmd_tx.send(WorkerCommand::Shutdown);
            let _ = worker_handle.join();
            let _ = hook_thread.join();
            return Err(err);
        }
        Err(_) => {
            let _ = cmd_tx.send(WorkerCommand::Shutdown);
            let _ = worker_handle.join();
            let _ = hook_thread.join();
            return Err(HookError::InstallFailed(
                "Windows WH_MOUSE_LL hook thread exited before signaling readiness".into(),
            ));
        }
    }

    let stop_cmd_tx = cmd_tx;
    let stop_tid = hook_tid;
    Ok(HookHandle::new(event_rx, control, move || {
        let tid = stop_tid.swap(0, Ordering::AcqRel);
        if tid != 0 {
            // SAFETY: Posts WM_QUIT to the dedicated hook thread message queue.
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        let _ = stop_cmd_tx.send(WorkerCommand::Shutdown);
        let _ = hook_thread.join();
        let _ = worker_handle.join();
    }))
}

fn run_win_hook_loop(
    control: Arc<HookControl>,
    state: Arc<Mutex<LongPressStateMachine>>,
    cmd_tx: Sender<WorkerCommand>,
    hook_tid: Arc<AtomicU32>,
    ready_tx: Sender<Result<(), HookError>>,
) {
    // SAFETY: Win32 thread ID query and current module handle lookup.
    let tid = unsafe { GetCurrentThreadId() };
    hook_tid.store(tid, Ordering::Release);

    HOOK_CTX.with(|slot| {
        *slot.borrow_mut() = Some(WinHookContext {
            control,
            state,
            cmd_tx,
        });
    });

    let hmodule = unsafe { GetModuleHandleW(None) }
        .map(HINSTANCE::from)
        .unwrap_or_default();

    // SAFETY: Installs a global WH_MOUSE_LL hook pumped by this thread's message loop.
    let hook = match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(low_level_mouse_proc), Some(hmodule), 0) } {
        Ok(h) => h,
        Err(err) => {
            HOOK_CTX.with(|slot| {
                *slot.borrow_mut() = None;
            });
            hook_tid.store(0, Ordering::Release);
            let _ = ready_tx.send(Err(HookError::InstallFailed(format!(
                "SetWindowsHookExW(WH_MOUSE_LL) failed: {err}"
            ))));
            return;
        }
    };

    info!("Windows WH_MOUSE_LL hook active (tid={tid})");
    let _ = ready_tx.send(Ok(()));

    let mut msg = MSG::default();
    // SAFETY: Standard Win32 message loop for keeping WH_MOUSE_LL alive until WM_QUIT.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(hook);
    }

    HOOK_CTX.with(|slot| {
        *slot.borrow_mut() = None;
    });
    hook_tid.store(0, Ordering::Release);
    debug!("Windows WH_MOUSE_LL hook stopped");
}

unsafe extern "system" fn low_level_mouse_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code != HC_ACTION as i32 || lparam.0 == 0 {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    // SAFETY: When code == HC_ACTION, lparam points to a valid MSLLHOOKSTRUCT.
    let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };

    // Prevent hook recursion on synthetic events injected by RPanel.
    if info.dwExtraInfo == RPANEL_INJECTED_TAG as usize {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let msg_id = wparam.0 as u32;
    if !matches!(msg_id, WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MOUSEMOVE) {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let pos = ScreenPoint::new(info.pt.x, info.pt.y);
    let now = Instant::now();

    let should_swallow = HOOK_CTX.with(|slot| {
        let borrow = slot.borrow();
        let Some(ctx) = borrow.as_ref() else {
            return false;
        };

        let action = {
            let mut sm = ctx.state.lock().unwrap_or_else(|e| e.into_inner());
            if !ctx.control.is_enabled() && sm.is_idle() {
                return false;
            }
            sm.set_hold_duration(ctx.control.hold_duration());

            match msg_id {
                WM_RBUTTONDOWN => sm.on_right_down(pos, now),
                WM_MOUSEMOVE => sm.on_mouse_move(pos),
                WM_RBUTTONUP => sm.on_right_up(pos, now),
                _ => StateAction::PassThrough,
            }
        };

        dispatch_win_action(action, now, &ctx.cmd_tx)
    });

    if should_swallow {
        LRESULT(1)
    } else {
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }
}

fn dispatch_win_action(
    action: StateAction,
    now: Instant,
    cmd_tx: &Sender<WorkerCommand>,
) -> bool {
    match action {
        StateAction::PassThrough => false,
        StateAction::Swallow => true,
        StateAction::SwallowAndArmTimer {
            generation,
            hold_duration,
        } => {
            let _ = cmd_tx.send(WorkerCommand::ArmTimer {
                generation,
                deadline: now + hold_duration,
            });
            true
        }
        StateAction::ReplayShortClick { down_at, up_at } => {
            let _ = cmd_tx.send(WorkerCommand::ReplayShortClick { down_at, up_at });
            true
        }
        StateAction::ReplayDragStart { down_at, current } => {
            let _ = cmd_tx.send(WorkerCommand::ReplayDragStart { down_at, current });
            true
        }
        StateAction::TriggerLongPress { position } => {
            let _ = cmd_tx.send(WorkerCommand::TriggerLongPress { position });
            true
        }
    }
}

fn mouse_input(point: ScreenPoint, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    let (norm_x, norm_y) = normalize_virtual_desktop(point);
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: norm_x,
                dy: norm_y,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: RPANEL_INJECTED_TAG as usize,
            },
        },
    }
}

fn normalize_virtual_desktop(point: ScreenPoint) -> (i32, i32) {
    // SAFETY: GetSystemMetrics is thread-safe for virtual desktop bounds queries.
    let (vx, vy, vw, vh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
        )
    };

    let denom_x = i64::from((vw - 1).max(1));
    let denom_y = i64::from((vh - 1).max(1));

    let nx = ((i64::from(point.x - vx) * 65535 + denom_x / 2) / denom_x).clamp(0, 65535) as i32;
    let ny = ((i64::from(point.y - vy) * 65535 + denom_y / 2) / denom_y).clamp(0, 65535) as i32;
    (nx, ny)
}

fn send_mouse_inputs(inputs: &[INPUT], label: &str) {
    // SAFETY: `inputs` is a contiguous slice of valid Win32 `INPUT` structs.
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        warn!(
            sent,
            expected = inputs.len(),
            "SendInput incomplete for {label} (target window may be elevated)"
        );
    }
}

/// Capture currently selected text on Windows via clipboard backup + tagged `Ctrl+C` + restore.
pub fn capture_selection() -> Result<CaptureResult, HookError> {
    match clipboard::capture_via_clipboard_shortcut(keyboard::send_ctrl_c) {
        Ok(res) => Ok(res),
        Err(err) => {
            error!("Windows Ctrl+C clipboard capture failed: {err}");
            Err(err)
        }
    }
}

/// Convert a physical Win32 `MSLLHOOKSTRUCT` coordinate to logical DIP pixels
/// using the DPI of the monitor containing the cursor.
pub fn to_logical_point(point: ScreenPoint) -> ScreenPoint {
    // SAFETY: Win32 monitor lookup and DPI query for the given screen point.
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: point.x, y: point.y }, MONITOR_DEFAULTTONEAREST);
        let mut dpi_x: u32 = 96;
        let mut dpi_y: u32 = 96;
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_ok()
            && dpi_x >= 96
            && dpi_y >= 96
        {
            let lx = ((f64::from(point.x) * 96.0) / f64::from(dpi_x)).round() as i32;
            let ly = ((f64::from(point.y) * 96.0) / f64::from(dpi_y)).round() as i32;
            return ScreenPoint::new(lx, ly);
        }
    }
    point
}

/// Detect whether the process is running inside a Windows Remote Desktop / VDI session.
pub fn is_remote_desktop_session() -> bool {
    // SAFETY: GetSystemMetrics is thread-safe and side-effect free.
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

static VEH_LOGGED: AtomicBool = AtomicBool::new(false);

/// Install a Win32 Vectored Exception Handler that captures fatal native SEH exceptions
/// (e.g. `0xC0000005`, `0xC00000FD`, `0xC000001D`, `0xC000041D`) into `%TEMP%\rpanel-crash.log`.
pub fn install_native_crash_handler() {
    // SAFETY: Registers a process-wide vectored exception handler with a valid callback signature.
    unsafe {
        let _ = AddVectoredExceptionHandler(1, Some(vectored_exception_handler));
    }
}

unsafe extern "system" fn vectored_exception_handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    if info.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }

    // SAFETY: Windows passes a valid EXCEPTION_POINTERS pointer to the VEH callback.
    let record_ptr = unsafe { (*info).ExceptionRecord };
    if record_ptr.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let record = unsafe { &*record_ptr };
    let code = record.ExceptionCode;

    // Only log genuinely fatal native CPU/SEH exceptions; ignore benign WinRT/C++/RPC exceptions.
    let is_fatal = code == EXCEPTION_ACCESS_VIOLATION
        || code == EXCEPTION_STACK_OVERFLOW
        || code == EXCEPTION_ILLEGAL_INSTRUCTION
        || code == EXCEPTION_PRIV_INSTRUCTION
        || code.0 as u32 == 0xC000_0409
        || code.0 as u32 == 0xC000_041D;

    if is_fatal && !VEH_LOGGED.swap(true, Ordering::AcqRel) {
        let addr = record.ExceptionAddress as usize;
        let base = unsafe { GetModuleHandleW(None) }
            .map(|h| h.0 as usize)
            .unwrap_or(0);
        let offset = addr.saturating_sub(base);
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut report = format!(
            "=== RPanel Native SEH Exception (unix={ts}) ===\n\
             ExceptionCode: 0x{:08X}\n\
             ExceptionAddress: 0x{:016X}\n\
             ExeBase: 0x{:016X}\n\
             OffsetFromExeBase: 0x{:016X}\n",
            code.0 as u32, addr, base, offset
        );

        if code != EXCEPTION_STACK_OVERFLOW {
            let bt = Backtrace::force_capture();
            report.push_str(&format!("\nBacktrace:\n{bt}\n"));
        } else {
            report.push_str("\n(Stack overflow detected; skipped deep backtrace capture)\n");
        }
        report.push('\n');

        let log_path = std::env::temp_dir().join("rpanel-crash.log");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
            let _ = file.write_all(report.as_bytes());
        }
        eprintln!("{report}");
    }

    EXCEPTION_CONTINUE_SEARCH
}
