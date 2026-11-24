use std::ffi::c_void;
use std::ptr;

use core_foundation::base::{CFRelease, CFTypeID, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};

use crate::{CaptureResult, CaptureSource};

type AXUIElementRef = *mut c_void;
type AXError = i32;

const K_AX_ERROR_SUCCESS: AXError = 0;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn CFGetTypeID(cf: CFTypeRef) -> CFTypeID;
}

/// Silent macOS Accessibility (`AXUIElement`) selection reader.
///
/// Queries the system-wide focused UI element for `AXSelectedText`.
/// Returns `None` when the target application does not expose selection via Accessibility
/// (e.g. some Electron/browser/terminal views), allowing seamless fallback to `Cmd+C`.
pub fn try_read_selected_text() -> Option<CaptureResult> {
    // SAFETY: All created/copied CF objects are checked for null and released on every path.
    unsafe {
        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            return None;
        }

        let focused_attr = CFString::new("AXFocusedUIElement");
        let mut focused_ref: CFTypeRef = ptr::null();
        let err = AXUIElementCopyAttributeValue(
            system_wide,
            focused_attr.as_concrete_TypeRef(),
            &mut focused_ref,
        );
        CFRelease(system_wide as CFTypeRef);

        if err != K_AX_ERROR_SUCCESS || focused_ref.is_null() {
            return None;
        }

        let selected_attr = CFString::new("AXSelectedText");
        let mut selected_ref: CFTypeRef = ptr::null();
        let err = AXUIElementCopyAttributeValue(
            focused_ref as AXUIElementRef,
            selected_attr.as_concrete_TypeRef(),
            &mut selected_ref,
        );
        CFRelease(focused_ref);

        if err != K_AX_ERROR_SUCCESS || selected_ref.is_null() {
            return None;
        }

        if CFGetTypeID(selected_ref) != CFString::type_id() {
            CFRelease(selected_ref);
            return None;
        }

        let text = CFString::wrap_under_create_rule(selected_ref as CFStringRef).to_string();
        if text.is_empty() {
            return None;
        }

        Some(CaptureResult {
            text: Some(text),
            source: CaptureSource::Accessibility,
        })
    }
}
