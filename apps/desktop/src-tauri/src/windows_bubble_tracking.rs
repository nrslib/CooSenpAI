use coosenpai_core::ports::RuntimeLogger;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker};
use objc2_app_kit::{NSEvent, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow};
use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize};
use tauri::{AppHandle, Manager};

struct TrackingIvars {
    app: AppHandle,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "CoosenpaiBubbleTrackingOwner"]
    #[ivars = TrackingIvars]
    struct TrackingOwner;

    impl TrackingOwner {
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            self.send_position(event);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.send_position(event);
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.emit(serde_json::Value::Null);
        }
    }
);

impl TrackingOwner {
    fn send_position(&self, event: &NSEvent) {
        let Some(window) = event
            .window(MainThreadMarker::new().expect("AppKit mouse event は main thread に届きます"))
        else {
            return;
        };
        let Some(view) = window.contentView() else {
            return;
        };
        let point = view.convertPoint_fromView(event.locationInWindow(), None);
        let bounds = view.bounds();
        let y = if view.isFlipped() {
            point.y - bounds.origin.y
        } else {
            bounds.size.height - (point.y - bounds.origin.y)
        };
        self.emit(serde_json::json!({ "x": point.x - bounds.origin.x, "y": y }));
    }

    fn emit(&self, position: serde_json::Value) {
        if let Err(error) = crate::webview_event::emit_to(
            &self.ivars().app,
            "bubble",
            "coosenpai:bubble:pointer-position",
            &position,
        ) {
            if let Some(state) = self
                .ivars()
                .app
                .try_state::<std::sync::Arc<crate::state::DesktopState>>()
            {
                let _ = state.logger.write(
                    "WARN",
                    &format!("吹き出しのカーソル位置を配送できません: {error}"),
                );
            }
        }
    }
}

pub(super) fn configure(window: &tauri::WebviewWindow) -> tauri::Result<()> {
    let native_window = window.ns_window()?;
    // SAFETY: Tauri returns the live NSWindow for this WebviewWindow.
    let native_window: &NSWindow = unsafe { &*native_window.cast() };
    native_window.setAcceptsMouseMovedEvents(true);
    let view = native_window
        .contentView()
        .ok_or(tauri::Error::WindowNotFound)?;

    let owner = unsafe {
        let owner = TrackingOwner::alloc().set_ivars(TrackingIvars {
            app: window.app_handle().clone(),
        });
        let owner: objc2::rc::Retained<TrackingOwner> = msg_send![super(owner), init];
        owner
    };
    let tracking = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
            NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect,
            Some(&owner),
            None,
        )
    };
    view.addTrackingArea(&tracking);
    // NSTrackingArea は owner を保持しないため、contentView と寿命を揃える。
    static OWNER_KEY: u8 = 0;
    unsafe {
        objc2::ffi::objc_setAssociatedObject(
            (&*view as *const NSView).cast_mut().cast(),
            (&OWNER_KEY as *const u8).cast(),
            (&*owner as *const TrackingOwner).cast_mut().cast(),
            objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
    }

    Ok(())
}
