use std::thread;
use std::time::Duration;

use arboard::Clipboard;

use crate::{CaptureResult, CaptureSource, HookError};

/// Read current clipboard text directly (used by "Reload from Clipboard").
pub fn read_clipboard_text() -> Result<String, HookError> {
    let mut clipboard = Clipboard::new().map_err(|e| HookError::ClipboardFailed(e.to_string()))?;
    clipboard
        .get_text()
        .map_err(|e| HookError::ClipboardFailed(e.to_string()))
}

/// Write text to the system clipboard (used by "Copy & Hide").
pub fn write_clipboard_text(text: &str) -> Result<(), HookError> {
    let mut clipboard = Clipboard::new().map_err(|e| HookError::ClipboardFailed(e.to_string()))?;
    clipboard
        .set_text(text.to_owned())
        .map_err(|e| HookError::ClipboardFailed(e.to_string()))
}

/// Backup current clipboard text, trigger platform copy shortcut (`Cmd+C` / `Ctrl+C`),
/// poll briefly for updated clipboard content, and restore the original clipboard.
pub fn capture_via_clipboard_shortcut<F>(trigger_copy: F) -> Result<CaptureResult, HookError>
where
    F: FnOnce() -> Result<(), HookError>,
{
    let mut clipboard = Clipboard::new().map_err(|e| HookError::ClipboardFailed(e.to_string()))?;
    let previous = clipboard.get_text().ok();
    let sentinel = format!("__RPANEL_SENTINEL_{}__", std::process::id());
    let _ = clipboard.set_text(sentinel.clone());

    trigger_copy()?;

    let mut captured = None;
    for _ in 0..8 {
        thread::sleep(Duration::from_millis(20));
        if let Ok(current) = clipboard.get_text()
            && current != sentinel
        {
            captured = Some(current);
            break;
        }
    }

    if let Some(prev) = previous {
        let _ = clipboard.set_text(prev);
    }

    let source = if captured.is_some() {
        CaptureSource::ClipboardShortcut
    } else {
        CaptureSource::None
    };

    Ok(CaptureResult {
        text: captured,
        source,
    })
}
