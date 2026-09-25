use coosenpai_core::ports::RuntimeLogger;
use objc2::{rc::Retained, runtime::ProtocolObject, AnyThread};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardItem, NSRunningApplication,
};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use plist::Value;
use tokio_util::sync::CancellationToken;

const SCREENSHOT_DISABLED: &str =
    "システム設定でスクリーンショットの『選択部分をクリップボードにコピー』を有効にしてください";
pub const SCREENSHOT_ACCESS_REQUIRED: &str = "アクセシビリティの許可が必要です";
const SELECTION_BUNDLE_ID: &str = "com.apple.screencaptureui";

#[derive(Debug, PartialEq, Eq)]
pub struct ScreenshotShortcut {
    character: u16,
    key_code: u16,
    modifiers: u64,
}

impl Default for ScreenshotShortcut {
    fn default() -> Self {
        Self {
            character: 52,
            key_code: 21,
            modifiers: 0x160000,
        }
    }
}

fn parse_shortcut(value: &Value) -> Result<ScreenshotShortcut, String> {
    let invalid = || "スクリーンショットのショートカット設定が不正です".to_owned();
    let root = value.as_dictionary().ok_or_else(invalid)?;
    let Some(hotkeys) = root.get("AppleSymbolicHotKeys") else {
        return Ok(ScreenshotShortcut::default());
    };
    let hotkeys = hotkeys.as_dictionary().ok_or_else(invalid)?;
    let Some(entry) = hotkeys.get("31") else {
        return Ok(ScreenshotShortcut::default());
    };
    let entry = entry.as_dictionary().ok_or_else(invalid)?;
    match entry.get("enabled") {
        Some(Value::Boolean(false)) => return Err(SCREENSHOT_DISABLED.to_owned()),
        Some(Value::Integer(n)) if n.as_unsigned() == Some(0) => {
            return Err(SCREENSHOT_DISABLED.to_owned())
        }
        Some(Value::Boolean(true)) => {}
        Some(Value::Integer(n)) if n.as_unsigned() == Some(1) => {}
        _ => return Err(invalid()),
    }
    let parameters = entry
        .get("value")
        .and_then(Value::as_dictionary)
        .and_then(|v| v.get("parameters"))
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if parameters.len() != 3 {
        return Err(invalid());
    }
    Ok(ScreenshotShortcut {
        character: parameters[0]
            .as_unsigned_integer()
            .and_then(|n| n.try_into().ok())
            .ok_or_else(invalid)?,
        key_code: parameters[1]
            .as_unsigned_integer()
            .and_then(|n| n.try_into().ok())
            .ok_or_else(invalid)?,
        modifiers: parameters[2].as_unsigned_integer().ok_or_else(invalid)?,
    })
}

fn load_shortcut() -> Result<ScreenshotShortcut, String> {
    let home = std::env::var_os("HOME").ok_or("ホームディレクトリを取得できません")?;
    let path =
        std::path::PathBuf::from(home).join("Library/Preferences/com.apple.symbolichotkeys.plist");
    read_shortcut(&path)
}

fn read_shortcut(path: &std::path::Path) -> Result<ScreenshotShortcut, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ScreenshotShortcut::default())
        }
        Err(error) => return Err(format!("ショートカット設定を読み込めません: {error}")),
    };
    parse_shortcut(
        &Value::from_reader(file)
            .map_err(|e| format!("ショートカット設定を読み込めません: {e}"))?,
    )
}

pub type ClipboardContents = Vec<Vec<(String, Vec<u8>)>>;

pub trait ScreenshotSystem {
    fn shortcut(&self) -> Result<ScreenshotShortcut, String>;
    fn access_allowed(&self) -> bool;
    fn request_access(&self) -> bool;
    fn selection_pids(&self) -> Vec<i32>;
    fn escape(&self, pid: i32) -> Result<(), String>;
    fn change_count(&self) -> i64;
    fn snapshot(&self) -> Option<ClipboardContents>;
    fn post(&self, shortcut: &ScreenshotShortcut) -> Result<(), String>;
    fn image(&self) -> Result<Option<Vec<u8>>, String>;
    fn restore(
        &self,
        contents: ClipboardContents,
        expected_count: i64,
        cancellation: &CancellationToken,
    ) -> Result<bool, String>;
}

pub struct MacScreenshotSystem<'a> {
    pub logger: &'a dyn RuntimeLogger,
}

