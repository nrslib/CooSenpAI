use crate::commands::{authorize_window, CommandOrigin, IpcResult, TauriIpcResult};
use crate::state::DesktopState;
use crate::ui_commands::UserCommand;
use crate::ui_events::{UiEvent, UiView};
use chrono::Utc;
use coosenpai_core::debug::{DebugGateRecord, DebugStore};
use coosenpai_core::observer::ObservationFrameInput;
use coosenpai_core::state::ActivityTriggerKind;
use image::{ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use std::io::Cursor;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

const DEBUG_WAKE_TRIGGER: &str = "developer-debug-wake";
const MAX_CONTEXT_BYTES: usize = 32 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugWakePayload {
    image: Vec<u8>,
    #[serde(default)]
    context: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugWakeResult {
    pub source_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    pub status: String,
    pub emit: bool,
    pub message_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[tauri::command]
pub async fn debug_wake(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: DebugWakePayload,
) -> TauriIpcResult<DebugWakeResult> {
    authorize_window(&window, CommandOrigin::Main)?;
    state
        .ui
        .query(UiView::Settings, |reply| {
            UiEvent::UserCommand(UserCommand::DebugWake {
                image: payload.image,
                context: payload.context,
                reply,
            })
        })
        .await
}

pub(crate) async fn run(
    state: Arc<DesktopState>,
    image: Vec<u8>,
    context: String,
) -> Result<DebugWakeResult, String> {
    let config = state.runtime_config();
    if !config.debug.enabled {
        return Err("デバッグモードが有効でないため、観察 Wake は実行できません".to_owned());
    }
    let context = context.trim().to_owned();
    if context.len() > MAX_CONTEXT_BYTES {
        return Err("観察コンテキストが長すぎます".to_owned());
    }
    let normalized = tokio::task::spawn_blocking(move || normalize_image(&image))
        .await
        .map_err(|error| format!("画像処理を完了できません: {error}"))??;

    let frame_id = DebugStore::new_id();
    let captured_at = Utc::now();
    let debug_store = DebugStore::from_paths(&state.paths);
    debug_store
        .record_frame(&frame_id, captured_at, &normalized, None)
        .map_err(|error| format!("デバッグ画像を保存できません: {error}"))?;
    debug_store
        .record_gate(&DebugGateRecord {
            id: frame_id.clone(),
            created_at: captured_at.to_rfc3339(),
            trigger: DEBUG_WAKE_TRIGGER.to_owned(),
            sent: true,
            reason: "developer selected image".to_owned(),
            image_file: Some(format!("frame-{frame_id}.png")),
            ocr_preview: None,
        })
        .map_err(|error| format!("デバッグ判定を保存できません: {error}"))?;

    let directory = tempfile::Builder::new()
        .prefix("coosenpai-debug-wake-")
        .tempdir()
        .map_err(|error| format!("観察画像の一時領域を作成できません: {error}"))?;
    let image_path = directory.path().join("frame.png");
    tokio::fs::write(&image_path, &normalized)
        .await
        .map_err(|error| format!("観察画像を一時保存できません: {error}"))?;

    let frame = ObservationFrameInput {
        display: None,
        window_id: None,
        window_bounds: None,
        scope_generation: state.core_runtime().watch_scope_generation(),
        context_id: frame_id.clone(),
        captured_at,
        debug_id: Some(frame_id),
        relative_seconds: 0.0,
        trigger: ActivityTriggerKind::Timer,
        front_app: None,
        app: None,
        target: "developer-selected-image".to_owned(),
        ocr_text: None,
        focus: None,
        own_window_context: None,
        image_path,
    };
    let directories = Arc::new(vec![directory]);
    let observation = state
        .core_runtime()
        .observe_without_companion_delivery(
            vec![frame],
            state.cancellation.child_token(),
            directories.clone(),
        )
        .await
        .map_err(|error| format!("画像観察に失敗しました: {error}"))?;
    let source_id = observation.id().to_owned();
    let outcome = if context.is_empty() {
        state
            .core_runtime()
            .companion_observations_with_result(vec![observation])
            .await
    } else {
        state
            .core_runtime()
            .companion_nudge_with_result(observation, context)
            .await
    }
    .map_err(|error| format!("proactive Companion の判断に失敗しました: {error}"))?;
    let coosenpai_core::runtime::CompanionObservationResult {
        response,
        call_id,
        deferred,
    } = outcome;

    let runtime = state.runtime_snapshot();
    let status = if call_id.is_some() {
        if response.emit {
            "emitted"
        } else {
            "silent"
        }
    } else if deferred {
        "deferred"
    } else {
        "silent"
    };
    state
        .publish_event(crate::snapshot_presenter::SnapshotEvent::Runtime(runtime))
        .await;
    state.refresh_debug().await;

    Ok(DebugWakeResult {
        source_id,
        call_id,
        status: status.to_owned(),
        emit: response.emit,
        message_kind: response.message_kind,
        thought: response.thought,
        message: response.message,
    })
}

fn normalize_image(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.is_empty() {
        return Err("画像が空です".to_owned());
    }
    if bytes.len() > coosenpai_core::attachments::MAX_ATTACHMENT_BYTES {
        return Err("選択した画像が大きすぎます".to_owned());
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("画像を読み込めません: {error}"))?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Png) | Some(ImageFormat::Jpeg)
    ) {
        return Err("PNG または JPEG の画像だけを選択できます".to_owned());
    }
    let limits = coosenpai_core::image_processing::ImageLimits::default();
    let mut decode_limits = image::Limits::default();
    decode_limits.max_image_width = Some(limits.max_width);
    decode_limits.max_image_height = Some(limits.max_height);
    decode_limits.max_alloc = Some(limits.max_decoded_bytes);
    reader.limits(decode_limits);
    let decoded = reader
        .decode()
        .map_err(|error| format!("画像を読み込めません: {error}"))?;
    let mut output = Cursor::new(Vec::new());
    decoded
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|error| format!("画像を PNG に変換できません: {error}"))?;
    Ok(output.into_inner())
}

