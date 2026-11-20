use std::{
    cell::RefCell,
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
};

use rpanel::gesture::{Point, RightHold};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
    },
    core::Result as WinResult,
};

const WM_MOUSE_EVENTS: u32 = WM_APP + 1;
const WM_LISTENING: u32 = WM_APP + 2;

use crate::platform::{MouseEvent, PanelBounds};

#[derive(Clone, Copy)]
pub struct Trigger {
    pub point: Point,
    // 跨线程仅携带标识，使用前检查前台窗口。
    pub(super) foreground: usize,
}

struct HookState {
    hold: RightHold,
    foreground: HWND,
    bounds: Arc<Mutex<Option<PanelBounds>>>,
    show: Option<Trigger>,
    dismiss: Option<u64>,
    posted: bool,
}

thread_local! {
    static HOOK_STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

impl HookState {
    // 此方法在 Hook 中运行。只更新当前线程的状态，不阻塞等待 GUI。
    fn observe(&mut self, message: u32, event: MSLLHOOKSTRUCT) {
        if event.flags & LLMHF_INJECTED != 0 {
            return;
        }
        let point = Point {
            x: event.pt.x,
            y: event.pt.y,
        };
        if matches!(
            message,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        ) && let Ok(bounds) = self.bounds.try_lock()
            && let Some(bounds) = *bounds
            && !bounds.contains(point)
        {
            self.dismiss = Some(bounds.generation);
        }

        match message {
            WM_RBUTTONDOWN => {
                // SAFETY: 这两个查询只读取窗口状态。空 HWND 由下方分支处理。
                self.foreground = unsafe { GetForegroundWindow() };
                if self.foreground.is_invalid() {
                    self.hold.cancel();
                } else {
                    let dpi = unsafe { GetDpiForWindow(self.foreground) }.max(96);
                    self.hold.press(event.time, point, (8 * dpi).div_ceil(96));
                }
            }
            WM_MOUSEMOVE => self.hold.move_to(point),
            WM_RBUTTONUP if self.hold.release(event.time, point) => {
                // SAFETY: 前台窗口标识在使用前再次校验，不跨线程解引用。
                if unsafe { GetForegroundWindow() } == self.foreground {
                    self.show = Some(Trigger {
                        point,
                        foreground: self.foreground.0 as usize,
                    });
                }
            }
            WM_LBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                self.hold.cancel();
            }
            _ => {}
        }

        if !self.posted && (self.show.is_some() || self.dismiss.is_some()) {
            // SAFETY: 监听线程已建立消息队列，消息不携带指针或借用。
            self.posted = unsafe {
                PostThreadMessageW(GetCurrentThreadId(), WM_MOUSE_EVENTS, WPARAM(0), LPARAM(0))
                    .is_ok()
            };
            if !self.posted {
                self.show = None;
                self.dismiss = None;
            }
        }
    }
}

unsafe extern "system" fn mouse_callback(code: i32, message: WPARAM, data: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && data.0 != 0 {
        // SAFETY: Windows 在 HC_ACTION 回调期间提供对齐且有效的 MSLLHOOKSTRUCT。
        let event = unsafe { *(data.0 as *const MSLLHOOKSTRUCT) };
        let _ = HOOK_STATE.try_with(|state| {
            if let Ok(mut state) = state.try_borrow_mut()
                && let Some(state) = state.as_mut()
            {
                state.observe(message.0 as u32, event);
            }
        });
    }

    // SAFETY: 使用系统传入的三个参数继续 Hook 链，所有输入均放行。
    unsafe { CallNextHookEx(None, code, message, data) }
}

struct Hook(HHOOK);

impl Hook {
    fn install() -> WinResult<Self> {
        // SAFETY: 在消息循环所属线程注册进程内静态回调，0 表示当前桌面的全局 Hook。
        unsafe {
            let module = GetModuleHandleW(None)?;
            SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_callback), Some(module.into()), 0).map(Self)
        }
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        // SAFETY: 句柄由此线程创建，且只由此实例释放一次。
        if let Err(error) = unsafe { UnhookWindowsHookEx(self.0) } {
            log::warn!("卸载鼠标 Hook 失败：{error}");
        }
    }
}

pub struct MouseListener {
    thread_id: u32,
    worker: Option<JoinHandle<()>>,
    bounds: Arc<Mutex<Option<PanelBounds>>>,
}

