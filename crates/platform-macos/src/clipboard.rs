use async_trait::async_trait;
use coosenpai_core::ports::{ClipboardReader, ClipboardWriter, PortError, SelectedTextCopyPort};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_core_graphics::{CGPreflightPostEventAccess, CGRequestPostEventAccess};
use objc2_foundation::NSString;

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

fn synthesize_copy() -> Result<bool, PortError> {
    if !CGPreflightPostEventAccess() && !CGRequestPostEventAccess() {
        return Ok(false);
    }

    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| PortError::Unavailable("コピー操作を作成できません".to_owned()))?;
    let key_down = CGEvent::new_keyboard_event(source.clone(), KEY_CODE_C, true)
        .map_err(|_| PortError::Unavailable("コピー操作を作成できません".to_owned()))?;
    let key_up = CGEvent::new_keyboard_event(source, KEY_CODE_C, false)
        .map_err(|_| PortError::Unavailable("コピー操作を作成できません".to_owned()))?;
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);
    key_down.post(CGEventTapLocation::HID);
    key_up.post(CGEventTapLocation::HID);
    Ok(true)
}

#[async_trait]
impl SelectedTextCopyPort for MacSelectedTextCopier {
    async fn synthesize_copy(&self) -> Result<bool, PortError> {
        synthesize_copy()
    }
}
