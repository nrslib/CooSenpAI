use crate::state::DesktopState;
use coosenpai_core::attachments::BoundedTextAttachment;
use serde::Serialize;
use std::path::PathBuf;

const MAX_PREVIEW_BYTES: u64 = 32 * 1024 * 1024;
#[path = "capture_effects.rs"]
pub(crate) mod effects;
#[path = "capture_helpers.rs"]
mod helpers;
#[path = "capture_manager.rs"]
pub(crate) mod manager;
#[path = "capture_port.rs"]
pub(crate) mod port;
#[path = "capture_region.rs"]
mod region;
#[path = "capture_window.rs"]
pub(crate) mod window;
pub(crate) use manager::PopupKeyInput;
pub(crate) use manager::{CancelSource, CaptureEvent, CaptureHandle, CapturePhase};
#[path = "capture_shortcut_errors.rs"]
mod shortcut_errors;
use shortcut_errors::{
    clear_shortcut_error_if_current, clear_speech_shortcut_error_if_current,
    current_shortcut_error_token, publish_shortcut_error, publish_speech_shortcut_error,
};
pub(crate) use shortcut_errors::{
    publish_speech_transient_shortcut_error, publish_transient_shortcut_error,
};
pub(crate) use shortcut_errors::{ShortcutErrorEvent, ShortcutErrorPresenter};
#[path = "capture_shortcuts.rs"]
pub(crate) mod shortcuts;
pub use shortcuts::{
    refresh_speech_cancel_shortcut, sync_shortcuts, ShortcutAction, ShortcutBindings,
    ShortcutCoordinator,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureKind {
    Image,
    Text,
    Voice,
}

#[derive(Debug)]
pub(crate) struct ReadyCapture {
    pub(crate) conversation: crate::command_guard::GenerationStamp,
    pub id: String,
    pub(crate) attachment: ReadyAttachment,
    origin: CaptureOrigin,
    accessibility_permission_required: bool,
}

#[derive(Debug)]
pub(crate) enum ReadyAttachment {
    Image {
        path: PathBuf,
        _directory: tempfile::TempDir,
    },
    Text(Option<BoundedTextAttachment>),
}

#[cfg(test)]
impl ReadyAttachment {
    fn kind(&self) -> CaptureKind {
        match self {
            Self::Image { .. } => CaptureKind::Image,
            Self::Text(_) => CaptureKind::Text,
        }
    }
}

pub(crate) use crate::activation_policy::CaptureOrigin;

pub(crate) use manager::{channel, CapturePresenter};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturePopupSnapshot {
    pub language: String,
    pub generation: u64,
    pub revision: u64,
    pub capture_id: String,
    pub attachment_kind: &'static str,
    pub accessibility_permission_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub png: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_preview: Option<String>,
    pub text_preview_truncated: bool,
    pub text_truncated: bool,
    pub text_truncated_characters: usize,
    pub quick_actions: Vec<coosenpai_core::config::PopupQuickAction>,
    pub send_key: String,
    pub companion_display_name: String,
    pub theme: String,
    pub font: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_image_png: Option<Vec<u8>>,
}

pub async fn snapshot(state: &DesktopState) -> Result<CapturePopupSnapshot, String> {
    let view = state.capture.view();
    let ready = view
        .content
        .ok_or_else(|| "送信する範囲選択がありません".to_owned())?;
    snapshot_content(state, view.generation, &ready).await
}

pub(super) async fn snapshot_content(
    state: &DesktopState,
    generation: u64,
    ready: &ReadyCapture,
) -> Result<CapturePopupSnapshot, String> {
    let capture_id = ready.id.clone();
    let accessibility_permission_required = ready.accessibility_permission_required;
    let source = match &ready.attachment {
        ReadyAttachment::Image { path, .. } => SnapshotSource::Image(path.clone()),
        ReadyAttachment::Text(text) => SnapshotSource::Text(text.clone()),
    };
    let (
        attachment_kind,
        png,
        text_preview,
        text_preview_truncated,
        text_truncated,
        text_truncated_characters,
    ) = match source {
        SnapshotSource::Image(path) => {
            let png = tokio::task::spawn_blocking(move || std::fs::read(path))
                .await
                .map_err(|_| "選択画像を読み込めません".to_owned())?
                .map_err(|_| "選択画像を読み込めません".to_owned())?;
            ("image", Some(png), None, false, false, 0)
        }
        SnapshotSource::Text(text) => (
            "text",
            None,
            text.as_ref()
                .map(|text| text.text.chars().take(2_000).collect()),
            text.as_ref()
                .is_some_and(|text| text.text.chars().count() > 2_000),
            text.as_ref().is_some_and(|text| text.truncated),
            text.as_ref().map_or(0, |text| text.truncated_characters),
        ),
    };
    let runtime = state.runtime_snapshot();
    let config = state.runtime_config();
    let avatar_image_png = crate::avatar::load(&state.paths, config.ui.avatar_path.as_deref());
    let quick_actions = if attachment_kind == "text" {
        config.popup.quick_actions.text
    } else {
        config.popup.quick_actions.image
    };
    Ok(CapturePopupSnapshot {
        language: config.ui.language,
        generation,
        revision: runtime.revision,
        capture_id,
        attachment_kind,
        accessibility_permission_required,
        png,
        text_preview,
        text_preview_truncated,
        text_truncated,
        text_truncated_characters,
        quick_actions,
        send_key: config.keymap.send_key,
        companion_display_name: runtime.companion_display_name,
        theme: config.ui.theme,
        font: config.ui.font,
        avatar_color: config.ui.avatar_color,
        avatar_image_png,
    })
}

enum SnapshotSource {
    Image(PathBuf),
    Text(Option<BoundedTextAttachment>),
}

pub(crate) trait ShortcutRegistrar {
    fn register(&self, shortcut: &str) -> Result<(), ()>;
    fn unregister(&self, shortcut: &str);
}

struct ShortcutReplacement {
    active: ShortcutBindings,
    error: Option<String>,
    accepted: bool,
}

fn replace_shortcuts(
    registrar: &dyn ShortcutRegistrar,
    previous: &ShortcutBindings,
    next: &ShortcutBindings,
) -> ShortcutReplacement {
    if previous == next {
        return ShortcutReplacement {
            active: previous.clone(),
            error: None,
            accepted: true,
        };
    }
    for (shortcut, _) in previous.entries() {
        registrar.unregister(shortcut);
    }
    let mut active = Vec::new();
    for (shortcut, action) in next.entries() {
        if registrar.register(shortcut).is_ok() {
            active.push((shortcut.clone(), *action));
        } else {
            for (value, _) in &active {
                registrar.unregister(value);
            }
            active.clear();
            let mut restore_failures = Vec::new();
            for (value, previous_action) in previous.entries() {
                if registrar.register(value).is_ok() {
                    active.push((value.clone(), *previous_action));
                } else {
                    restore_failures.push(value.as_str());
                }
            }
            let message = if restore_failures.is_empty() {
                format!("ショートカット {shortcut} は登録できません。別のキーを設定してください。")
            } else {
                format!(
                    "{shortcut} の登録に失敗し、以前の {} も復元できませんでした",
                    restore_failures.join(", ")
                )
            };
            return ShortcutReplacement {
                active: ShortcutBindings(active),
                error: Some(message),
                accepted: true,
            };
        }
    }
    ShortcutReplacement {
        active: ShortcutBindings(active),
        error: None,
        accepted: true,
    }
}

#[cfg(test)]
#[path = "../tests/support/capture_popup_ready.rs"]
pub(crate) mod ready_test_support;
