use objc2::ffi;
use objc2::runtime::{AnyObject, Imp, ProtocolObject, Sel};
use objc2::{sel, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSApplicationTerminateReply};
use std::sync::{Arc, OnceLock};

type TerminationHandler = dyn Fn() -> bool + Send + Sync;
static TERMINATION_HANDLER: OnceLock<Arc<TerminationHandler>> = OnceLock::new();
static ORIGINAL_TERMINATION_IMPLEMENTATION: OnceLock<Imp> = OnceLock::new();

pub fn install_termination_handler(handler: Arc<TerminationHandler>) -> Result<(), String> {
    TERMINATION_HANDLER
        .set(handler)
        .map_err(|_| "AppleEvent 終了handlerは登録済みです".to_owned())?;
    let marker = MainThreadMarker::new()
        .ok_or_else(|| "AppleEvent 終了handlerはmain threadで登録してください".to_owned())?;
    let application = NSApplication::sharedApplication(marker);
    let delegate = application
        .delegate()
        .ok_or_else(|| "NSApplication delegateがありません".to_owned())?;
    let delegate_protocol: &ProtocolObject<dyn NSApplicationDelegate> = &delegate;
    let delegate_object = AsRef::<AnyObject>::as_ref(delegate_protocol);
    let class = delegate_object.class();
    let selector = sel!(applicationShouldTerminate:);
    let implementation = termination_implementation();
    let encoding = b"q@:@\0";
    // SAFETY: class is registered and the IMP and encoding exactly describe
    // applicationShouldTerminate:.
    let added = unsafe {
        ffi::class_addMethod(
            class as *const _ as *mut _,
            selector,
            implementation,
            encoding.as_ptr().cast(),
        )
    };
    if !added.as_bool() {
        let method = class
            .instance_method(selector)
            .ok_or_else(|| "applicationShouldTerminate:を取得できません".to_owned())?;
        // SAFETY: termination_callback has the Objective-C signature encoded above.
        let previous = unsafe { method.set_implementation(implementation) };
        ORIGINAL_TERMINATION_IMPLEMENTATION
            .set(previous)
            .map_err(|_| "applicationShouldTerminate:の既存実装は保存済みです".to_owned())?;
    }
    Ok(())
}

fn termination_implementation() -> Imp {
    // SAFETY: both pointer types use the C-unwind ABI and this IMP is installed only for the
    // selector whose arguments are encoded above.
    unsafe {
        std::mem::transmute::<
            unsafe extern "C-unwind" fn(
                &AnyObject,
                Sel,
                &NSApplication,
            ) -> NSApplicationTerminateReply,
            Imp,
        >(termination_callback)
    }
}

unsafe extern "C-unwind" fn termination_callback(
    delegate: &AnyObject,
    selector: Sel,
    application: &NSApplication,
) -> NSApplicationTerminateReply {
    let original_reply = if let Some(implementation) = ORIGINAL_TERMINATION_IMPLEMENTATION.get() {
        // SAFETY: the saved IMP is the previous implementation of the same selector.
        let implementation =
            unsafe { std::mem::transmute::<Imp, TerminationImplementation>(*implementation) };
        // SAFETY: these are the unchanged AppKit callback arguments.
        unsafe { implementation(delegate, selector, application) }
    } else {
        NSApplicationTerminateReply::TerminateNow
    };
    let defer = original_reply != NSApplicationTerminateReply::TerminateCancel
        && TERMINATION_HANDLER.get().is_some_and(|handler| handler());
    termination_reply(original_reply, defer)
}

fn termination_reply(
    original_reply: NSApplicationTerminateReply,
    defer: bool,
) -> NSApplicationTerminateReply {
    if defer || original_reply == NSApplicationTerminateReply::TerminateLater {
        NSApplicationTerminateReply::TerminateLater
    } else {
        original_reply
    }
}

type TerminationImplementation =
    unsafe extern "C-unwind" fn(&AnyObject, Sel, &NSApplication) -> NSApplicationTerminateReply;

pub fn reply_to_termination_request() {
    let Some(marker) = MainThreadMarker::new() else {
        return;
    };
    NSApplication::sharedApplication(marker).replyToApplicationShouldTerminate(true);
}

