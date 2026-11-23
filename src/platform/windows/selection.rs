use std::{
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::Instant,
};

use windows::{
    Win32::{
        Foundation::{E_NOINTERFACE, E_POINTER, ERROR_TIMEOUT, HWND},
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
        UI::{
            Accessibility::*,
            WindowsAndMessaging::{GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, IsChild},
        },
    },
    core::{Error, Interface},
};

use super::mouse::WindowSource;
use crate::platform::{SelectionResult, selection::TIMEOUT_MESSAGE};

const MAX_TEXT_UNITS: usize = 4 * 1024 * 1024;
const MAX_ANCESTORS: usize = 16;
const MAX_RANGES: i32 = 32;

#[derive(Clone, Copy)]
pub struct SelectionTarget {
    source: WindowSource,
    focus: WindowSource,
}

impl SelectionTarget {
    pub(in crate::platform) fn capture(source: WindowSource) -> Option<Self> {
        let root = source.window();
        let mut gui = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: 只查询前台窗口和来源线程，gui 在调用期间是有效输出缓冲区。
        unsafe {
            if GetForegroundWindow() != root
                || WindowSource::from_window(root) != Some(source)
                || GetGUIThreadInfo(source.thread_id, &mut gui).is_err()
                || !within(root, gui.hwndFocus)
            {
                return None;
            }
        }
        let focus = WindowSource::from_window(gui.hwndFocus)?;
        Some(Self { source, focus })
    }

    pub fn is_current(self) -> bool {
        Self::capture(self.source).is_some_and(|target| target.focus == self.focus)
    }

    fn owns(self, hwnd: HWND) -> bool {
        let focus = self.focus.window();
        within(self.source.window(), hwnd) && (within(focus, hwnd) || within(hwnd, focus))
    }
}

fn within(parent: HWND, child: HWND) -> bool {
    // SAFETY: IsChild 只检查窗口层级，不解引用 HWND 中的地址。
    !child.is_invalid() && (parent == child || unsafe { IsChild(parent, child).as_bool() })
}

struct Request {
    target: SelectionTarget,
    deadline: Instant,
    reply: SyncSender<SelectionResult>,
}

pub struct SelectionWorker {
    requests: SyncSender<Request>,
    busy: Arc<AtomicBool>,
}

impl SelectionWorker {
    pub fn start(wake: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let (requests, incoming) = mpsc::sync_channel::<Request>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let worker_busy = Arc::clone(&busy);
        // 退出时丢弃发送端。若 provider 尚未返回，GUI 线程无需等待它。
        thread::Builder::new()
            .name("rpanel-selection".into())
            .spawn(move || read_requests(incoming, worker_busy, wake))?;
        Ok(Self { requests, busy })
    }

    pub fn request(
        &self,
        target: SelectionTarget,
        deadline: Instant,
    ) -> Result<Receiver<SelectionResult>, &'static str> {
        const BUSY: &str = "上一次选区读取仍在进行，请手动粘贴。";
        if self.busy.swap(true, Ordering::AcqRel) {
            log::debug!("跳过选区读取：已有 UI Automation 请求在处理");
            return Err(BUSY);
        }
        let (reply, result) = mpsc::sync_channel(1);
        if let Err(error) = self.requests.try_send(Request {
            target,
            deadline,
            reply,
        }) {
            self.busy.store(false, Ordering::Release);
            log::warn!("无法投递选区读取请求");
            return Err(match error {
                TrySendError::Full(_) => BUSY,
                TrySendError::Disconnected(_) => "选区读取不可用，请手动粘贴。",
            });
        }
        Ok(result)
    }
}

