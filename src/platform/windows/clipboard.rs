use std::{mem::size_of, ptr::NonNull};

use windows::{
    Win32::{
        Foundation::{E_INVALIDARG, GlobalFree, HANDLE, HGLOBAL, HWND},
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
                OpenClipboard, SetClipboardData,
            },
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
            Ole::CF_UNICODETEXT,
        },
    },
    core::{Error, Result},
};

#[derive(Clone, Copy)]
pub struct Clipboard(usize);

impl Clipboard {
    pub(super) fn new(owner: HWND) -> Self {
        Self(owner.0 as usize)
    }

    fn open(self) -> Result<OpenClipboardGuard> {
        // SAFETY: 在 GUI 线程调用。HWND 属于仍由 RPanel 持有的根窗口。
        unsafe { OpenClipboard(Some(HWND(self.0 as *mut _)))? };
        Ok(OpenClipboardGuard)
    }

    pub fn write(self, text: &str) -> Result<()> {
        if text.contains('\0') {
            return Err(Error::new(E_INVALIDARG, "剪贴板文本不能包含 NUL"));
        }
        let text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: 分配的容量与 UTF-16 缓冲区相同。先准备数据，再清空剪贴板。
        let buffer =
            ClipboardBuffer(unsafe { GlobalAlloc(GMEM_MOVEABLE, size_of::<u16>() * text.len())? });
        {
            let lock = LockedBuffer::new(buffer.0)?;
            // SAFETY: 目标已锁定，容量为 text.len() 个 UTF-16 单元，两块内存互不重叠。
            unsafe {
                lock.pointer
                    .as_ptr()
                    .copy_from_nonoverlapping(text.as_ptr(), text.len())
            };
        }
        let _clipboard = self.open()?;
        // SAFETY: 剪贴板已为本窗口打开，SetClipboardData 成功后转移内存所有权。
        unsafe {
            EmptyClipboard()?;
            SetClipboardData(CF_UNICODETEXT.0.into(), Some(HANDLE(buffer.0.0)))?;
        }
        std::mem::forget(buffer);
        Ok(())
    }

    pub fn read(self) -> Result<Option<String>> {
        let _clipboard = self.open()?;
        let format = CF_UNICODETEXT.0.into();
        // SAFETY: 剪贴板在读取和锁定其借用内存的全过程保持打开。
        unsafe {
            if IsClipboardFormatAvailable(format).is_err() {
                return Ok(None);
            }
            let memory = HGLOBAL(GetClipboardData(format)?.0);
            let capacity = GlobalSize(memory) / size_of::<u16>();
            if capacity == 0 || capacity > isize::MAX as usize / size_of::<u16>() {
                return Err(Error::new(E_INVALIDARG, "剪贴板文本大小无效"));
            }
            let lock = LockedBuffer::new(memory)?;
            let units = std::slice::from_raw_parts(lock.pointer.as_ptr(), capacity);
            let end = units
                .iter()
                .position(|&unit| unit == 0)
                .ok_or_else(|| Error::new(E_INVALIDARG, "剪贴板文本缺少结束符"))?;
            if end == 0 {
                return Ok(None);
            }
            String::from_utf16(&units[..end])
                .map(Some)
                .map_err(|_| Error::new(E_INVALIDARG, "剪贴板包含无效的 Unicode 文本"))
        }
    }
}

struct OpenClipboardGuard;

impl Drop for OpenClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: 只在成功打开剪贴板的 GUI 线程配对关闭。
        if let Err(error) = unsafe { CloseClipboard() } {
            log::warn!("无法关闭剪贴板：{error}");
        }
    }
}

struct ClipboardBuffer(HGLOBAL);

impl Drop for ClipboardBuffer {
    fn drop(&mut self) {
        // SAFETY: 只释放自行分配且尚未转交给剪贴板的内存。
        // GlobalFree 成功时返回空，windows crate 将非空返回值包装为 Ok。
        if unsafe { GlobalFree(Some(self.0)) }.is_ok() {
            log::warn!("无法释放剪贴板缓冲区：{}", Error::from_thread());
        }
    }
}

struct LockedBuffer {
    memory: HGLOBAL,
    pointer: NonNull<u16>,
}

impl LockedBuffer {
    fn new(memory: HGLOBAL) -> Result<Self> {
        // SAFETY: 内存由本进程分配或从当前打开的剪贴板借用。
        let pointer =
            NonNull::new(unsafe { GlobalLock(memory) }.cast()).ok_or_else(Error::from_thread)?;
        Ok(Self { memory, pointer })
    }
}

impl Drop for LockedBuffer {
    fn drop(&mut self) {
        // SAFETY: 内存仍有效，只释放本次成功的锁。锁计数归零时 Win32 正常返回 FALSE。
        if let Err(error) = unsafe { GlobalUnlock(self.memory) }
            && error.code().is_err()
        {
            log::warn!("无法解锁剪贴板缓冲区：{error}");
        }
    }
}
