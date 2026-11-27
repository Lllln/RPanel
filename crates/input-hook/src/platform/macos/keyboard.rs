use std::thread;
use std::time::Duration;

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, EventField};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

use crate::{HookError, RPANEL_INJECTED_TAG};

/// macOS virtual key codes (Carbon/HIToolbox).
const VK_ANSI_C: u16 = 0x08;
const VK_COMMAND: u16 = 0x37;
const VK_SHIFT: u16 = 0x38;
const VK_CONTROL: u16 = 0x3B;

/// Synthesize a tagged `Cmd+C` keystroke to copy the active selection.
pub fn send_cmd_c() -> Result<(), HookError> {
    send_chord(&[(VK_COMMAND, CGEventFlags::CGEventFlagCommand)], VK_ANSI_C)
}

/// Synthesize a tagged `Cmd+Shift+C` keystroke (used by some macOS terminal/SSH keymaps).
pub fn send_cmd_shift_c() -> Result<(), HookError> {
    send_chord(
        &[
            (VK_COMMAND, CGEventFlags::CGEventFlagCommand),
            (VK_SHIFT, CGEventFlags::CGEventFlagShift),
        ],
        VK_ANSI_C,
    )
}

/// Synthesize a tagged `Ctrl+Shift+C` keystroke (cross-platform terminal copy shortcut used by
/// WindTerm and other Qt/Electron terminal emulators without sending `SIGINT`).
pub fn send_ctrl_shift_c() -> Result<(), HookError> {
    send_chord(
        &[
            (VK_CONTROL, CGEventFlags::CGEventFlagControl),
            (VK_SHIFT, CGEventFlags::CGEventFlagShift),
        ],
        VK_ANSI_C,
    )
}

/// Synthesize a clean, isolated keyboard chord using `CGEventSourceStateID::Private` and
/// `CGEventTapLocation::Session`, emitting explicit `FlagsChanged` transitions and brief
/// inter-event delays so Cocoa, Qt (WindTerm), Electron, and GPU terminals all receive it reliably
/// even while the physical right mouse button is still held down.
fn send_chord(modifiers: &[(u16, CGEventFlags)], key_code: u16) -> Result<(), HookError> {
    let source = CGEventSource::new(CGEventSourceStateID::Private)
        .map_err(|_| HookError::InstallFailed("Failed to create CGEventSource".into()))?;

    let mut active_flags = CGEventFlags::CGEventFlagNull;
    for &(mod_vk, flag) in modifiers {
        active_flags |= flag;
        post_flags_changed(&source, mod_vk, true, active_flags)?;
    }

    thread::sleep(Duration::from_millis(5));
    post_key_event(&source, key_code, true, active_flags)?;
    thread::sleep(Duration::from_millis(10));
    post_key_event(&source, key_code, false, active_flags)?;
    thread::sleep(Duration::from_millis(5));

    for &(mod_vk, flag) in modifiers.iter().rev() {
        active_flags.remove(flag);
        post_flags_changed(&source, mod_vk, false, active_flags)?;
    }

    Ok(())
}

fn post_flags_changed(
    source: &CGEventSource,
    key_code: u16,
    key_down: bool,
    flags: CGEventFlags,
) -> Result<(), HookError> {
    let event = CGEvent::new_keyboard_event(source.clone(), key_code, key_down)
        .map_err(|_| HookError::InstallFailed("Failed to create modifier CGEvent".into()))?;
    event.set_type(CGEventType::FlagsChanged);
    event.set_flags(flags);
    event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, RPANEL_INJECTED_TAG as i64);
    event.post(CGEventTapLocation::Session);
    Ok(())
}

fn post_key_event(
    source: &CGEventSource,
    key_code: u16,
    key_down: bool,
    flags: CGEventFlags,
) -> Result<(), HookError> {
    let event = CGEvent::new_keyboard_event(source.clone(), key_code, key_down)
        .map_err(|_| HookError::InstallFailed("Failed to create keyboard CGEvent".into()))?;
    event.set_flags(flags);
    event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, RPANEL_INJECTED_TAG as i64);
    event.post(CGEventTapLocation::Session);
    Ok(())
}
