use std::{ffi::c_void, mem::size_of};

use raw_window_handle::RawWindowHandle;
use rpanel::{gesture::Point, placement::axis_position};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, E_FAIL, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT,
            SetLastError, WIN32_ERROR, WPARAM,
        },
        Graphics::{
            Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute},
            Gdi::{
                CreateRoundRectRgn, DeleteObject, GetMonitorInfoW, HRGN, MONITOR_DEFAULTTONEAREST,
                MONITORINFO, MonitorFromPoint, SetWindowRgn,
            },
        },
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::Threading::{
            GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
        },
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
                SetProcessDpiAwarenessContext,
            },
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
    core::{BOOL, Error, Result},
};

use super::Trigger;
use crate::platform::PanelBounds;

const SUBCLASS_ID: usize = 1;

pub fn set_dpi_awareness() {
    // SAFETY: 在创建 GUI 窗口前调用，只设置当前进程的 DPI 模式。
    if let Err(error) =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
    {
        log::debug!("未能显式设置 Per-Monitor V2 DPI 模式：{error}");
    }
}

pub struct Popup {
    hwnd: HWND,
}

impl Popup {
    pub fn attach(handle: RawWindowHandle) -> Result<Self> {
        let RawWindowHandle::Win32(handle) = handle else {
            return Err(Error::new(E_FAIL, "eframe 未提供 Win32 窗口句柄"));
        };
        let hwnd = HWND(handle.hwnd.get() as *mut c_void);
        // SAFETY: eframe 在 GUI 线程交付尚未显示的本进程窗口。子类过程不持有 Rust 引用。
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let updated = (style | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW).0 as isize)
                & !(WS_EX_APPWINDOW.0 as isize);
            SetLastError(WIN32_ERROR(0));
            if SetWindowLongPtrW(hwnd, GWL_EXSTYLE, updated) == 0 && GetLastError().0 != 0 {
                return Err(Error::from_thread());
            }
            if !SetWindowSubclass(hwnd, Some(popup_proc), SUBCLASS_ID, 0).as_bool() {
                return Err(Error::new(E_FAIL, "无法配置悬浮面板的窗口子类"));
            }
        }
        // eframe 会在首帧后自动显示根窗口。DWM 遮蔽允许渲染，同时防止驻留窗口显示。
        set_cloaked(hwnd, true)?;
        Ok(Self { hwnd })
    }

    pub fn preview() -> Option<Trigger> {
        let mut point = POINT::default();
        // SAFETY: point 是有效的输出缓冲区，坐标仅用于定位主动请求的预览。
        if let Err(error) = unsafe { GetCursorPos(&mut point) } {
            log::warn!("读取预览位置失败：{error}");
            return None;
        }
        Some(Trigger {
            point: Point {
                x: point.x,
                y: point.y,
            },
            foreground: None,
        })
    }

    pub fn show(
        &self,
        trigger: Trigger,
        size: [u32; 2],
        generation: u64,
    ) -> Result<Option<PanelBounds>> {
        let point = trigger.point;
        let Some(monitor) = monitor_at(point) else {
            return Ok(None);
        };
        if let Some(foreground) = trigger.foreground
            && !can_show(foreground, point, monitor.rcMonitor)
        {
            return Ok(None);
        }

        let work = monitor.rcWork;
        // SAFETY: 只在 GUI 线程操作自己的窗口。先隐藏移动到目标屏，取得该屏 DPI。
        unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                work.left,
                work.top,
                0,
                0,
                SWP_NOZORDER | SWP_NOSIZE | SWP_NOACTIVATE | SWP_HIDEWINDOW,
            )?;
            let dpi = GetDpiForWindow(self.hwnd).max(96);
            let scale = |points: u32| (points * dpi).div_ceil(96) as i32;
            let width = scale(size[0]).min(work.right - work.left);
            let height = scale(size[1]).min(work.bottom - work.top);
            if width <= 0 || height <= 0 {
                return Ok(None);
            }
            let x = axis_position(point.x, work.left, work.right, width, scale(10));
            let y = axis_position(point.y, work.top, work.bottom, height, scale(10));

            let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, scale(24), scale(24));
            set_region(self.hwnd, region)?;
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_FRAMECHANGED,
            )?;
        }

        set_cloaked(self.hwnd, false)?;
        self.bounds(generation).map(Some)
    }

    fn bounds(&self, generation: u64) -> Result<PanelBounds> {
        let mut rect = RECT::default();
        // SAFETY: rect 为有效输出缓冲区，窗口由调用线程持有。
        unsafe { GetWindowRect(self.hwnd, &mut rect)? };
        Ok(PanelBounds {
            generation,
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    }

    pub fn hide(&self) {
        // SAFETY: 在 GUI 线程隐藏自己的窗口。ShowWindow 的返回值表示此前可见性。
        let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        if let Err(error) = set_cloaked(self.hwnd, true) {
            log::warn!("无法屏蔽已隐藏的悬浮面板：{error}");
        }
    }
}

fn set_cloaked(hwnd: HWND, cloaked: bool) -> Result<()> {
    let value = BOOL::from(cloaked);
    // SAFETY: hwnd 属于当前 GUI 线程。DWMWA_CLOAK 接收 BOOL，引用在调用期间有效。
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            std::ptr::from_ref(&value).cast(),
            size_of::<BOOL>() as u32,
        )
    }
}

