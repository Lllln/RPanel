use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use arboard::Clipboard;
use tracing::debug;

use crate::{CaptureResult, CaptureSource, HookError, platform};

/// Maximum age of a left-mouse-down selection for treating a subsequent clipboard change
/// as a fresh terminal auto-copy-on-select (e.g. WindTerm, PuTTY, Xshell, MobaXterm, iTerm2).
const AUTO_COPY_WINDOW: Duration = Duration::from_secs(10);

/// Polling interval while waiting for the OS clipboard sequence number to advance.
const SEQ_POLL_INTERVAL: Duration = Duration::from_millis(15);

/// Sentinel value meaning RPanel has not recorded any own clipboard write yet.
const NO_RECORDED_SEQ: u64 = u64::MAX;

/// Sequence number recorded immediately after RPanel itself writes/restores the clipboard,
/// so RPanel's own clipboard writes are never mistaken for a user's auto-copy selection.
static LAST_RPANEL_WRITE_SEQ: AtomicU64 = AtomicU64::new(NO_RECORDED_SEQ);

/// Clipboard change counter snapshot taken on the most recent left-mouse-down event.
/// If a terminal app auto-copies text when the user drags/double-clicks with the left button,
/// the clipboard sequence number will advance strictly after this baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoCopyBaseline {
    pub seq: u64,
    pub at: Instant,
}

/// A single shortcut attempt in a prioritized selection-capture sequence.
pub struct CopyShortcutStep<'a> {
    pub label: &'static str,
    pub timeout: Duration,
    pub trigger: &'a dyn Fn() -> Result<(), HookError>,
}

/// Read current clipboard text directly (used by "Reload from Clipboard").
pub fn read_clipboard_text() -> Result<String, HookError> {
    let mut clipboard = Clipboard::new().map_err(|e| HookError::ClipboardFailed(e.to_string()))?;
    clipboard
        .get_text()
        .map_err(|e| HookError::ClipboardFailed(e.to_string()))
}

/// Read clipboard text with brief retries to tolerate transient `OpenClipboard` contention
/// (e.g. Windows `rdpclip.exe`, clipboard history, or delayed rendering right after copy).
fn read_clipboard_text_with_retry(attempts: usize, delay: Duration) -> Option<String> {
    for attempt in 0..attempts {
        if let Ok(text) = read_clipboard_text()
            && !text.is_empty()
        {
            return Some(text);
        }
        if attempt + 1 < attempts {
            thread::sleep(delay);
        }
    }
    None
}

/// Write text to the system clipboard (used by "Copy & Hide").
pub fn write_clipboard_text(text: &str) -> Result<(), HookError> {
    let mut clipboard = Clipboard::new().map_err(|e| HookError::ClipboardFailed(e.to_string()))?;
    clipboard
        .set_text(text.to_owned())
        .map_err(|e| HookError::ClipboardFailed(e.to_string()))?;
    drop(clipboard);
    record_rpanel_clipboard_write();
    Ok(())
}

fn record_rpanel_clipboard_write() {
    if let Some(seq) = platform::clipboard_sequence_number() {
        LAST_RPANEL_WRITE_SEQ.store(seq, Ordering::Release);
    }
}

fn last_rpanel_write_seq() -> Option<u64> {
    match LAST_RPANEL_WRITE_SEQ.load(Ordering::Acquire) {
        NO_RECORDED_SEQ => None,
        seq => Some(seq),
    }
}

/// Backup current clipboard text, trigger a single platform copy shortcut (`Cmd+C` / `Ctrl+C`),
/// poll for updated clipboard content, and restore the original clipboard.
pub fn capture_via_clipboard_shortcut<F>(trigger_copy: F) -> Result<CaptureResult, HookError>
where
    F: Fn() -> Result<(), HookError>,
{
    let steps = [CopyShortcutStep {
        label: "copy",
        timeout: Duration::from_millis(240),
        trigger: &trigger_copy,
    }];
    capture_via_shortcuts(&steps, None)
}