impl MouseListener {
    pub fn start(
        threshold_ms: u32,
        emit: impl Fn(MouseEvent) + Send + 'static,
    ) -> std::io::Result<Self> {
        let bounds = Arc::new(Mutex::new(None));
        let worker_bounds = Arc::clone(&bounds);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("rpanel-mouse".into())
            .spawn(move || {
                listen(threshold_ms, worker_bounds, ready_tx, emit);
            })?;
        let thread_id = ready_rx.recv().map_err(std::io::Error::other)?;
        Ok(Self {
            thread_id,
            worker: Some(worker),
            bounds,
        })
    }

    pub fn set_enabled(&self, enabled: bool) -> WinResult<()> {
        self.post(WM_LISTENING, WPARAM(usize::from(enabled)))
    }

    pub fn set_panel_bounds(&self, bounds: Option<PanelBounds>) {
        if let Ok(mut current) = self.bounds.lock() {
            *current = bounds;
        }
    }

    fn post(&self, message: u32, parameter: WPARAM) -> WinResult<()> {
        // SAFETY: 启动握手确认线程消息队列已建立，消息仅传递整数。
        unsafe { PostThreadMessageW(self.thread_id, message, parameter, LPARAM(0)) }
    }
}

impl Drop for MouseListener {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        if let Err(error) = self.post(WM_QUIT, WPARAM(0))
            && !worker.is_finished()
        {
            log::error!("无法发送监听退出消息：{error}");
            return;
        }
        if worker.join().is_err() {
            log::error!("鼠标监听线程意外退出");
        }
    }
}

fn listen(
    threshold_ms: u32,
    bounds: Arc<Mutex<Option<PanelBounds>>>,
    ready: mpsc::SyncSender<u32>,
    emit: impl Fn(MouseEvent),
) {
    let mut message = MSG::default();
    // SAFETY: message 是当前线程独占的输出缓冲区，PeekMessage 初始化系统消息队列。
    let thread_id = unsafe {
        let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        GetCurrentThreadId()
    };
    HOOK_STATE.with(|state| {
        state.replace(Some(HookState {
            hold: RightHold::new(threshold_ms),
            foreground: HWND::default(),
            bounds,
            show: None,
            dismiss: None,
            posted: false,
        }));
    });
    let mut hook = install_hook();
    if ready.send(thread_id).is_err() {
        return;
    }
    emit(MouseEvent::Listening(hook.is_some()));

    loop {
        // SAFETY: 此线程拥有 message 和队列，未限定窗口，以便接收线程消息和 WM_QUIT。
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if result <= 0 {
            if result < 0 {
                log::error!("鼠标消息循环失败：{}", windows::core::Error::from_thread());
                emit(MouseEvent::Listening(false));
            }
            break;
        }
        match message.message {
            WM_LISTENING => {
                if message.wParam.0 != 0 {
                    if hook.is_none() {
                        hook = install_hook();
                    }
                } else {
                    drop(hook.take());
                    drain_state(true);
                }
                log::info!(
                    "右键监听{}",
                    if hook.is_some() {
                        "已启用"
                    } else {
                        "已停用"
                    }
                );
                emit(MouseEvent::Listening(hook.is_some()));
            }
            WM_MOUSE_EVENTS => {
                let (dismiss, show) = drain_state(false);
                if let Some(generation) = dismiss {
                    emit(MouseEvent::Dismiss(generation));
                }
                if let Some(trigger) = show {
                    emit(MouseEvent::Show(trigger));
                }
            }
            _ => {
                // SAFETY: 其他 Win32 消息由 GetMessage 产生，交由系统分发。
                unsafe { DispatchMessageW(&message) };
            }
        }
    }
    drop(hook);
    HOOK_STATE.with(|state| state.replace(None));
}

fn drain_state(cancel: bool) -> (Option<u64>, Option<Trigger>) {
    HOOK_STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(state) = state.as_mut() else {
            return (None, None);
        };
        if cancel {
            state.hold.cancel();
        }
        state.posted = false;
        (state.dismiss.take(), state.show.take())
    })
}

fn install_hook() -> Option<Hook> {
    Hook::install()
        .map_err(|error| log::error!("安装右键鼠标 Hook 失败：{error}"))
        .ok()
}
