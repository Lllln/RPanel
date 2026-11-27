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
/// Queries the system-wide focused UI element (falling back to `AXFocusedApplication`'s focused
/// element) for `AXSelectedText`. Returns `None` when the target application does not expose
/// non-empty selection via Accessibility (e.g. Qt/WindTerm, Electron, or custom terminal views),
/// allowing seamless fallback to keyboard copy shortcuts.
pub fn try_read_selected_text() -> Option<CaptureResult> {
    // SAFETY: All created/copied CF objects are checked for null and released on every path.
    unsafe {
        let system_wide = AXUIElementCreateSystemWide();
        if system_wide.is_null() {
            return None;
        }

        let focused_elem = copy_focused_element(system_wide);
        CFRelease(system_wide as CFTypeRef);

        let focused_elem = focused_elem?;
        let selected_text = copy_selected_text(focused_elem);
        CFRelease(focused_elem as CFTypeRef);

        let text = selected_text?;
        if text.trim().is_empty() {
            return None;
        }

        Some(CaptureResult {
            text: Some(text),
            source: CaptureSource::Accessibility,
        })
    }
}

unsafe fn copy_focused_element(system_wide: AXUIElementRef) -> Option<AXUIElementRef> {
    let focused_attr = CFString::new("AXFocusedUIElement");
    let mut focused_ref: CFTypeRef = ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(
            system_wide,
            focused_attr.as_concrete_TypeRef(),
            &mut focused_ref,
        )
    };
    if err == K_AX_ERROR_SUCCESS && !focused_ref.is_null() {
        return Some(focused_ref as AXUIElementRef);
    }

    // Some apps only expose `AXFocusedUIElement` through `AXFocusedApplication`.
    let app_attr = CFString::new("AXFocusedApplication");
    let mut app_ref: CFTypeRef = ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(system_wide, app_attr.as_concrete_TypeRef(), &mut app_ref)
    };
    if err != K_AX_ERROR_SUCCESS || app_ref.is_null() {
        return None;
    }

    let mut nested_focused: CFTypeRef = ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(
            app_ref as AXUIElementRef,
            focused_attr.as_concrete_TypeRef(),
            &mut nested_focused,
        )
    };
    unsafe { CFRelease(app_ref) };

    if err == K_AX_ERROR_SUCCESS && !nested_focused.is_null() {
        Some(nested_focused as AXUIElementRef)
    } else {
        None
    }
}

unsafe fn copy_selected_text(element: AXUIElementRef) -> Option<String> {
    let selected_attr = CFString::new("AXSelectedText");
    let mut selected_ref: CFTypeRef = ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(
            element,
            selected_attr.as_concrete_TypeRef(),
            &mut selected_ref,
        )
    };

    if err != K_AX_ERROR_SUCCESS || selected_ref.is_null() {
        return None;
    }

    if unsafe { CFGetTypeID(selected_ref) } != CFString::type_id() {
        unsafe { CFRelease(selected_ref) };
        return None;
    }

    Some(unsafe { CFString::wrap_under_create_rule(selected_ref as CFStringRef) }.to_string())
}
