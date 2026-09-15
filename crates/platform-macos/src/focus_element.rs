use async_trait::async_trait;
use coosenpai_core::ports::{
    FocusElement, FocusElementPort, PortError, FOCUS_ELEMENT_TIMEOUT, SECURE_TEXT_FIELD_ROLE,
};
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use objc2_app_kit::NSWorkspace;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

type AXUIElementRef = *const c_void;
type AXError = i32;
type Pid = i32;

const AX_SUCCESS: AXError = 0;
const AX_ROLE: &str = "AXRole";
const AX_SUBROLE: &str = "AXSubrole";
const AX_TITLE: &str = "AXTitle";
const AX_DESCRIPTION: &str = "AXDescription";
const AX_VALUE: &str = "AXValue";
const AX_WINDOW: &str = "AXWindow";
const AX_FOCUSED_UI_ELEMENT: &str = "AXFocusedUIElement";

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateApplication(pid: Pid) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> AXError;
}

type FocusReadResult = Result<Option<FocusElement>, PortError>;
type FocusReadReceiver = tokio::sync::oneshot::Receiver<FocusReadResult>;

fn focus_thread_active() -> Arc<AtomicBool> {
    static ACTIVE: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    ACTIVE
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

struct FocusThreadGuard {
    active: Arc<AtomicBool>,
}

impl Drop for FocusThreadGuard {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

fn spawn_focus_thread<F>(
    active: Arc<AtomicBool>,
    operation: F,
) -> Result<Option<FocusReadReceiver>, PortError>
where
    F: FnOnce() -> FocusReadResult + Send + 'static,
{
    if active
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(None);
    }

    let (sender, receiver) = tokio::sync::oneshot::channel();
    let guard = FocusThreadGuard {
        active: active.clone(),
    };
    let spawned = std::thread::Builder::new()
        .name("coosenpai-focus-ax".to_owned())
        .spawn(move || {
            let _guard = guard;
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
                .unwrap_or_else(|_| {
                    Err(PortError::Unavailable(
                        "Accessibility 取得 thread が異常終了しました".to_owned(),
                    ))
                });
            let _ = sender.send(result);
        });
    if let Err(error) = spawned {
        active.store(false, Ordering::Release);
        return Err(PortError::Unavailable(format!(
            "Accessibility 取得 thread の起動に失敗しました: {error}"
        )));
    }
    // JoinHandle は保持しない。呼び出し側の期限後も Tokio runtime と
    // blocking pool に依存せず、AX 側の短い期限で自然終了させる。
    Ok(Some(receiver))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MacFocusedElement;

#[async_trait]
impl FocusElementPort for MacFocusedElement {
    async fn read_focused_element(
        &self,
        target_bundle_id: Option<&str>,
    ) -> Result<Option<FocusElement>, PortError> {
        let target_bundle_id = target_bundle_id.map(str::to_owned);
        let deadline = Instant::now() + FOCUS_ELEMENT_TIMEOUT;
        let Some(receiver) = spawn_focus_thread(focus_thread_active(), move || {
            read_focused_element_blocking(target_bundle_id.as_deref(), deadline)
        })?
        else {
            return Ok(None);
        };
        let remaining = ensure_time_remaining(deadline)?;
        tokio::time::timeout(remaining, receiver)
            .await
            .map_err(|_| PortError::Timeout)?
            .map_err(|error| {
                PortError::Unavailable(format!(
                    "Accessibility 取得 thread から結果を受信できませんでした: {error}"
                ))
            })?
    }
}

fn read_focused_element_blocking(
    target_bundle_id: Option<&str>,
    deadline: Instant,
) -> Result<Option<FocusElement>, PortError> {
    ensure_time_remaining(deadline)?;
    // 設定画面からの明示的な許可変更だけを使い、ここでは prompt option を渡さない。
    let trusted = unsafe { AXIsProcessTrusted() != 0 };
    ensure_time_remaining(deadline)?;
    if !trusted {
        return Ok(None);
    }
    ensure_time_remaining(deadline)?;
    let Some((pid, bundle_id)) = target_application(target_bundle_id, deadline)? else {
        return Ok(None);
    };
    ensure_time_remaining(deadline)?;
    let application_raw = unsafe { AXUIElementCreateApplication(pid) };
    if application_raw.is_null() {
        ensure_time_remaining(deadline)?;
        return Ok(None);
    }
    // AXUIElementCreateApplication は create rule の所有権を返す。
    let application = unsafe { CFType::wrap_under_create_rule(application_raw.cast()) };
    ensure_time_remaining(deadline)?;
    let Some(focused) =
        copy_attribute(as_ax_element(&application), AX_FOCUSED_UI_ELEMENT, deadline)?
    else {
        return Ok(None);
    };
    let focused_element = as_ax_element(&focused);
    let Some(role) = string_attribute(focused_element, AX_ROLE, deadline)? else {
        return Ok(None);
    };
    let subrole = string_attribute(focused_element, AX_SUBROLE, deadline)?;
    let secure =
        role == SECURE_TEXT_FIELD_ROLE || subrole.as_deref() == Some(SECURE_TEXT_FIELD_ROLE);
    let window_title = match copy_attribute(focused_element, AX_WINDOW, deadline)? {
        Some(window) => string_attribute(as_ax_element(&window), AX_TITLE, deadline)?,
        None => None,
    };
    let title = string_attribute(focused_element, AX_TITLE, deadline)?;
    let description = string_attribute(focused_element, AX_DESCRIPTION, deadline)?;
    let value = if secure {
        None
    } else {
        string_attribute(focused_element, AX_VALUE, deadline)?
    };

    Ok(Some(
        FocusElement {
            bundle_id,
            window_title,
            role,
            title,
            description,
            value,
        }
        .bounded(),
    ))
}

fn target_application(
    target_bundle_id: Option<&str>,
    deadline: Instant,
) -> Result<Option<(Pid, String)>, PortError> {
    ensure_time_remaining(deadline)?;
    let workspace = NSWorkspace::sharedWorkspace();
    ensure_time_remaining(deadline)?;
    match target_bundle_id {
        Some(target_bundle_id) => {
            for application in workspace.runningApplications().iter() {
                ensure_time_remaining(deadline)?;
                let bundle_id = application.bundleIdentifier();
                ensure_time_remaining(deadline)?;
                let Some(bundle_id) = bundle_id else {
                    continue;
                };
                let bundle_id = bundle_id.to_string();
                ensure_time_remaining(deadline)?;
                if bundle_id != target_bundle_id {
                    continue;
                }
                let terminated = application.isTerminated();
                ensure_time_remaining(deadline)?;
                if terminated {
                    continue;
                }
                let pid = application.processIdentifier();
                ensure_time_remaining(deadline)?;
                return Ok(Some((pid, bundle_id)));
            }
            Ok(None)
        }
        None => {
            let application = workspace.frontmostApplication();
            ensure_time_remaining(deadline)?;
            let Some(application) = application else {
                return Ok(None);
            };
            let bundle_id = application.bundleIdentifier();
            ensure_time_remaining(deadline)?;
            let Some(bundle_id) = bundle_id else {
                return Ok(None);
            };
            let bundle_id = bundle_id.to_string();
            ensure_time_remaining(deadline)?;
            if application.isTerminated() {
                ensure_time_remaining(deadline)?;
                return Ok(None);
            }
            ensure_time_remaining(deadline)?;
            let pid = application.processIdentifier();
            ensure_time_remaining(deadline)?;
            Ok(Some((pid, bundle_id)))
        }
    }
}

fn as_ax_element(value: &CFType) -> AXUIElementRef {
    value.as_CFTypeRef().cast()
}

fn ensure_time_remaining(deadline: Instant) -> Result<Duration, PortError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(PortError::Timeout)
}

fn copy_attribute(
    element: AXUIElementRef,
    attribute: &str,
    deadline: Instant,
) -> Result<Option<CFType>, PortError> {
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
    let value = if value.is_null() {
        None
    } else {
        Some(unsafe { CFType::wrap_under_create_rule(value) })
    };
    ensure_time_remaining(deadline)?;
    if result != AX_SUCCESS {
        return Ok(None);
    }
    Ok(value)
}

fn set_messaging_timeout(element: AXUIElementRef, deadline: Instant) -> Result<(), PortError> {
    let remaining = ensure_time_remaining(deadline)?;
    let result = unsafe { AXUIElementSetMessagingTimeout(element, remaining.as_secs_f32()) };
    if result != AX_SUCCESS {
        return Err(PortError::Unavailable(format!(
            "Accessibility の messaging timeout 設定に失敗しました: AXError={result}"
        )));
    }
    ensure_time_remaining(deadline).map(|_| ())
}

fn string_attribute(
    element: AXUIElementRef,
    attribute: &str,
    deadline: Instant,
) -> Result<Option<String>, PortError> {
    let Some(value) = copy_attribute(element, attribute, deadline)? else {
        return Ok(None);
    };
    let Some(value) = value.downcast::<CFString>() else {
        return Ok(None);
    };
    let value = value.to_string();
    ensure_time_remaining(deadline)?;
    Ok(Some(value))
}