fn read_requests(incoming: Receiver<Request>, busy: Arc<AtomicBool>, wake: impl Fn()) {
    let apartment = match ComApartment::initialize() {
        Ok(apartment) => apartment,
        Err(error) => {
            log::warn!("初始化选区读取的 COM 线程失败：{:08X}", error.code().0);
            return;
        }
    };
    let automation = match apartment.automation() {
        Ok(automation) => automation,
        Err(error) => {
            log::warn!("初始化 UI Automation 失败：{:08X}", error.code().0);
            return;
        }
    };
    for request in incoming {
        let started = Instant::now();
        let result = read_selection(&automation, request.target, request.deadline)
            .inspect(|_| log::debug!("选区读取成功，耗时 {} ms", started.elapsed().as_millis()))
            .map_err(ReadError::message);
        busy.store(false, Ordering::Release);
        // 每个请求有独立接收端，放弃或超时后结果无法进入新的面板。
        if request.reply.send(result).is_ok() {
            wake();
        }
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> windows::core::Result<Self> {
        // SAFETY: 在新建的无窗口工作线程初始化 COM，成功后由 Drop 配对释放。
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        Ok(Self)
    }

    fn automation(&self) -> windows::core::Result<IUIAutomation> {
        // SAFETY: 当前工作线程已初始化 COM，所有接口由该线程独占。
        unsafe {
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
            let timeouts = automation.cast::<IUIAutomation2>()?;
            timeouts.SetConnectionTimeout(250)?;
            timeouts.SetTransactionTimeout(350)?;
            Ok(automation)
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: 工作线程先释放 UIA 对象，再与该线程成功的 CoInitializeEx 配对。
        unsafe { CoUninitialize() };
    }
}

#[derive(Debug)]
enum ReadError {
    Changed,
    Protected,
    Unsupported,
    Empty,
    Multiple,
    TooLarge,
    TimedOut,
    InvalidText,
    Api(Error),
}

impl From<Error> for ReadError {
    fn from(error: Error) -> Self {
        let code = error.code();
        match code.0 as u32 {
            UIA_E_ELEMENTNOTAVAILABLE => Self::Changed,
            UIA_E_TIMEOUT => Self::TimedOut,
            _ if code == ERROR_TIMEOUT.to_hresult() => Self::TimedOut,
            _ => Self::Api(error),
        }
    }
}

impl ReadError {
    fn message(self) -> &'static str {
        let message = match self {
            Self::Changed => "来源窗口或焦点已更换，请手动粘贴。",
            Self::Protected => "受保护的控件无法读取选区，请手动粘贴。",
            Self::Unsupported => "当前控件无法读取选区，请手动粘贴。",
            Self::Empty => "没有检测到选中文本，请手动粘贴。",
            Self::Multiple => "检测到多个选区，请改为单个选区或手动粘贴。",
            Self::TooLarge => "选区超出读取上限，请缩小选区或手动粘贴。",
            Self::TimedOut => TIMEOUT_MESSAGE,
            Self::InvalidText => "选区包含无效的 Unicode 文本，请手动粘贴。",
            Self::Api(error) => {
                log::debug!("UI Automation 读取选区失败：{:08X}", error.code().0);
                "无法读取当前选区，请手动粘贴。"
            }
        };
        log::debug!("选区回退：{message}");
        message
    }
}

fn check_deadline(deadline: Instant) -> Result<(), ReadError> {
    if Instant::now() >= deadline {
        return Err(ReadError::TimedOut);
    }
    Ok(())
}

fn read_selection(
    automation: &IUIAutomation,
    target: SelectionTarget,
    deadline: Instant,
) -> Result<String, ReadError> {
    check_deadline(deadline)?;
    if !target.is_current() {
        return Err(ReadError::Changed);
    }

    // SAFETY: UIA 调用发生在已初始化 COM 的工作线程，所有接口均由该线程持有。
    let (focused, walker) =
        unsafe { (automation.GetFocusedElement()?, automation.RawViewWalker()?) };
    let mut pattern = find_pattern(&walker, &focused, target, deadline)?;
    if pattern.is_none() && target.focus != target.source {
        check_deadline(deadline)?;
        // SAFETY: 焦点 HWND 已由来源上下文验证，后续仍会核对元素归属和焦点。
        let native = unsafe { automation.ElementFromHandle(target.focus.window())? };
        pattern = find_pattern(&walker, &native, target, deadline)?;
    }
    let pattern = pattern.ok_or(ReadError::Unsupported)?;
    check_focus(automation, &focused, target, deadline)?;
    let text = read_text(&pattern, deadline)?;
    check_focus(automation, &focused, target, deadline)?;
    Ok(text)
}

fn check_focus(
    automation: &IUIAutomation,
    original: &IUIAutomationElement,
    target: SelectionTarget,
    deadline: Instant,
) -> Result<(), ReadError> {
    check_deadline(deadline)?;
    if !target.is_current() {
        return Err(ReadError::Changed);
    }
    // SAFETY: 在原工作线程比较两个 UIA 元素，检测同一 HWND 内的控件焦点切换。
    let same = unsafe {
        automation
            .CompareElements(original, &automation.GetFocusedElement()?)?
            .as_bool()
    };
    check_deadline(deadline)?;
    if !same || !target.is_current() {
        return Err(ReadError::Changed);
    }
    Ok(())
}

fn find_pattern(
    walker: &IUIAutomationTreeWalker,
    start: &IUIAutomationElement,
    target: SelectionTarget,
    deadline: Instant,
) -> Result<Option<IUIAutomationTextPattern>, ReadError> {
    let mut path = Vec::new();
    let mut element = start.clone();
    // SAFETY: 只遍历本线程的 UIA 元素，找到第一个有效 HWND 后停止向上查找。
    unsafe {
        for _ in 0..MAX_ANCESTORS {
            check_deadline(deadline)?;
            if element.CurrentIsPassword()?.as_bool() {
                return Err(ReadError::Protected);
            }
            let hwnd = element.CurrentNativeWindowHandle()?;
            if !hwnd.is_invalid() {
                if !target.owns(hwnd) {
                    return Err(ReadError::Changed);
                }
                // 浏览器的无窗口元素可能属于渲染进程，以最近的原生窗口确认上下文。
                path.push(element);
                for candidate in path {
                    check_deadline(deadline)?;
                    match candidate.GetCurrentPatternAs(UIA_TextPatternId) {
                        Ok(pattern) => return Ok(Some(pattern)),
                        // COM 成功但返回空指针时，windows crate 返回 HRESULT 为 0 的空错误。
                        Err(error)
                            if error.code().is_ok()
                                || [E_NOINTERFACE, E_POINTER].contains(&error.code())
                                || error.code().0 as u32 == UIA_E_NOTSUPPORTED => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                return Ok(None);
            }
            path.push(element.clone());
            element = walker.GetParentElement(&element)?;
        }
    }
    Err(ReadError::Changed)
}

fn read_text(pattern: &IUIAutomationTextPattern, deadline: Instant) -> Result<String, ReadError> {
    // SAFETY: 只读取当前 TextPattern 返回的选区，UIA 对象均留在工作线程。
    unsafe {
        let ranges = match pattern.GetSelection() {
            Ok(ranges) => ranges,
            Err(error) if error.code().is_ok() => return Err(ReadError::Empty),
            Err(error) => return Err(error.into()),
        };
        check_deadline(deadline)?;
        let count = ranges.Length()?;
        if count > MAX_RANGES {
            return Err(ReadError::Multiple);
        }
        let mut selected = None;
        for index in 0..count {
            check_deadline(deadline)?;
            let range = ranges.GetElement(index)?;
            if range.CompareEndpoints(
                TextPatternRangeEndpoint_Start,
                &range,
                TextPatternRangeEndpoint_End,
            )? == 0
            {
                continue;
            }
            if selected.replace(range).is_some() {
                return Err(ReadError::Multiple);
            }
        }
        let range = selected.ok_or(ReadError::Empty)?;
        check_deadline(deadline)?;
        // 多读一个 UTF-16 单元来区分完整选区与被截断的结果。
        let text = range.GetText((MAX_TEXT_UNITS + 1) as i32)?;
        decode_text(&text)
    }
}

fn decode_text(text: &[u16]) -> Result<String, ReadError> {
    if text.is_empty() {
        return Err(ReadError::Empty);
    }
    if text.len() > MAX_TEXT_UNITS {
        return Err(ReadError::TooLarge);
    }
    String::from_utf16(text).map_err(|_| ReadError::InvalidText)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_truncation_and_keeps_complete_unicode_and_line_endings() {
        let original = "{\r\n\"值\":\"🦀\"\n}";
        let units: Vec<_> = original.encode_utf16().collect();
        assert_eq!(decode_text(&units).unwrap(), original);

        let mut boundary = vec![u16::from(b'x'); MAX_TEXT_UNITS];
        assert_eq!(decode_text(&boundary).unwrap().len(), MAX_TEXT_UNITS);
        boundary.push(u16::from(b'x'));
        assert!(matches!(decode_text(&boundary), Err(ReadError::TooLarge)));
        assert!(matches!(
            decode_text(&[0xd800]),
            Err(ReadError::InvalidText)
        ));
    }
}
