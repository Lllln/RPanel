pub mod hotkey;
pub mod keyboard;

use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Sender, unbounded};
use tracing::{debug, error, info, warn};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, EXCEPTION_ACCESS_VIOLATION, EXCEPTION_ILLEGAL_INSTRUCTION,
    EXCEPTION_PRIV_INSTRUCTION, EXCEPTION_STACK_OVERFLOW, GetLastError, HANDLE, HINSTANCE, HWND,
    LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMNCRENDERINGPOLICY, DWMNCRP_DISABLED, DWMWCP_DONOTROUND,
    DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
use windows::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetCurrentThreadId, OpenProcess, OpenProcessToken,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::{HSTRING, PWSTR};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GA_ROOT, GetAncestor, GetClassNameW, GetCursorPos,
    GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId,
    HC_ACTION, LLMHF_INJECTED, MSG, MSLLHOOKSTRUCT, PostThreadMessageW, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_REMOTESESSION, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SetWindowsHookExW,
    TranslateMessage, UnhookWindowsHookEx, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
    WM_MOUSEMOVE, WM_QUIT, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_XBUTTONDOWN, WindowFromPoint,
};

use crate::clipboard::{AutoCopyBaseline, CopyShortcutStep};
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

static PENDING_SYNTHETIC_RDOWN: AtomicUsize = AtomicUsize::new(0);
static PENDING_SYNTHETIC_RUP: AtomicUsize = AtomicUsize::new(0);
static SELF_ELEVATED: OnceLock<bool> = OnceLock::new();

struct WinActions;

impl PlatformActions for WinActions {
    fn replay_short_click(&self, _down_at: ScreenPoint, _up_at: ScreenPoint) {
        // Click directly at the current pointer location without `MOUSEEVENTF_MOVE` quantization
        // jitter, and leave a brief beat between down and up so Win32/Shell menus process cleanly.
        send_synthetic_right_button(MOUSEEVENTF_RIGHTDOWN, None, "short click down");
        thread::sleep(Duration::from_millis(10));
        send_synthetic_right_button(MOUSEEVENTF_RIGHTUP, None, "short click up");
    }

    fn replay_drag_start(&self, down_at: ScreenPoint, current: ScreenPoint) {
        send_synthetic_right_button(MOUSEEVENTF_RIGHTDOWN, Some(down_at), "drag start down");
        thread::sleep(Duration::from_millis(5));
        let move_only = [mouse_input(
            current,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        )];
        send_mouse_inputs(&move_only, "drag start move");
    }

