use std::mem::size_of;
use std::thread;
use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_INSERT, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

use crate::{HookError, RPANEL_INJECTED_TAG};

const VK_C: VIRTUAL_KEY = VIRTUAL_KEY(0x43);

/// Synthesize a tagged `Ctrl+C` keystroke for standard Windows apps.
pub fn send_ctrl_c() -> Result<(), HookError> {
    send_chord(&[VK_CONTROL], VK_C, false)
}

/// Synthesize a tagged `Ctrl+Shift+C` keystroke for terminal emulators / SSH clients
/// (WindTerm, Windows Terminal, Tabby, WezTerm, Alacritty, Kitty, mintty, FinalShell, etc.)
/// without sending `SIGINT` (`^C`) to the active shell.
pub fn send_ctrl_shift_c() -> Result<(), HookError> {
    send_chord(&[VK_CONTROL, VK_SHIFT], VK_C, false)
}

/// Synthesize a tagged `Ctrl+Insert` keystroke (classic CUA Copy supported by ConHost/cmd,
/// PowerShell, PuTTY, Xshell, MobaXterm, SecureCRT, WindTerm, JetBrains terminals, and Qt/Win32
/// controls) without ever sending `SIGINT`.
pub fn send_ctrl_insert() -> Result<(), HookError> {
    send_chord(&[VK_CONTROL], VK_INSERT, true)
}

/// Synthesize a keyboard chord with hardware scan codes (`MapVirtualKeyW`), extended-key flags,
/// conflicting-modifier hygiene, and short inter-step spacing so Win32, Qt (WindTerm), Electron,
/// Java/JetBrains, and RDP/VDI windows all receive it reliably.
fn send_chord(
    modifiers: &[VIRTUAL_KEY],
    key: VIRTUAL_KEY,
    key_extended: bool,
) -> Result<(), HookError> {
    let tag = RPANEL_INJECTED_TAG as usize;

    // Release any unrelated modifier currently held down (e.g. Alt/Shift/Win) so it doesn't
    // corrupt the synthetic copy shortcut.
    let conflicting: Vec<INPUT> = [VK_CONTROL, VK_SHIFT, VK_MENU, VK_LWIN, VK_RWIN]
        .into_iter()
        .filter(|vk| !modifiers.contains(vk) && is_key_down(*vk))
        .map(|vk| key_input(vk, true, is_extended_modifier(vk), tag))
        .collect();
    if !conflicting.is_empty() {
        send_inputs(&conflicting)?;
        thread::sleep(Duration::from_millis(8));
    }

    let mods_down: Vec<INPUT> = modifiers
        .iter()
        .map(|&vk| key_input(vk, false, is_extended_modifier(vk), tag))
        .collect();
    send_inputs(&mods_down)?;
    thread::sleep(Duration::from_millis(10));

    send_inputs(&[key_input(key, false, key_extended, tag)])?;
    thread::sleep(Duration::from_millis(10));

    send_inputs(&[key_input(key, true, key_extended, tag)])?;
    thread::sleep(Duration::from_millis(10));

    let mods_up: Vec<INPUT> = modifiers
        .iter()
        .rev()
        .map(|&vk| key_input(vk, true, is_extended_modifier(vk), tag))
        .collect();
    send_inputs(&mods_up)?;

    Ok(())
}

fn is_key_down(vk: VIRTUAL_KEY) -> bool {
    // SAFETY: `GetAsyncKeyState` is thread-safe and side-effect free.
    (unsafe { GetAsyncKeyState(i32::from(vk.0)) } as u16 & 0x8000) != 0
}

fn is_extended_modifier(vk: VIRTUAL_KEY) -> bool {
    matches!(vk, VK_LWIN | VK_RWIN)
}

fn send_inputs(inputs: &[INPUT]) -> Result<(), HookError> {
    // SAFETY: `inputs` is a contiguous slice of valid Win32 `INPUT` structs.
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err(HookError::InstallFailed(format!(
            "SendInput sent {sent}/{} events (target window may be elevated)",
            inputs.len()
        )));
    }
    Ok(())
}

fn key_input(vk: VIRTUAL_KEY, key_up: bool, extended: bool, extra_info: usize) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS::default();
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }

    // SAFETY: `MapVirtualKeyW` translates a Win32 virtual key to its hardware scan code.
    let scan = unsafe { MapVirtualKeyW(u32::from(vk.0), MAPVK_VK_TO_VSC) } as u16;

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: extra_info,
            },
        },
    }
}