fn set_region(hwnd: HWND, region: HRGN) -> Result<()> {
    if region.is_invalid() {
        return Err(Error::new(E_FAIL, "无法创建悬浮面板窗口区域"));
    }
    // SAFETY: hwnd 是 GUI 线程自己的窗口。系统只在成功后接管 region。
    unsafe {
        if SetWindowRgn(hwnd, Some(region), false) == 0 {
            let _ = DeleteObject(region.into());
            return Err(Error::new(E_FAIL, "无法设置悬浮面板窗口区域"));
        }
    }
    Ok(())
}

fn monitor_at(point: Point) -> Option<MONITORINFO> {
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: MONITORINFO 的大小正确，缓冲区由栈借用至调用结束。
    let ok = unsafe {
        let handle = MonitorFromPoint(
            POINT {
                x: point.x,
                y: point.y,
            },
            MONITOR_DEFAULTTONEAREST,
        );
        GetMonitorInfoW(handle, &mut monitor).as_bool()
    };
    if !ok {
        log::warn!("读取光标所在显示器的工作区失败");
    }
    ok.then_some(monitor)
}

fn can_show(source: usize, point: Point, monitor: RECT) -> bool {
    // SAFETY: 这里只读取当前桌面上窗口、进程和线程状态。每个查询失败时取消弹出。
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() || foreground.0 as usize != source {
            log::debug!("跳过弹出：前台窗口已切换");
            return false;
        }
        let mut process_id = 0;
        let thread_id = GetWindowThreadProcessId(foreground, Some(&mut process_id));
        if thread_id == 0 || process_id == GetCurrentProcessId() {
            return false;
        }
        let mut gui = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(thread_id, &mut gui).is_err()
            || gui.flags.0 & (GUI_INMOVESIZE | GUI_SYSTEMMENUMODE).0 != 0
        {
            log::debug!("跳过弹出：前台线程不可用、正在移动窗口或操作系统菜单");
            return false;
        }
        let root = GetAncestor(foreground, GA_ROOTOWNER);
        let hit = WindowFromPoint(POINT {
            x: point.x,
            y: point.y,
        });
        // 原应用可能在我们处理松开事件前弹出菜单，命中同线程的原生菜单仍是同一次手势。
        let hits_current_menu = gui.flags.0 & (GUI_INMENUMODE | GUI_POPUPMENUMODE).0 != 0
            && is_native_menu(hit, thread_id);
        if hit.is_invalid() || (GetAncestor(hit, GA_ROOTOWNER) != root && !hits_current_menu) {
            log::debug!("跳过弹出：光标下的窗口不属于本次手势来源");
            return false;
        }
        let mut rect = RECT::default();
        if GetWindowRect(foreground, &mut rect).is_err()
            || (!IsZoomed(foreground).as_bool()
                && rect.left <= monitor.left
                && rect.top <= monitor.top
                && rect.right >= monitor.right
                && rect.bottom >= monitor.bottom)
        {
            log::debug!("跳过弹出：前台窗口不可用或处于全屏");
            return false;
        }
        let allowed = matches!(is_elevated(process_id), Ok(false));
        if !allowed {
            log::debug!("跳过弹出：前台进程已提权或权限无法读取");
        }
        allowed
    }
}

fn is_native_menu(hwnd: HWND, owner_thread: u32) -> bool {
    let mut class = [0; 16];
    // SAFETY: 查询当前光标命中的窗口，类名输出缓冲区在调用期间有效。
    unsafe {
        GetWindowThreadProcessId(hwnd, None) == owner_thread
            && GetClassNameW(hwnd, &mut class) == 6
            && class[..6] == *windows::core::w!("#32768").as_wide()
    }
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: 只持有成功打开的独占句柄，并在此处释放一次。
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn is_elevated(process_id: u32) -> Result<bool> {
    // SAFETY: 仅取得查询权限，输出缓冲区与 TokenElevation 的类型和长度一致。
    unsafe {
        let process = OwnedHandle(OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            process_id,
        )?);
        let mut token = HANDLE::default();
        OpenProcessToken(process.0, TOKEN_QUERY, &mut token)?;
        let token = OwnedHandle(token);
        let mut info = TOKEN_ELEVATION::default();
        let mut written = 0;
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some(std::ptr::from_mut(&mut info).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut written,
        )?;
        Ok(info.TokenIsElevated != 0)
    }
}

unsafe extern "system" fn popup_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    if message == WM_MOUSEACTIVATE {
        // 保留点击消息，同时保持原窗口的前台状态。
        return LRESULT(MA_NOACTIVATE as isize);
    }
    // SAFETY: HWND 和消息均来自系统的子类过程调用。销毁时摘除本过程。
    unsafe {
        if message == WM_NCDESTROY {
            let _ = RemoveWindowSubclass(hwnd, Some(popup_proc), id);
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}