    fn replay_drag_end(&self, _up_at: ScreenPoint) {
        send_synthetic_right_button(MOUSEEVENTF_RIGHTUP, None, "drag end up");
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

    let msg_id = wparam.0 as u32;

    // Prevent hook recursion on synthetic events injected by RPanel (check both `dwExtraInfo`
    // and our fallback pending-injected counters in case a driver/RDP filter strips extra info).
    if info.dwExtraInfo == RPANEL_INJECTED_TAG as usize {
        if msg_id == WM_RBUTTONDOWN {
            decrement_saturated(&PENDING_SYNTHETIC_RDOWN);
        } else if msg_id == WM_RBUTTONUP {
            decrement_saturated(&PENDING_SYNTHETIC_RUP);
        }
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    if (info.flags & LLMHF_INJECTED) != 0 {
        if msg_id == WM_RBUTTONDOWN && try_consume_pending(&PENDING_SYNTHETIC_RDOWN) {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
        if msg_id == WM_RBUTTONUP && try_consume_pending(&PENDING_SYNTHETIC_RUP) {
            return unsafe { CallNextHookEx(None, code, wparam, lparam) };
        }
    }

    let is_mouse_down = matches!(
        msg_id,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    );
    if !is_mouse_down && !matches!(msg_id, WM_RBUTTONUP | WM_MOUSEMOVE) {
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }

    let pos = ScreenPoint::new(info.pt.x, info.pt.y);
    let now = Instant::now();

    let should_swallow = HOOK_CTX.with(|slot| {
        let borrow = slot.borrow();
        let Some(ctx) = borrow.as_ref() else {
            return false;
        };

        if msg_id == WM_LBUTTONDOWN {
            let _ = ctx.cmd_tx.send(WorkerCommand::LeftMouseDown { at: now });
        }

        if is_mouse_down && ctx.control.is_dismiss_watch_active() {
            let _ = ctx.cmd_tx.send(WorkerCommand::GlobalMouseDown {
                position: pos,
                at: now,
            });
        }

        if matches!(msg_id, WM_LBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN) {
            return false;
        }

        let action = {
            let mut sm = ctx.state.lock().unwrap_or_else(|e| e.into_inner());
            if !ctx.control.is_enabled() && sm.is_idle() {
                return false;
            }
            if msg_id == WM_RBUTTONDOWN && sm.is_idle() && should_bypass_right_click(pos) {
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
        StateAction::ReplayDragEnd { up_at } => {
            let _ = cmd_tx.send(WorkerCommand::ReplayDragEnd { up_at });
            true
        }
        StateAction::TriggerLongPress { position } => {
            let _ = cmd_tx.send(WorkerCommand::TriggerLongPress { position });
            true
        }
    }
}

fn mouse_button_only_input(flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: RPANEL_INJECTED_TAG as usize,
            },
        },
    }
}

fn send_synthetic_right_button(
    button_flag: MOUSE_EVENT_FLAGS,
    move_to: Option<ScreenPoint>,
    label: &str,
) {
    let counter = if button_flag == MOUSEEVENTF_RIGHTDOWN {
        &PENDING_SYNTHETIC_RDOWN
    } else {
        &PENDING_SYNTHETIC_RUP
    };
    counter.fetch_add(1, Ordering::AcqRel);

    let input = match move_to {
        Some(point) => mouse_input(
            point,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | button_flag,
        ),
        None => mouse_button_only_input(button_flag),
    };

    let sent = unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    if sent != 1 {
        decrement_saturated(counter);
        warn!(
            sent,
            expected = 1,
            "SendInput incomplete for {label} (target window may be elevated)"
        );
    }
}

fn decrement_saturated(counter: &AtomicUsize) {
    let _ = try_consume_pending(counter);
}

fn try_consume_pending(counter: &AtomicUsize) -> bool {
    let mut current = counter.load(Ordering::Acquire);
    while current > 0 {
        match counter.compare_exchange_weak(
            current,
            current - 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(next) => current = next,
        }
    }
    false
}

fn is_self_elevated() -> bool {
    *SELF_ELEVATED.get_or_init(|| unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut ret_len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut std::ffi::c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    })
}

fn is_process_elevated_or_protected(pid: u32) -> bool {
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            // Access denied from a standard user process usually indicates SYSTEM / PPL / elevated.
            return true;
        };
        let mut token = HANDLE::default();
        let token_res = OpenProcessToken(process, TOKEN_QUERY, &mut token);
        let _ = CloseHandle(process);
        if token_res.is_err() {
            return true;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut ret_len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut std::ffi::c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// Bypass global right-click interception when clicking on Windows taskbar/system tray icons,
/// tray overflow popups, active context menus, or elevated windows (which block `SendInput`
/// via UIPI when RPanel is running non-elevated).
fn should_bypass_right_click(pos: ScreenPoint) -> bool {
    unsafe {
        let hwnd = WindowFromPoint(POINT { x: pos.x, y: pos.y });
        if hwnd.0.is_null() {
            return false;
        }

        let mut class_buf = [0u16; 128];
        let len = GetClassNameW(hwnd, &mut class_buf);
        if len > 0 {
            let class_name = String::from_utf16_lossy(&class_buf[..len as usize]);
            if crate::platform::is_bypass_right_click_window_class(&class_name) {
                return true;
            }
        }

        let root = GetAncestor(hwnd, GA_ROOT);
        let inspect_hwnd = if !root.0.is_null() { root } else { hwnd };
        if inspect_hwnd != hwnd {
            let root_len = GetClassNameW(inspect_hwnd, &mut class_buf);
            if root_len > 0 {
                let root_class = String::from_utf16_lossy(&class_buf[..root_len as usize]);
                if crate::platform::is_bypass_right_click_window_class(&root_class) {
                    return true;
                }
            }
        }

        if !is_self_elevated() {
            let mut pid = 0u32;
            GetWindowThreadProcessId(inspect_hwnd, Some(&mut pid));
            if pid != 0 && pid != std::process::id() && is_process_elevated_or_protected(pid) {
                return true;
            }
        }

        false
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

/// Query the Win32 lock-free clipboard change counter (`GetClipboardSequenceNumber`).
pub fn clipboard_sequence_number() -> Option<u64> {
    // SAFETY: `GetClipboardSequenceNumber` is thread-safe and does not lock the clipboard.
    Some(u64::from(unsafe { GetClipboardSequenceNumber() }))
}

/// Inspect the foreground Win32 window's class name and executable image name to determine
/// whether it is a terminal emulator / SSH shell (e.g. WindTerm, Windows Terminal, PuTTY,
/// Xshell, MobaXterm, PowerShell, cmd, mintty, Tabby, etc.).
fn is_foreground_terminal() -> bool {
    // SAFETY: Win32 foreground window, class name, and limited process-image queries.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return false;
        }

        let mut class_buf = [0u16; 128];
        let class_len = GetClassNameW(hwnd, &mut class_buf);
        if class_len > 0 {
            let class_name = String::from_utf16_lossy(&class_buf[..class_len as usize]);
            if crate::platform::is_known_terminal_window_class(&class_name) {
                debug!(%class_name, "detected foreground Windows terminal by window class");
                return true;
            }
        }

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return false;
        }

        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };

        let mut path_buf = [0u16; 512];
        let mut path_len = path_buf.len() as u32;
        let query_ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR::from_raw(path_buf.as_mut_ptr()),
            &mut path_len,
        )
        .is_ok();
        let _ = CloseHandle(process);

        if query_ok && path_len > 0 {
            let image_path = String::from_utf16_lossy(&path_buf[..path_len as usize]);
            let is_term = crate::platform::is_known_terminal_identifier(&image_path);
            debug!(%image_path, is_term, "inspected foreground Windows process");
            return is_term;
        }

        false
    }
}

