use coosenpai_core::ports::{WindowBounds, FOCUS_ELEMENT_TIMEOUT};
use core_foundation::base::{CFRange, CFType, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use std::ffi::c_void;
use std::ptr;
use std::time::{Duration, Instant};

type AXUIElementRef = *const c_void;
type AXError = i32;
type AXValueType = u32;

const AX_SUCCESS: AXError = 0;
const AX_VALUE_CG_RECT: AXValueType = 3;
const AX_VALUE_CF_RANGE: AXValueType = 4;
const AX_FOCUSED_UI_ELEMENT: &str = "AXFocusedUIElement";
const AX_SELECTED_TEXT_RANGE: &str = "AXSelectedTextRange";
const AX_BOUNDS_FOR_RANGE: &str = "AXBoundsForRange";

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementCopyParameterizedAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        parameter: CFTypeRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> AXError;
    fn AXValueCreate(value_type: AXValueType, value: *const c_void) -> CFTypeRef;
    fn AXValueGetType(value: CFTypeRef) -> AXValueType;
    fn AXValueGetValue(value: CFTypeRef, value_type: AXValueType, output: *mut c_void) -> u8;
}

pub(crate) fn read_selected_text_bounds() -> Option<WindowBounds> {
    let deadline = Instant::now() + FOCUS_ELEMENT_TIMEOUT;
    if unsafe { AXIsProcessTrusted() } == 0 {
        return None;
    }
    let system = unsafe { AXUIElementCreateSystemWide() };
    if system.is_null() {
        return None;
    }
    let system = unsafe { CFType::wrap_under_create_rule(system.cast()) };
    let focused = copy_attribute(as_ax_element(&system), AX_FOCUSED_UI_ELEMENT, deadline)?;
    let focused = as_ax_element(&focused);
    let selected_range = copy_attribute(focused, AX_SELECTED_TEXT_RANGE, deadline)?;
    let range: CFRange = decode_ax_value(
        &selected_range,
        AX_VALUE_CF_RANGE,
        CFRange {
            location: 0,
            length: 0,
        },
    )?;
    if range.location < 0 || range.length <= 0 || expired(deadline) {
        return None;
    }
    let range_value = unsafe {
        AXValueCreate(
            AX_VALUE_CF_RANGE,
            (&range as *const CFRange).cast::<c_void>(),
        )
    };
    if range_value.is_null() {
        return None;
    }
    let range_value = unsafe { CFType::wrap_under_create_rule(range_value) };
    let bounds =
        copy_parameterized_attribute(focused, AX_BOUNDS_FOR_RANGE, &range_value, deadline)?;
    let rect = decode_ax_value(
        &bounds,
        AX_VALUE_CG_RECT,
        CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(0.0, 0.0)),
    )?;
    let bounds = WindowBounds {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    };
    (valid_bounds(&bounds) && !expired(deadline)).then_some(bounds)
}

fn valid_bounds(bounds: &WindowBounds) -> bool {
    bounds.x.is_finite()
        && bounds.y.is_finite()
        && bounds.width.is_finite()
        && bounds.height.is_finite()
        && bounds.width > 0.0
        && bounds.height > 0.0
}

fn as_ax_element(value: &CFType) -> AXUIElementRef {
    value.as_CFTypeRef().cast()
}

fn decode_ax_value<T>(value: &CFType, value_type: AXValueType, mut output: T) -> Option<T> {
    let raw = value.as_CFTypeRef();
    if unsafe { AXValueGetType(raw) } != value_type {
        return None;
    }
    let decoded =
        unsafe { AXValueGetValue(raw, value_type, (&mut output as *mut T).cast::<c_void>()) };
    (decoded != 0).then_some(output)
}

fn copy_attribute(element: AXUIElementRef, attribute: &str, deadline: Instant) -> Option<CFType> {
    set_messaging_timeout(element, deadline)?;
    let attribute = CFString::new(attribute);
    let mut value: CFTypeRef = ptr::null();
    let result = unsafe {
        AXUIElementCopyAttributeValue(
            element,
            attribute.as_concrete_TypeRef(),
            &mut value as *mut CFTypeRef,
        )
    };
    wrapped_value(value, result, deadline)
}

fn copy_parameterized_attribute(
    element: AXUIElementRef,
    attribute: &str,
    parameter: &CFType,
    deadline: Instant,
) -> Option<CFType> {
    set_messaging_timeout(element, deadline)?;
    let attribute = CFString::new(attribute);
    let mut value: CFTypeRef = ptr::null();
    let result = unsafe {
        AXUIElementCopyParameterizedAttributeValue(
            element,
            attribute.as_concrete_TypeRef(),
            parameter.as_CFTypeRef(),
            &mut value as *mut CFTypeRef,
        )
    };
    wrapped_value(value, result, deadline)
}

fn wrapped_value(value: CFTypeRef, result: AXError, deadline: Instant) -> Option<CFType> {
    if result != AX_SUCCESS || value.is_null() || expired(deadline) {
        if !value.is_null() {
            drop(unsafe { CFType::wrap_under_create_rule(value) });
        }
        return None;
    }
    Some(unsafe { CFType::wrap_under_create_rule(value) })
}

fn set_messaging_timeout(element: AXUIElementRef, deadline: Instant) -> Option<()> {
    let remaining = remaining(deadline)?;
    let result = unsafe { AXUIElementSetMessagingTimeout(element, remaining.as_secs_f32()) };
    (result == AX_SUCCESS && !expired(deadline)).then_some(())
}

fn remaining(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
}

fn expired(deadline: Instant) -> bool {
    remaining(deadline).is_none()
}
