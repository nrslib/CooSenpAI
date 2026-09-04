use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::{
    authorize_window, dispatch_result, validate_id, CommandOrigin, IpcResult, TauriIpcResult,
};
use crate::state::DesktopState;
use coosenpai_core::ports::RuntimeLogger;
use serde::Deserialize;
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BubbleAckPayload {
    generation: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BubbleRendererReadyPayload {
    attempt: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BubbleDismissPayload {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsAppearancePreviewPayload {
    theme: String,
    font: String,
    avatar_color: String,
    bubble_position: String,
    bubble_display: String,
}

#[tauri::command]
pub async fn settings_appearance_preview(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: Option<SettingsAppearancePreviewPayload>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let preview = match payload {
        Some(payload) => {
            if !matches!(payload.theme.as_str(), "system" | "light" | "dark")
                || !matches!(
                    payload.bubble_position.as_str(),
                    "bottom-right" | "top-right" | "bottom-left" | "top-left"
                )
                || !matches!(payload.bubble_display.as_str(), "main" | "cursor" | "front")
                || payload.font.trim().is_empty()
                || payload.avatar_color.trim().is_empty()
            {
                return Ok(IpcResult::failure("見た目のプレビュー値が不正です"));
            }
            Some(crate::bubbles::BubbleAppearancePreview {
                theme: payload.theme,
                font: payload.font,
                avatar_color: payload.avatar_color,
                position: payload.bubble_position,
                display: payload.bubble_display,
            })
        }
        None => None,
    };
    let handler_state = state.inner().clone();
    Ok(dispatch_result(
        state.inner().clone(),
        CommandSource::IpcMain,
        DesktopCommand::SettingsAppearancePreview,
        move |_context| async move {
            handler_state
                .bubbles
                .lock()
                .await
                .set_appearance_preview(preview);
            match crate::bubbles::sync_window(handler_state.as_ref()).await {
                Ok(()) => IpcResult::success(()),
                Err(error) => IpcResult::failure(error.to_string()),
            }
        },
    )
    .await)
}

#[tauri::command]
pub async fn bubble_dismiss(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: BubbleDismissPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    validate_id(&payload.id)?;
    Ok(dismiss_bubble_for_state(state.inner().clone(), payload.id).await)
}

#[tauri::command]
pub async fn tutorial_sequence_fast_forward(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: Option<BubbleDismissPayload>,
) -> TauriIpcResult<bool> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    if let Some(payload) = &payload {
        validate_id(&payload.id)?;
    }
    let handler_state = state.inner().clone();
    Ok(dispatch_result(
        state.inner().clone(),
        CommandSource::IpcBubble,
        DesktopCommand::TutorialFastForward,
        move |_context| async move {
            IpcResult::success(
                handler_state
                    .fast_forward_tutorial_sequence(payload.as_ref().map(|item| item.id.as_str()))
                    .await,
            )
        },
    )
    .await)
}

pub(crate) async fn dismiss_bubble_for_state(
    state: Arc<DesktopState>,
    id: String,
) -> IpcResult<()> {
    let (restarts_setup, allows_manual_dismiss) = {
        let bubbles = state.bubbles.lock().await;
        (
            bubbles.restarts_setup_on_dismiss(&id),
            bubbles.allows_manual_dismiss(&id),
        )
    };
    if !allows_manual_dismiss {
        return IpcResult::failure("チュートリアルの案内は手動で閉じられません");
    }
    let command = if restarts_setup {
        DesktopCommand::SetupRestart
    } else {
        DesktopCommand::BubbleDismiss
    };
    let handler_state = state.clone();
    dispatch_result(
        state,
        CommandSource::IpcBubble,
        command,
        move |context| async move {
            if restarts_setup {
                match handler_state
                    .command_dismiss_setup_bubble(&context, &id)
                    .await
                {
                    Ok(()) => IpcResult::success(()),
                    Err(error) => IpcResult::failure(error.to_string()),
                }
            } else {
                crate::bubbles::dismiss(handler_state.as_ref(), &id).await;
                IpcResult::success(())
            }
        },
    )
    .await
}

#[tauri::command]
pub async fn bubble_snapshot(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<crate::bubbles::BubbleSnapshot> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    Ok(IpcResult::success(snapshot(state.inner().as_ref()).await))
}

#[tauri::command]
pub async fn bubble_renderer_ready(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: BubbleRendererReadyPayload,
) -> TauriIpcResult<crate::bubbles::BubbleSnapshot> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    if payload.attempt == 0 {
        return Ok(IpcResult::failure("renderer 初期化回数が不正です"));
    }
    let snapshot = snapshot(state.inner().as_ref()).await;
    let setup_visible = snapshot
        .records
        .iter()
        .any(|record| record.message_kind == "setup");
    let _ = state.logger.write(
        "INFO",
        &format!(
            "吹き出しrendererの初期化を確認しsnapshotを配信しました: attempt={} generation={} records={} setup={setup_visible}",
            payload.attempt,
            snapshot.generation,
            snapshot.records.len(),
        ),
    );
    Ok(IpcResult::success(snapshot))
}

#[tauri::command]
pub async fn bubble_ack(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: BubbleAckPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    let (acknowledged, setup_visible) = {
        let bubbles = state.bubbles.lock().await;
        (
            bubbles.acknowledge(payload.generation),
            bubbles.record_for_message_kind("setup").is_some(),
        )
    };
    if !acknowledged {
        return Ok(IpcResult::failure("未知の吹き出しgenerationです"));
    }
    if setup_visible {
        let _ = state.logger.write(
            "INFO",
            &format!(
                "初回セットアップ吹き出しの表示ACKを受信しました: generation={}",
                payload.generation
            ),
        );
    }
    Ok(IpcResult::success(()))
}

async fn snapshot(state: &DesktopState) -> crate::bubbles::BubbleSnapshot {
    let config = state.runtime_config();
    let avatar_image_png = state.snapshot().await.avatar_image_png;
    let bubbles = state.bubbles.lock().await;
    let preview = bubbles.appearance_preview();
    bubbles.snapshot_with_appearance(
        preview
            .as_ref()
            .map_or(config.ui.theme.as_str(), |value| value.theme.as_str()),
        preview
            .as_ref()
            .map_or(config.ui.font.as_str(), |value| value.font.as_str()),
        preview
            .as_ref()
            .map(|value| value.avatar_color.as_str())
            .or(config.ui.avatar_color.as_deref()),
        preview
            .as_ref()
            .map_or(config.bubble.position.as_str(), |value| {
                value.position.as_str()
            }),
        avatar_image_png.as_deref(),
    )
}