/// Capture currently selected text on Windows using lock-free clipboard sequence polling:
/// - In terminal/shell windows (WindTerm, Windows Terminal, PuTTY, Xshell, MobaXterm, cmd,
///   PowerShell, mintty, etc.), sends `Ctrl+Shift+C` then `Ctrl+Insert` (NEVER `Ctrl+C`,
///   avoiding accidental `SIGINT`), and falls back to recent auto-copy-on-select.
/// - In standard apps, sends `Ctrl+C` (with hardware scan codes and modifier hygiene), then
///   safe `Ctrl+Insert` fallback (for IDE integrated terminals & legacy controls), then recent
///   auto-copy-on-select.
pub fn capture_selection_with_baseline(
    auto_copy_baseline: Option<AutoCopyBaseline>,
) -> Result<CaptureResult, HookError> {
    let is_terminal = is_foreground_terminal();

    let result = if is_terminal {
        let steps = [
            CopyShortcutStep {
                label: "ctrl+shift+c",
                timeout: Duration::from_millis(220),
                trigger: &keyboard::send_ctrl_shift_c,
            },
            CopyShortcutStep {
                label: "ctrl+insert",
                timeout: Duration::from_millis(180),
                trigger: &keyboard::send_ctrl_insert,
            },
        ];
        clipboard::capture_via_shortcuts(&steps, auto_copy_baseline)
    } else {
        let steps = [
            CopyShortcutStep {
                label: "ctrl+c",
                timeout: Duration::from_millis(260),
                trigger: &keyboard::send_ctrl_c,
            },
            CopyShortcutStep {
                label: "ctrl+insert",
                timeout: Duration::from_millis(160),
                trigger: &keyboard::send_ctrl_insert,
            },
        ];
        clipboard::capture_via_shortcuts(&steps, auto_copy_baseline)
    };

    match result {
        Ok(res) => Ok(res),
        Err(err) => {
            error!("Windows clipboard selection capture failed: {err}");
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

/// RAII owner of the Win32 session-scoped named mutex used for single-instance enforcement.
pub struct WinSingleInstanceGuard {
    handle: HANDLE,
}

// SAFETY: A Win32 mutex HANDLE is an opaque kernel handle safe to transfer across threads
// and close on process exit; ownership is never shared concurrently.
unsafe impl Send for WinSingleInstanceGuard {}

impl Drop for WinSingleInstanceGuard {
    fn drop(&mut self) {
        if !self.handle.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}

/// Acquire a per-session Win32 named mutex (`Local\<app_id>`). Returns `None` if another
/// instance in the current Windows session already owns the mutex.
pub fn acquire_single_instance(app_id: &str) -> Option<WinSingleInstanceGuard> {
    let mutex_name = HSTRING::from(format!("Local\\{app_id}"));
    // SAFETY: `CreateMutexW` returns a valid kernel HANDLE or Err; `GetLastError` immediately
    // reports `ERROR_ALREADY_EXISTS` when opening an existing mutex owned by another instance.
    let handle = unsafe { CreateMutexW(None, false, &mutex_name) }.ok()?;
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_exists {
        unsafe {
            let _ = CloseHandle(handle);
        }
        return None;
    }
    Some(WinSingleInstanceGuard { handle })
}

/// Detect whether the process is running inside a Windows Remote Desktop / VDI session.
pub fn is_remote_desktop_session() -> bool {
    // SAFETY: GetSystemMetrics is thread-safe and side-effect free.
    unsafe { GetSystemMetrics(SM_REMOTESESSION) != 0 }
}

/// Disable DWM non-client frame/titlebar rendering (`DWMNCRP_DISABLED`) and native corner
/// rounding (`DWMWCP_DONOTROUND`) on a transparent DirectComposition `HWND`, so Windows 10
/// never shows its ~30px top caption strip behind transparent top corners and Windows 11 DWM
/// does not clip or clash with GPUI's custom rounded frame.
pub fn disable_dwm_window_frame(hwnd_raw: isize) {
    if hwnd_raw == 0 {
        return;
    }

    let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
    let nc_policy = DWMNCRP_DISABLED;
    let corner_pref = DWMWCP_DONOTROUND;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_NCRENDERING_POLICY,
            &nc_policy as *const _ as *const std::ffi::c_void,
            size_of::<DWMNCRENDERINGPOLICY>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner_pref as *const _ as *const std::ffi::c_void,
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
    }
}

/// Check whether a raw Win32 physical screen point lies inside the `HWND`'s window rectangle.
pub fn window_contains_screen_point(hwnd_raw: isize, point: ScreenPoint) -> Option<bool> {
    if hwnd_raw == 0 {
        return None;
    }
    let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
    let mut rect = RECT::default();
    // SAFETY: `hwnd` is a live Win32 window handle queried on the UI thread.
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(
        point.x >= rect.left
            && point.x < rect.right
            && point.y >= rect.top
            && point.y < rect.bottom,
    )
}

/// Query the current global cursor location in Win32 physical screen coordinates.
pub fn current_cursor_position() -> Option<ScreenPoint> {
    let mut pt = POINT::default();
    // SAFETY: `GetCursorPos` writes the current physical screen cursor coordinates into `pt`.
    unsafe { GetCursorPos(&mut pt) }.ok()?;
    Some(ScreenPoint::new(pt.x, pt.y))
}

/// Windows does not have a zero-keystroke cross-app selection API safe during hotkey chord hold.
pub fn try_capture_selection_non_intrusive() -> Option<String> {
    None
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