#[tauri::command]
pub async fn developer_brain_activity_open(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(
        match state
            .ui
            .request(UiView::Application, UiEvent::OpenBrainActivity)
            .await
        {
            Ok(_) => IpcResult::success(()),
            Err(error) => IpcResult::failure(error),
        },
    )
}

#[tauri::command]
pub async fn developer_brain_activity_close(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::BrainActivity)?;
    Ok(
        match state
            .ui
            .request(UiView::BrainActivity, UiEvent::Close)
            .await
        {
            Ok(_) => IpcResult::success(()),
            Err(error) => IpcResult::failure(error),
        },
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrainActivityPayload {
    generation: u64,
    record_id: u64,
    input_id: String,
}

#[tauri::command]
pub async fn developer_brain_activity(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: BrainActivityPayload,
) -> TauriIpcResult<Option<coosenpai_core::brain_activity::BrainActivityRecord>> {
    authorize_window(&window, CommandOrigin::BrainActivity)?;
    if payload.generation == 0
        || payload.record_id == 0
        || payload.input_id.is_empty()
        || payload.input_id.len() > 256
    {
        return Ok(IpcResult::failure("invalid brain activity input ID"));
    }
    Ok(IpcResult::success(
        state.core_runtime().brain_activity_record(
            payload.generation,
            payload.record_id,
            &payload.input_id,
        ),
    ))
}

#[tauri::command]
pub async fn developer_brain_activity_history(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<coosenpai_core::brain_activity::BrainActivityHistory> {
    authorize_window(&window, CommandOrigin::BrainActivity)?;
    Ok(IpcResult::success(
        state.core_runtime().brain_activity_history(),
    ))
}