/// Capture selected text by trying one or more copy shortcuts in order without writing
/// sentinel strings into the system clipboard.
///
/// Uses lock-free OS clipboard sequence numbers (`GetClipboardSequenceNumber` on Windows,
/// `NSPasteboard.changeCount` on macOS) so polling never contends for `OpenClipboard()` or
/// pollutes clipboard history / RDP sync. If no shortcut triggers a clipboard update,
/// checks `auto_copy_baseline` to detect terminals that already auto-copied the selection
/// on mouse-up (e.g. WindTerm `autoCopySelection`, PuTTY, Xshell, MobaXterm, iTerm2).
pub fn capture_via_shortcuts(
    steps: &[CopyShortcutStep<'_>],
    auto_copy_baseline: Option<AutoCopyBaseline>,
) -> Result<CaptureResult, HookError> {
    if let Some(seq_before) = platform::clipboard_sequence_number() {
        capture_via_sequence_number(
            steps,
            auto_copy_baseline,
            seq_before,
            platform::clipboard_sequence_number,
            || read_clipboard_text().ok(),
            || read_clipboard_text_with_retry(4, Duration::from_millis(15)),
            |text| {
                let _ = write_clipboard_text(text);
            },
            last_rpanel_write_seq(),
        )
    } else {
        capture_via_sentinel_fallback(steps)
    }
}

#[allow(clippy::too_many_arguments)]
fn capture_via_sequence_number<FSeq, FRead, FReadRetry, FRestore>(
    steps: &[CopyShortcutStep<'_>],
    auto_copy_baseline: Option<AutoCopyBaseline>,
    seq_before: u64,
    read_seq: FSeq,
    read_initial: FRead,
    read_after_copy: FReadRetry,
    restore_clipboard: FRestore,
    rpanel_write_seq: Option<u64>,
) -> Result<CaptureResult, HookError>
where
    FSeq: Fn() -> Option<u64>,
    FRead: FnOnce() -> Option<String>,
    FReadRetry: FnOnce() -> Option<String>,
    FRestore: FnOnce(&str),
{
    // Snapshot previous text once upfront and release the OS clipboard handle immediately
    // so the target application never hits an `OpenClipboard` lock when copying.
    let previous = read_initial();
    let mut last_error: Option<HookError> = None;

    for step in steps {
        if let Err(err) = (step.trigger)() {
            debug!(shortcut = step.label, "copy shortcut synthesis failed: {err}");
            last_error = Some(err);
            continue;
        }

        let deadline = Instant::now() + step.timeout;
        let mut changed = false;
        while Instant::now() < deadline {
            thread::sleep(SEQ_POLL_INTERVAL);
            if read_seq().is_some_and(|current| current != seq_before) {
                changed = true;
                break;
            }
        }

        if changed {
            let captured = read_after_copy();
            debug!(
                shortcut = step.label,
                captured_len = captured.as_ref().map(|s| s.chars().count()).unwrap_or(0),
                "clipboard sequence advanced after copy shortcut"
            );

            if let Some(prev) = previous.as_deref() {
                // Brief pause so clipboard observers finish reading before we restore.
                thread::sleep(Duration::from_millis(10));
                restore_clipboard(prev);
            }

            let text = captured.filter(|s| !s.trim().is_empty());
            let source = if text.is_some() {
                CaptureSource::ClipboardShortcut
            } else {
                CaptureSource::None
            };
            return Ok(CaptureResult { text, source });
        }
    }

    // No shortcut updated the clipboard. Check whether the user's most recent mouse selection
    // already auto-copied text into the clipboard (common in WindTerm, PuTTY, Xshell, iTerm2).
    if let Some(baseline) = auto_copy_baseline
        && baseline.at.elapsed() <= AUTO_COPY_WINDOW
        && seq_before != baseline.seq
        && rpanel_write_seq != Some(seq_before)
        && let Some(text) = previous.filter(|s| !s.trim().is_empty())
    {
        debug!(
            chars = text.chars().count(),
            baseline_seq = baseline.seq,
            current_seq = seq_before,
            "captured fresh selection via terminal auto-copy-on-select"
        );
        return Ok(CaptureResult {
            text: Some(text),
            source: CaptureSource::ClipboardAutoCopy,
        });
    }

    if let Some(err) = last_error {
        return Err(err);
    }

    Ok(CaptureResult {
        text: None,
        source: CaptureSource::None,
    })
}

/// Legacy sentinel fallback used only on platforms that do not expose a clipboard sequence counter.
fn capture_via_sentinel_fallback(steps: &[CopyShortcutStep<'_>]) -> Result<CaptureResult, HookError> {
    let mut clipboard = Clipboard::new().map_err(|e| HookError::ClipboardFailed(e.to_string()))?;
    let previous = clipboard.get_text().ok();
    let sentinel = format!("__RPANEL_SENTINEL_{}__", std::process::id());
    let _ = clipboard.set_text(sentinel.clone());

    let mut captured = None;
    for step in steps {
        if (step.trigger)().is_err() {
            continue;
        }
        let deadline = Instant::now() + step.timeout;
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
            if let Ok(current) = clipboard.get_text()
                && current != sentinel
            {
                captured = Some(current);
                break;
            }
        }
        if captured.is_some() {
            break;
        }
    }

    if let Some(prev) = previous {
        let _ = clipboard.set_text(prev);
    }

    let text = captured.filter(|s| !s.trim().is_empty());
    let source = if text.is_some() {
        CaptureSource::ClipboardShortcut
    } else {
        CaptureSource::None
    };

    Ok(CaptureResult { text, source })
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn sequence_capture_succeeds_on_fallback_shortcut_and_restores_previous() {
        let seq = AtomicU64::new(10);
        let restored = RefCell::new(None::<String>);

        let primary_noop = || Ok(());
        let fallback_copies = || {
            seq.store(11, Ordering::SeqCst);
            Ok(())
        };

        let steps = [
            CopyShortcutStep {
                label: "ctrl+shift+c",
                timeout: Duration::from_millis(30),
                trigger: &primary_noop,
            },
            CopyShortcutStep {
                label: "ctrl+insert",
                timeout: Duration::from_millis(60),
                trigger: &fallback_copies,
            },
        ];

        let result = capture_via_sequence_number(
            &steps,
            None,
            10,
            || Some(seq.load(Ordering::SeqCst)),
            || Some("old-clipboard".to_string()),
            || Some("1732619278".to_string()),
            |prev| {
                *restored.borrow_mut() = Some(prev.to_string());
            },
            None,
        )
        .expect("capture should succeed");

        assert_eq!(
            result,
            CaptureResult {
                text: Some("1732619278".to_string()),
                source: CaptureSource::ClipboardShortcut,
            }
        );
        assert_eq!(restored.into_inner().as_deref(), Some("old-clipboard"));
    }

    #[test]
    fn sequence_capture_uses_recent_auto_copy_when_shortcuts_do_not_change_clipboard() {
        let noop = || Ok(());
        let steps = [CopyShortcutStep {
            label: "ctrl+shift+c",
            timeout: Duration::from_millis(30),
            trigger: &noop,
        }];

        let baseline = AutoCopyBaseline {
            seq: 20,
            at: Instant::now(),
        };

        let result = capture_via_sequence_number(
            &steps,
            Some(baseline),
            21,
            || Some(21),
            || Some("1732619278000".to_string()),
            || None,
            |_| panic!("should not restore clipboard when no shortcut modified it"),
            None,
        )
        .expect("capture should return auto-copied selection");

        assert_eq!(
            result,
            CaptureResult {
                text: Some("1732619278000".to_string()),
                source: CaptureSource::ClipboardAutoCopy,
            }
        );
    }

    #[test]
    fn sequence_capture_ignores_stale_clipboard_and_rpanel_own_writes() {
        let noop = || Ok(());
        let steps = [CopyShortcutStep {
            label: "ctrl+c",
            timeout: Duration::from_millis(30),
            trigger: &noop,
        }];

        // Case 1: Clipboard did not change since LeftMouseDown -> must return None.
        let unchanged = capture_via_sequence_number(
            &steps,
            Some(AutoCopyBaseline {
                seq: 30,
                at: Instant::now(),
            }),
            30,
            || Some(30),
            || Some("stale-clipboard".to_string()),
            || None,
            |_| {},
            None,
        )
        .unwrap();
        assert_eq!(unchanged.source, CaptureSource::None);
        assert_eq!(unchanged.text, None);

        // Case 2: Clipboard changed because RPanel itself wrote/restored it -> must return None.
        let own_write = capture_via_sequence_number(
            &steps,
            Some(AutoCopyBaseline {
                seq: 30,
                at: Instant::now(),
            }),
            31,
            || Some(31),
            || Some("rpanel-copied".to_string()),
            || None,
            |_| {},
            Some(31),
        )
        .unwrap();
        assert_eq!(own_write.source, CaptureSource::None);
        assert_eq!(own_write.text, None);
    }
}
