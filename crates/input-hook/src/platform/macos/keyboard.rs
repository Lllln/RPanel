use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, EventField};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

use crate::{HookError, RPANEL_INJECTED_TAG};

/// macOS virtual key code for ANSI 'C'.
const VK_ANSI_C: u16 = 0x08;

/// Synthesize a tagged `Cmd+C` keystroke to copy the active selection.
pub fn send_cmd_c() -> Result<(), HookError> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| HookError::InstallFailed("Failed to create CGEventSource".into()))?;

    let key_down = CGEvent::new_keyboard_event(source.clone(), VK_ANSI_C, true)
        .map_err(|_| HookError::InstallFailed("Failed to create Cmd+C key-down event".into()))?;
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    key_down.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, RPANEL_INJECTED_TAG as i64);

    let key_up = CGEvent::new_keyboard_event(source, VK_ANSI_C, false)
        .map_err(|_| HookError::InstallFailed("Failed to create Cmd+C key-up event".into()))?;
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);
    key_up.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, RPANEL_INJECTED_TAG as i64);

    key_down.post(CGEventTapLocation::HID);
    key_up.post(CGEventTapLocation::HID);
    Ok(())
}
