use crate::command_guard::{CommandSource, DesktopCommand, GenerationStamp};
use crate::input_popup::{self, InputPopupKind, InputPopupStartAction};
use crate::snapshot::AppSnapshot;
use crate::state::DesktopState;
use coosenpai_core::attachments::BoundedTextAttachment;
use coosenpai_core::ports::{ForegroundApplication, InteractiveCapturePort, RuntimeLogger};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
const MAX_PREVIEW_BYTES: u64 = 32 * 1024 * 1024;
static NEXT_SHORTCUT_ERROR_ID: AtomicU64 = AtomicU64::new(1);
#[path = "capture_helpers.rs"]
mod helpers;
use helpers::{capture_origin, close_capture_popup, normalize_message, validate_captured_file};
#[path = "capture_text.rs"]
mod text;
use text::prepare_selected_text_attachment;
#[path = "capture_shortcuts.rs"]
mod shortcuts;
pub use shortcuts::{
    refresh_speech_cancel_shortcut, sync_shortcuts, ShortcutAction, ShortcutBindings,
    ShortcutCoordinator,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureKind {
    Image,
    Text,
}

#[derive(Default)]
pub(crate) enum CapturePopupState {
    #[default]
    Idle,
    Capturing {
        kind: CaptureKind,
        generation: GenerationStamp,
        cancellation: tokio_util::sync::CancellationToken,
    },
    Ready(Arc<ReadyCapture>),
    Sending {
        id: String,
        kind: CaptureKind,
    },
}
pub(crate) struct ReadyCapture {
    pub id: String,
    attachment: ReadyAttachment,
    origin: CaptureOrigin,
    accessibility_permission_required: bool,
}
impl CapturePopupState {
    pub(crate) fn kind(&self) -> Option<CaptureKind> {
        match self {
            Self::Idle => None,
            Self::Capturing { kind, .. } | Self::Sending { kind, .. } => Some(*kind),
            Self::Ready(ready) => Some(ready.attachment.kind()),
        }
    }

    pub(crate) fn can_focus(&self) -> bool {
        matches!(self, Self::Ready(_) | Self::Sending { .. })
    }

    fn cancel(&mut self) -> Result<Option<CaptureOrigin>, String> {
        match std::mem::replace(self, Self::Idle) {
            Self::Ready(ready) => Ok(Some(ready.origin.clone())),
            Self::Capturing { cancellation, .. } => {
                cancellation.cancel();
                Ok(None)
            }
            Self::Sending { id, kind } => {
                *self = Self::Sending { id, kind };
                Err("範囲選択を送信中です".to_owned())
            }
            Self::Idle => Ok(None),
        }
    }

    fn begin_send(&mut self, capture_id: &str) -> Option<Arc<ReadyCapture>> {
        let Self::Ready(ready) = self else {
            return None;
        };
        if ready.id != capture_id {
            return None;
        }
        let ready = ready.clone();
        *self = Self::Sending {
            id: capture_id.to_owned(),
            kind: ready.attachment.kind(),
        };
        Some(ready)
    }

    fn complete_send(&mut self, capture_id: &str, retry: Option<Arc<ReadyCapture>>) -> bool {
        if !matches!(self, Self::Sending { id, .. } if id == capture_id) {
            return false;
        }
        *self = retry.map_or(Self::Idle, Self::Ready);
        true
    }
}

enum ReadyAttachment {
    Image {
        path: PathBuf,
        _directory: tempfile::TempDir,
    },
    Text(Option<BoundedTextAttachment>),
}

impl ReadyAttachment {
    fn kind(&self) -> CaptureKind {
        match self {
            Self::Image { .. } => CaptureKind::Image,
            Self::Text(_) => CaptureKind::Text,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct CaptureOrigin {
    main_was_foreground: bool,
    frontmost_application: Option<ForegroundApplication>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturePopupSnapshot {
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

pub(super) fn begin(app: &AppHandle) {
    begin_with(app, PopupRequest::Image, CommandSource::GlobalShortcut);
}

pub(super) fn begin_text(app: &AppHandle) {
    begin_with(app, PopupRequest::Text, CommandSource::GlobalShortcut);
}

#[derive(Clone, Copy)]
enum PopupRequest {
    Image,
    Text,
}

impl PopupRequest {
    fn kind(self) -> CaptureKind {
        match self {
            Self::Image => CaptureKind::Image,
            Self::Text => CaptureKind::Text,
        }
    }

    fn input_popup_kind(self) -> InputPopupKind {
        match self.kind() {
            CaptureKind::Image => InputPopupKind::CaptureImage,
            CaptureKind::Text => InputPopupKind::CaptureText,
        }
    }
}

fn begin_with(app: &AppHandle, request: PopupRequest, source: CommandSource) {
    let Some(state) = app.try_state::<Arc<DesktopState>>() else {
        return;
    };
    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        let command = match request {
            PopupRequest::Image => DesktopCommand::CaptureStartImage,
            PopupRequest::Text => DesktopCommand::CaptureStartText,
        };
        let handler_state = state.clone();
        let result = state
            .dispatch(source, command, move |context| async move {
                let action = input_popup::start_action(
                    handler_state.input_popup_kind().await,
                    request.input_popup_kind(),
                    source,
                );
                if action == InputPopupStartAction::Focus {
                    focus_existing_popup(&handler_state).await;
                    return Ok(None);
                }
                if action == InputPopupStartAction::Cancel {
                    handler_state
                        .cancel_input_popup_for_switch(&context)
                        .await
                        .map_err(crate::command_guard::DispatchError::handler)?;
                    return Ok(None);
                }
                if action == InputPopupStartAction::CancelThenStart {
                    handler_state
                        .cancel_input_popup_for_switch(&context)
                        .await
                        .map_err(crate::command_guard::DispatchError::handler)?;
                }
                let Some(generation) =
                    context.fence(crate::command_guard::GenerationResource::Capture)
                else {
                    return Err(crate::command_guard::DispatchError::handler(
                        "範囲選択の世代を開始できません",
                    ));
                };
                let cancellation = handler_state.cancellation.child_token();
                let mut capture = handler_state.capture_popup_for_command(&context).await;
                if !matches!(*capture, CapturePopupState::Idle) {
                    return Err(crate::command_guard::DispatchError::handler(
                        "範囲選択を開始できません",
                    ));
                }
                *capture = CapturePopupState::Capturing {
                    kind: request.kind(),
                    generation,
                    cancellation: cancellation.clone(),
                };
                Ok(Some((generation, cancellation)))
            })
            .await;
        let Ok(Some((generation, cancellation))) = result else {
            return;
        };
        match request {
            PopupRequest::Image => run_capture(state, generation, cancellation).await,
            PopupRequest::Text => run_text_capture(state, generation, cancellation).await,
        }
    });
}

async fn run_capture(
    state: Arc<DesktopState>,
    generation: GenerationStamp,
    cancellation: tokio_util::sync::CancellationToken,
) {
    let origin = capture_origin(&state);
    let permission = state.request_screen_permission_for_watch().await;
    if permission.presentation().status != "granted" {
        reset_if_current(&state, generation).await;
        crate::windows::show_main(&state.app);
        return;
    }
    let directory = match tempfile::Builder::new()
        .prefix("coosenpai-selection-")
        .tempdir()
    {
        Ok(value) => value,
        Err(_) => {
            fail_if_current(
                &state,
                generation,
                "範囲選択の一時ファイルを作成できませんでした",
            )
            .await;
            return;
        }
    };
    let path = directory.path().join("capture.png");
    let output = crate::platform::MacInteractiveCapture
        .capture_interactive(&path, cancellation.clone())
        .await;
    if cancellation.is_cancelled() || state.cancellation.is_cancelled() {
        reset_if_current(&state, generation).await;
        return;
    }
    if !matches!(output, Ok(true)) {
        reset_if_current(&state, generation).await;
        return;
    }
    let validation_path = path.clone();
    let valid = tokio::task::spawn_blocking(move || validate_captured_file(&validation_path))
        .await
        .is_ok_and(|valid| valid);
    if !valid {
        fail_if_current(&state, generation, "選択した画像を読み込めませんでした").await;
        return;
    }
    let id = uuid::Uuid::new_v4().to_string();
    {
        let Ok(mut capture) = state.capture_popup_for_event(generation).await else {
            return;
        };
        if !matches!(*capture, CapturePopupState::Capturing { generation: current, .. } if current == generation)
        {
            return;
        }
        *capture = CapturePopupState::Ready(Arc::new(ReadyCapture {
            id: id.clone(),
            attachment: ReadyAttachment::Image {
                path,
                _directory: directory,
            },
            origin,
            accessibility_permission_required: false,
        }));
    }
    show_popup(&state, &id).await;
}

async fn run_text_capture(
    state: Arc<DesktopState>,
    generation: GenerationStamp,
    cancellation: tokio_util::sync::CancellationToken,
) {
    let _text_capture_guard = state.text_capture_guard().await;
    if cancellation.is_cancelled() {
        reset_if_current(&state, generation).await;
        return;
    }
    let previous_error = current_shortcut_error_token(&state).await;
    let origin = capture_origin(&state);
    let prepared = prepare_selected_text_attachment(
        state.selected_text_copier.as_ref(),
        state.clipboard_reader.as_ref(),
        state.logger.as_ref(),
        &cancellation,
    )
    .await;
    let prepared = match prepared {
        Ok(Some(prepared)) => prepared,
        Ok(None) => {
            reset_if_current(&state, generation).await;
            return;
        }
        Err(_) => {
            if cancellation.is_cancelled() || state.cancellation.is_cancelled() {
                reset_if_current(&state, generation).await;
                return;
            }
            reset_if_current(&state, generation).await;
            publish_transient_shortcut_error(
                state.clone(),
                "クリップボードを読み取れませんでした".to_owned(),
            )
            .await;
            return;
        }
    };
    let text::PreparedSelectedTextAttachment {
        attachment,
        accessibility_permission_required,
    } = prepared;
    if attachment.is_none() && !accessibility_permission_required {
        reset_if_current(&state, generation).await;
        if cancellation.is_cancelled() || state.cancellation.is_cancelled() {
            return;
        }
        crate::capture_notice::show_empty_clipboard(state.clone()).await;
        return;
    }
    let id = uuid::Uuid::new_v4().to_string();
    {
        let Ok(mut capture) = state.capture_popup_for_event(generation).await else {
            return;
        };
        if !matches!(*capture, CapturePopupState::Capturing { generation: current, .. } if current == generation)
        {
            return;
        }
        *capture = CapturePopupState::Ready(Arc::new(ReadyCapture {
            id: id.clone(),
            attachment: ReadyAttachment::Text(attachment),
            origin,
            accessibility_permission_required,
        }));
    }
    if cancellation.is_cancelled() || state.cancellation.is_cancelled() {
        return;
    }
    show_popup(&state, &id).await;
    if cancellation.is_cancelled() || state.cancellation.is_cancelled() {
        return;
    }
    clear_shortcut_error_if_current(&state, previous_error).await;
}

async fn show_popup(state: &DesktopState, id: &str) {
    if let Some(window) = state.app.get_webview_window("capture-popup") {
        match crate::windows::show_capture_popup(state, &window).await {
            Ok(true) => {}
            Ok(false) => {
                let details = crate::windows::focus_failure_details(&window);
                crate::windows::log_focus_failure(
                    state.logger.as_ref(),
                    "送信ポップアップ",
                    &details,
                );
            }
            Err(error) => {
                let _ = state.logger.write(
                    "WARN",
                    &format!("送信ポップアップを表示できませんでした: {error}"),
                );
            }
        }
        let _ = state
            .app
            .emit_to("capture-popup", "coosenpai:capture-popup:ready", id);
    }
}

async fn focus_existing_popup(state: &DesktopState) {
    let can_focus = state.capture_popup_read().await.can_focus();
    if !can_focus {
        return;
    }
    if let Some(window) = state.app.get_webview_window("capture-popup") {
        match crate::windows::focus_capture_popup(state, &window).await {
            Ok(true) => {}
            Ok(false) => {
                let details = crate::windows::focus_failure_details(&window);
                crate::windows::log_focus_failure(
                    state.logger.as_ref(),
                    "送信ポップアップ",
                    &details,
                );
            }
            Err(error) => {
                let _ = state.logger.write(
                    "WARN",
                    &format!("送信ポップアップを表示できませんでした: {error}"),
                );
            }
        }
    }
}

pub async fn snapshot(state: &DesktopState) -> Result<CapturePopupSnapshot, String> {
    let (capture_id, source, accessibility_permission_required) = {
        let capture = state.capture_popup_read().await;
        let CapturePopupState::Ready(ready) = &*capture else {
            return Err("送信する範囲選択がありません".to_owned());
        };
        let source = match &ready.attachment {
            ReadyAttachment::Image { path, .. } => SnapshotSource::Image(path.clone()),
            ReadyAttachment::Text(text) => SnapshotSource::Text(text.clone()),
        };
        (
            ready.id.clone(),
            source,
            ready.accessibility_permission_required,
        )
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

pub(super) async fn send(
    state: &DesktopState,
    context: &crate::command_guard::CommandContext,
    capture_id: &str,
    message: String,
) -> Result<String, String> {
    let ready = {
        let mut capture = state.capture_popup_for_command(context).await;
        capture
            .begin_send(capture_id)
            .ok_or_else(|| "送信する範囲選択が一致しません".to_owned())?
    };
    if matches!(&ready.attachment, ReadyAttachment::Text(None)) {
        complete_send(state, context, capture_id, Some(ready)).await;
        return Err("送信できる文章がありません".to_owned());
    }
    let message = normalize_message(message);
    let result = match &ready.attachment {
        ReadyAttachment::Image { path, .. } => {
            state
                .command_enqueue_user_message(
                    context,
                    message,
                    Vec::new(),
                    crate::state::user_input::UserMessageAttachment::Image(path.clone()),
                )
                .await
        }
        ReadyAttachment::Text(Some(text)) => {
            state
                .command_enqueue_user_message(
                    context,
                    message,
                    Vec::new(),
                    crate::state::user_input::UserMessageAttachment::Text(text.text.clone()),
                )
                .await
        }
        ReadyAttachment::Text(None) => unreachable!("空のテキスト添付は送信前に拒否されます"),
    };
    match result {
        Ok(id) => {
            let completed = complete_send(state, context, capture_id, None).await;
            if completed {
                close_capture_popup(state, &ready.origin);
            }
            state.refresh_conversation().await;
            Ok(id)
        }
        Err(error) => {
            complete_send(state, context, capture_id, Some(ready)).await;
            Err(error)
        }
    }
}

async fn complete_send(
    state: &DesktopState,
    context: &crate::command_guard::CommandContext,
    capture_id: &str,
    retry: Option<Arc<ReadyCapture>>,
) -> bool {
    let mut capture = state.capture_popup_for_command(context).await;
    capture.complete_send(capture_id, retry)
}

pub(super) async fn cancel(state: &DesktopState, permit: &crate::command_guard::CommandContext) {
    let _ = cancel_for_switch(state, permit).await;
}

pub(crate) async fn cancel_for_switch(
    state: &DesktopState,
    permit: &crate::command_guard::CommandContext,
) -> Result<(), String> {
    let origin = {
        let mut capture = state.capture_popup_for_command(permit).await;
        capture.cancel()?
    };
    if let Some(origin) = origin {
        close_capture_popup(state, &origin);
    }
    drop(state.text_capture_guard().await);
    Ok(())
}

trait ShortcutRegistrar {
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

async fn publish_shortcut_error(state: &DesktopState, error: String) -> AppSnapshot {
    let error_id = next_shortcut_error_id();
    state
        .publish(|snapshot| {
            set_shortcut_error(snapshot, error_id, Some(error), None);
        })
        .await
}

async fn publish_speech_shortcut_error(
    state: &DesktopState,
    lifecycle_revision: u64,
    error: String,
) -> AppSnapshot {
    let error_id = next_shortcut_error_id();
    state
        .publish(|snapshot| {
            if state
                .shortcut_coordinator
                .accepts_speech_revision(lifecycle_revision)
            {
                set_shortcut_error(snapshot, error_id, Some(error), None);
            }
        })
        .await
}

async fn clear_shortcut_error_if_current(state: &DesktopState, expected: ShortcutErrorToken) {
    state
        .publish(|snapshot| {
            if can_clear_shortcut_error(snapshot, expected) {
                set_shortcut_error(snapshot, next_shortcut_error_id(), None, None);
            }
        })
        .await;
}

async fn clear_speech_shortcut_error_if_current(
    state: &DesktopState,
    lifecycle_revision: u64,
    expected: ShortcutErrorToken,
) {
    state
        .publish(|snapshot| {
            if state
                .shortcut_coordinator
                .accepts_speech_revision(lifecycle_revision)
                && can_clear_shortcut_error(snapshot, expected)
            {
                set_shortcut_error(snapshot, next_shortcut_error_id(), None, None);
            }
        })
        .await;
}

pub(crate) async fn publish_transient_shortcut_error(state: Arc<DesktopState>, message: String) {
    let error_id = next_shortcut_error_id();
    state
        .publish(|snapshot| {
            set_shortcut_error(snapshot, error_id, Some(message.clone()), None);
        })
        .await;
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        clear_transient_shortcut_error(&state, error_id, None, &message).await;
    });
}

pub(crate) async fn publish_speech_transient_shortcut_error(
    state: Arc<DesktopState>,
    generation: u64,
    message: String,
) {
    let error_id = next_shortcut_error_id();
    let snapshot = state
        .publish(|snapshot| {
            if state.speech_accepts_transient_shortcut_error(generation)
                && snapshot.speech.generation == generation
            {
                set_shortcut_error(snapshot, error_id, Some(message.clone()), Some(generation));
            }
        })
        .await;
    if snapshot.capture_shortcut_error_id != error_id {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        clear_transient_shortcut_error(&state, error_id, Some(generation), &message).await;
    });
}

fn next_shortcut_error_id() -> u64 {
    NEXT_SHORTCUT_ERROR_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ShortcutErrorToken {
    id: u64,
    speech_generation: Option<u64>,
}

fn shortcut_error_token(snapshot: &AppSnapshot) -> ShortcutErrorToken {
    ShortcutErrorToken {
        id: snapshot.capture_shortcut_error_id,
        speech_generation: snapshot.capture_shortcut_error_speech_generation,
    }
}

fn can_clear_shortcut_error(snapshot: &AppSnapshot, expected: ShortcutErrorToken) -> bool {
    can_clear_shortcut_error_token(
        shortcut_error_token(snapshot),
        snapshot.capture_shortcut_error.is_some(),
        expected,
    )
}

fn can_clear_shortcut_error_token(
    current: ShortcutErrorToken,
    has_error: bool,
    expected: ShortcutErrorToken,
) -> bool {
    has_error && current == expected
}

async fn current_shortcut_error_token(state: &DesktopState) -> ShortcutErrorToken {
    shortcut_error_token(&state.snapshot().await)
}

fn set_shortcut_error(
    snapshot: &mut AppSnapshot,
    error_id: u64,
    error: Option<String>,
    speech_generation: Option<u64>,
) {
    snapshot.capture_shortcut_error_id = error_id;
    snapshot.capture_shortcut_error = error;
    snapshot.capture_shortcut_error_speech_generation = speech_generation;
}

async fn clear_transient_shortcut_error(
    state: &DesktopState,
    error_id: u64,
    speech_generation: Option<u64>,
    message: &str,
) {
    state
        .publish(|snapshot| {
            if is_current_shortcut_error(
                snapshot.capture_shortcut_error_id,
                error_id,
                snapshot.capture_shortcut_error_speech_generation,
                speech_generation,
                snapshot.capture_shortcut_error.as_deref(),
                message,
            ) {
                set_shortcut_error(snapshot, next_shortcut_error_id(), None, None);
            }
        })
        .await;
}

fn is_current_shortcut_error(
    current_id: u64,
    expected_id: u64,
    current_generation: Option<u64>,
    expected_generation: Option<u64>,
    current_message: Option<&str>,
    expected_message: &str,
) -> bool {
    current_id == expected_id
        && current_generation == expected_generation
        && current_message == Some(expected_message)
}

async fn fail_if_current(state: &DesktopState, generation: GenerationStamp, message: &str) {
    if reset_if_current(state, generation).await {
        if state.ensure_command_generation(generation).is_ok() {
            publish_shortcut_error(state, message.to_owned()).await;
        }
        if state.ensure_command_generation(generation).is_ok() {
            crate::windows::show_main(&state.app);
        }
    }
}

async fn reset_if_current(state: &DesktopState, generation: GenerationStamp) -> bool {
    let Ok(mut capture) = state.capture_popup_for_event(generation).await else {
        return false;
    };
    if matches!(*capture, CapturePopupState::Capturing { generation: current, .. } if current == generation)
    {
        *capture = CapturePopupState::Idle;
        true
    } else {
        false
    }
}

