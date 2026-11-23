use std::io;

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

#[derive(Clone, Copy)]
pub struct Clipboard;

impl Clipboard {
    pub fn read(self) -> io::Result<Option<String>> {
        // SAFETY: NSPasteboardTypeString 是 AppKit 提供的进程内常量，GUI 线程调用。
        let text =
            NSPasteboard::generalPasteboard().stringForType(unsafe { NSPasteboardTypeString });
        Ok(text
            .map(|text| text.to_string())
            .filter(|text| !text.is_empty()))
    }

    pub fn write(self, text: &str) -> io::Result<()> {
        let pasteboard = NSPasteboard::generalPasteboard();
        let text = NSString::from_str(text);
        pasteboard.clearContents();
        // SAFETY: NSPasteboardTypeString 是 AppKit 提供的进程内常量，GUI 线程调用。
        if pasteboard.setString_forType(&text, unsafe { NSPasteboardTypeString }) {
            Ok(())
        } else {
            Err(io::Error::other("无法写入系统剪贴板"))
        }
    }
}
