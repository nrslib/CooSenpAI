use async_trait::async_trait;
use coosenpai_core::ports::{
    ClipboardReader, ClipboardWriter, PortError, SelectedTextCopyOutcome, SelectedTextCopyPort,
    SELECTED_TEXT_POLL_INTERVAL, SELECTED_TEXT_POLL_TIMEOUT,
};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_core_graphics::{
    CGEventFlags as ObjcCGEventFlags, CGEventSource as ObjcCGEventSource,
    CGEventSourceStateID as ObjcCGEventSourceStateID, CGPreflightPostEventAccess,
    CGRequestPostEventAccess,
};
use objc2_foundation::NSString;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const KEY_CODE_C: u16 = 8;

#[derive(Debug, Default, Clone, Copy)]
pub struct MacClipboardReader;

impl ClipboardReader for MacClipboardReader {
    fn read_text(&self) -> Result<Option<String>, PortError> {
        let pasteboard = NSPasteboard::generalPasteboard();
        // SAFETY: AppKit がプロセス寿命中保持する immutable な定数を読み取るだけである。
        let text_type = unsafe { NSPasteboardTypeString };
        Ok(pasteboard
            .stringForType(text_type)
            .map(|value| value.to_string()))
    }

    fn change_count(&self) -> Result<i64, PortError> {
        let pasteboard = NSPasteboard::generalPasteboard();
        Ok(pasteboard.changeCount() as i64)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MacClipboardWriter;

impl ClipboardWriter for MacClipboardWriter {
    fn write_text(&self, text: &str) -> Result<(), PortError> {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        // SAFETY: AppKit がプロセス寿命中保持する immutable な定数を読み取るだけである。
        let text_type = unsafe { NSPasteboardTypeString };
        if pasteboard.setString_forType(&NSString::from_str(text), text_type) {
            Ok(())
        } else {
            Err(PortError::Unavailable(
                "クリップボードへ文章を書き込めません".to_owned(),
            ))
        }
    }

    fn clear(&self) -> Result<(), PortError> {
        let pasteboard = NSPasteboard::generalPasteboard();
        if pasteboard.clearContents() != 0 {
            Ok(())
        } else {
            Err(PortError::Unavailable(
                "クリップボードを空にできません".to_owned(),
            ))
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MacSelectedTextCopier;

trait SelectedTextCopySystem: Send + Sync {
    type PreparedCopy;

    fn event_access_allowed(&self) -> bool;
    fn shortcut_input_is_pressed(&self) -> bool;
    fn prepare_copy(&self) -> Result<Self::PreparedCopy, PortError>;
    fn clipboard_change_count(&self) -> i64;
    fn post_copy(&self, prepared: Self::PreparedCopy);
}

#[async_trait]
trait SelectedTextCopyClock: Send + Sync {
    fn now(&self) -> tokio::time::Instant;
    async fn sleep(&self, duration: Duration);
}

struct MacSelectedTextCopySystem;

struct MacPreparedCopy {
    key_down: CGEvent,
    key_up: CGEvent,
}

impl SelectedTextCopySystem for MacSelectedTextCopySystem {
    type PreparedCopy = MacPreparedCopy;

    fn event_access_allowed(&self) -> bool {
        CGPreflightPostEventAccess() || CGRequestPostEventAccess()
    }

    fn shortcut_input_is_pressed(&self) -> bool {
        let flags = ObjcCGEventSource::flags_state(ObjcCGEventSourceStateID::CombinedSessionState);
        let modifier_pressed = [
            ObjcCGEventFlags::MaskCommand,
            ObjcCGEventFlags::MaskAlternate,
            ObjcCGEventFlags::MaskShift,
            ObjcCGEventFlags::MaskControl,
        ]
        .into_iter()
        .any(|modifier| flags.contains(modifier));
        modifier_pressed
            || (0_u16..=127).any(|key_code| {
                ObjcCGEventSource::key_state(
                    ObjcCGEventSourceStateID::CombinedSessionState,
                    key_code,
                )
            })
    }

    fn prepare_copy(&self) -> Result<Self::PreparedCopy, PortError> {
        let source = CGEventSource::new(CGEventSourceStateID::Private)
            .map_err(|_| PortError::Unavailable("コピー操作を作成できません".to_owned()))?;
        let key_down = CGEvent::new_keyboard_event(source.clone(), KEY_CODE_C, true)
            .map_err(|_| PortError::Unavailable("コピー操作を作成できません".to_owned()))?;
        let key_up = CGEvent::new_keyboard_event(source, KEY_CODE_C, false)
            .map_err(|_| PortError::Unavailable("コピー操作を作成できません".to_owned()))?;
        key_down.set_flags(CGEventFlags::CGEventFlagCommand);
        key_up.set_flags(CGEventFlags::CGEventFlagCommand);
        Ok(MacPreparedCopy { key_down, key_up })
    }

    fn clipboard_change_count(&self) -> i64 {
        NSPasteboard::generalPasteboard().changeCount() as i64
    }

    fn post_copy(&self, prepared: Self::PreparedCopy) {
        prepared.key_down.post(CGEventTapLocation::HID);
        prepared.key_up.post(CGEventTapLocation::HID);
    }
}

struct TokioSelectedTextCopyClock;

#[async_trait]
impl SelectedTextCopyClock for TokioSelectedTextCopyClock {
    fn now(&self) -> tokio::time::Instant {
        tokio::time::Instant::now()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

enum ShortcutReleaseOutcome {
    Released,
    Cancelled,
    TimedOut,
}

async fn synthesize_copy_with<S: SelectedTextCopySystem>(
    system: &S,
    clock: &dyn SelectedTextCopyClock,
    cancellation: &CancellationToken,
) -> Result<SelectedTextCopyOutcome, PortError> {
    if !system.event_access_allowed() {
        return Ok(SelectedTextCopyOutcome::PermissionDenied);
    }

    let deadline = clock.now() + SELECTED_TEXT_POLL_TIMEOUT;
    match wait_for_shortcut_release(system, clock, cancellation, deadline).await {
        ShortcutReleaseOutcome::Cancelled => return Ok(SelectedTextCopyOutcome::Cancelled),
        ShortcutReleaseOutcome::TimedOut => return Ok(SelectedTextCopyOutcome::ReleaseTimeout),
        ShortcutReleaseOutcome::Released => {}
    }
    if cancellation.is_cancelled() {
        return Ok(SelectedTextCopyOutcome::Cancelled);
    }
    if clock.now() >= deadline {
        return Ok(SelectedTextCopyOutcome::ReleaseTimeout);
    }
    let prepared = system.prepare_copy()?;
    let change_count_before_post = system.clipboard_change_count();
    if cancellation.is_cancelled() {
        return Ok(SelectedTextCopyOutcome::Cancelled);
    }
    if clock.now() >= deadline {
        return Ok(SelectedTextCopyOutcome::ReleaseTimeout);
    }
    system.post_copy(prepared);
    Ok(SelectedTextCopyOutcome::Sent {
        change_count_before_post,
    })
}

async fn wait_for_shortcut_release<S: SelectedTextCopySystem>(
    system: &S,
    clock: &dyn SelectedTextCopyClock,
    cancellation: &CancellationToken,
    deadline: tokio::time::Instant,
) -> ShortcutReleaseOutcome {
    loop {
        if cancellation.is_cancelled() {
            return ShortcutReleaseOutcome::Cancelled;
        }
        let now = clock.now();
        if now >= deadline {
            return ShortcutReleaseOutcome::TimedOut;
        }
        if !system.shortcut_input_is_pressed() {
            return ShortcutReleaseOutcome::Released;
        }
        let sleep_duration = SELECTED_TEXT_POLL_INTERVAL.min(deadline - now);
        tokio::select! {
            () = cancellation.cancelled() => return ShortcutReleaseOutcome::Cancelled,
            () = clock.sleep(sleep_duration) => {}
        }
    }
}

#[async_trait]
impl SelectedTextCopyPort for MacSelectedTextCopier {
    async fn synthesize_copy(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<SelectedTextCopyOutcome, PortError> {
        synthesize_copy_with(
            &MacSelectedTextCopySystem,
            &TokioSelectedTextCopyClock,
            cancellation,
        )
        .await
    }
}

