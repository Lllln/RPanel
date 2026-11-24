use std::mem::size_of;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
    VK_CONTROL,
};

use crate::{HookError, RPANEL_INJECTED_TAG};

const VK_C: VIRTUAL_KEY = VIRTUAL_KEY(0x43);

/// Synthesize a tagged `Ctrl+C` keystroke using `SendInput`.
pub fn send_ctrl_c() -> Result<(), HookError> {
    let tag = RPANEL_INJECTED_TAG as usize;
    let inputs = [
        key_input(VK_CONTROL, false, tag),
        key_input(VK_C, false, tag),
        key_input(VK_C, true, tag),
        key_input(VK_CONTROL, true, tag),
    ];

    // SAFETY: `inputs` is a contiguous slice of valid Win32 `INPUT` structs.
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        return Err(HookError::InstallFailed(format!(
            "SendInput sent {sent}/{} events (target window may be elevated)",
            inputs.len()
        )));
    }
    Ok(())
}

fn key_input(vk: VIRTUAL_KEY, key_up: bool, extra_info: usize) -> INPUT {
    let flags = if key_up {
        KEYEVENTF_KEYUP
    } else {
        Default::default()
    };

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: extra_info,
            },
        },
    }
}