impl ScreenshotSystem for MacScreenshotSystem<'_> {
    fn shortcut(&self) -> Result<ScreenshotShortcut, String> {
        load_shortcut()
    }
    fn access_allowed(&self) -> bool {
        objc2_core_graphics::CGPreflightPostEventAccess()
    }
    fn request_access(&self) -> bool {
        objc2_core_graphics::CGRequestPostEventAccess()
    }
    fn selection_pids(&self) -> Vec<i32> {
        NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
            SELECTION_BUNDLE_ID,
        ))
        .iter()
        .filter(|app| !app.isTerminated())
        .map(|app| app.processIdentifier())
        .collect()
    }
    fn escape(&self, pid: i32) -> Result<(), String> {
        let [down, up] = escape_events()?;
        for event in [&down, &up] {
            if pid == 0 {
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event));
            } else {
                CGEvent::post_to_pid(pid, Some(event));
            }
        }
        Ok(())
    }
    fn change_count(&self) -> i64 {
        NSPasteboard::generalPasteboard().changeCount() as i64
    }
    fn snapshot(&self) -> Option<ClipboardContents> {
        let board = NSPasteboard::generalPasteboard();
        let Some(items) = board.pasteboardItems() else {
            return Some(Vec::new());
        };
        read_pasteboard_items(&items)
    }
    fn post(&self, shortcut: &ScreenshotShortcut) -> Result<(), String> {
        let [down, up] = shortcut_events(shortcut)?;
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&down));
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up));
        Ok(())
    }
    fn image(&self) -> Result<Option<Vec<u8>>, String> {
        image_from_pasteboard(&NSPasteboard::generalPasteboard(), self.logger)
    }
    fn restore(
        &self,
        contents: ClipboardContents,
        expected_count: i64,
        cancellation: &CancellationToken,
    ) -> Result<bool, String> {
        restore_pasteboard(
            &NSPasteboard::generalPasteboard(),
            contents,
            expected_count,
            cancellation,
        )
    }
}

fn image_from_pasteboard(
    board: &NSPasteboard,
    logger: &dyn RuntimeLogger,
) -> Result<Option<Vec<u8>>, String> {
    if let Some(data) = board.dataForType(&NSString::from_str("public.png")) {
        log_stage(logger, "image", &format!("type=PNG bytes={}", data.len()));
        return Ok(Some(data.to_vec()));
    }
    let Some(data) = board.dataForType(&NSString::from_str("public.tiff")) else {
        return Ok(None);
    };
    log_stage(logger, "image", &format!("type=TIFF bytes={}", data.len()));
    let png = tiff_to_png(&data)?;
    log_stage(
        logger,
        "image-converted",
        &format!("type=PNG bytes={}", png.len()),
    );
    Ok(Some(png))
}

fn restore_pasteboard(
    board: &NSPasteboard,
    contents: ClipboardContents,
    expected_count: i64,
    cancellation: &CancellationToken,
) -> Result<bool, String> {
    let items = create_pasteboard_items(contents)?
        .into_iter()
        .map(ProtocolObject::from_retained)
        .collect::<Vec<_>>();
    if cancellation.is_cancelled() || board.changeCount() as i64 != expected_count {
        return Ok(false);
    }
    board.clearContents();
    if !items.is_empty() && !board.writeObjects(&NSArray::from_retained_slice(&items)) {
        return Err("クリップボードを復元できません".to_owned());
    }
    Ok(true)
}

fn escape_events() -> Result<[CFRetained<CGEvent>; 2], String> {
    let error = || "Esc のキー入力を作成できません".to_owned();
    let down = CGEvent::new_keyboard_event(None, 53, true).ok_or_else(error)?;
    let up = CGEvent::new_keyboard_event(None, 53, false).ok_or_else(error)?;
    for event in [&down, &up] {
        CGEvent::set_flags(Some(event), CGEventFlags(0));
    }
    Ok([down, up])
}

fn shortcut_events(shortcut: &ScreenshotShortcut) -> Result<[CFRetained<CGEvent>; 2], String> {
    let error = || "スクリーンショットのキー入力を作成できません".to_owned();
    let source =
        CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok_or_else(error)?;
    let down =
        CGEvent::new_keyboard_event(Some(&source), shortcut.key_code, true).ok_or_else(error)?;
    let up =
        CGEvent::new_keyboard_event(Some(&source), shortcut.key_code, false).ok_or_else(error)?;
    for event in [&down, &up] {
        // OS の hotkey はキーコードと修飾キーで指定し、Unicode 文字列で上書きしない。
        CGEvent::set_flags(Some(event), CGEventFlags(shortcut.modifiers));
    }
    Ok([down, up])
}

fn log_stage(logger: &dyn RuntimeLogger, stage: &str, detail: &str) {
    let _ = logger.write("DEBUG", &format!("範囲選択: 段階={stage} {detail}"));
}

fn read_pasteboard_items(items: &NSArray<NSPasteboardItem>) -> Option<ClipboardContents> {
    items
        .iter()
        .map(|item| {
            item.types()
                .iter()
                .map(|kind| {
                    item.dataForType(&kind)
                        .map(|data| (kind.to_string(), data.to_vec()))
                })
                .collect()
        })
        .collect()
}

fn create_pasteboard_items(
    contents: ClipboardContents,
) -> Result<Vec<Retained<NSPasteboardItem>>, String> {
    let mut items = Vec::new();
    for representations in contents {
        let item = NSPasteboardItem::new();
        for (kind, bytes) in representations {
            if !item.setData_forType(&NSData::with_bytes(&bytes), &NSString::from_str(&kind)) {
                return Err("クリップボードの復元データを作成できません".to_owned());
            }
        }
        items.push(item);
    }
    Ok(items)
}

fn tiff_to_png(data: &NSData) -> Result<Vec<u8>, String> {
    let bitmap = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), data)
        .ok_or("TIFF画像を読み込めません")?;
    // SAFETY: PNG エンコードに空のプロパティ辞書を渡す。
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .ok_or("TIFF画像をPNGへ変換できません")?;
    Ok(png.to_vec())
}
