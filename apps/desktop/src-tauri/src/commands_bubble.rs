use crate::commands::{
    authorize_window, validate_id_for_locale, CommandOrigin, IpcResult, TauriIpcResult,
};
use crate::state::DesktopState;
use crate::ui_commands::UserCommand;
use crate::ui_events::{UiEvent, UiView};
use coosenpai_core::locale::{text, Locale, TextKey};
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
#[serde(deny_unknown_fields)]
pub struct BubbleNavigatePayload {
    direction: crate::bubbles::BubbleDeckDirection,
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
                return Ok(IpcResult::failure(text(
                    TextKey::BubbleAppearancePreviewInvalid,
                    Locale::from_config(&state.runtime_config().ui.language),
                )));
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
    state
        .ui
        .query(UiView::Settings, |reply| UiEvent::SettingsPreview {
            preview,
            reply,
        })
        .await
}

#[tauri::command]
pub async fn bubble_dismiss(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: BubbleDismissPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    validate_id_for_locale(
        &payload.id,
        Locale::from_config(&state.runtime_config().ui.language),
    )?;
    Ok(dismiss_bubble_for_state(state.inner().clone(), payload.id).await)
}

#[tauri::command]
pub async fn bubble_fast_forward(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: Option<BubbleDismissPayload>,
) -> TauriIpcResult<bool> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    if let Some(payload) = &payload {
        validate_id_for_locale(
            &payload.id,
            Locale::from_config(&state.runtime_config().ui.language),
        )?;
    }
    state
        .ui
        .query(UiView::Bubble, |reply| {
            UiEvent::UserCommand(UserCommand::BubbleFastForward {
                id: payload.map(|item| item.id),
                reply,
            })
        })
        .await
}

#[tauri::command]
pub async fn bubble_navigate(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: BubbleNavigatePayload,
) -> TauriIpcResult<bool> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    state
        .ui
        .query(UiView::Bubble, |reply| {
            UiEvent::UserCommand(UserCommand::BubbleNavigate {
                direction: payload.direction,
                reply,
            })
        })
        .await
}

pub(crate) async fn dismiss_bubble_for_state(
    state: Arc<DesktopState>,
    id: String,
) -> IpcResult<()> {
    state
        .ui
        .query(UiView::Bubble, |reply| {
            UiEvent::UserCommand(UserCommand::BubbleDismiss { id, reply })
        })
        .await
        .unwrap_or_else(IpcResult::failure)
}

#[tauri::command]
pub async fn bubble_snapshot(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<crate::bubbles::BubbleSnapshot> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    state
        .ui
        .query(UiView::Bubble, UiEvent::BubbleSnapshot)
        .await
}

#[tauri::command]
pub async fn bubble_renderer_ready(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: BubbleRendererReadyPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    if payload.attempt == 0 {
        return Ok(IpcResult::failure("初期化の試行番号が不正です"));
    }
    Ok(
        match state
            .ui
            .request(
                crate::ui_events::UiView::Bubble,
                crate::ui_events::UiEvent::BubbleRendererReady {
                    attempt: payload.attempt,
                },
            )
            .await
        {
            Ok(_) => IpcResult::success(()),
            Err(error) => IpcResult::failure(error),
        },
    )
}

#[tauri::command]
pub async fn bubble_ack(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: BubbleAckPayload,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    state
        .ui
        .query(UiView::Bubble, |reply| UiEvent::BubbleAck {
            generation: payload.generation,
            reply,
        })
        .await
}

#[tauri::command]
pub async fn bubble_view_input<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, crate::bubble_click::BubbleClickState>,
    payload: crate::bubble_controls_presenter::BubbleViewInput,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Bubble)?;
    use crate::bubble_controls_presenter::BubbleViewInput as Input;
    let host = &state.inner().0;
    let locale = host.locale();
    match &payload {
        Input::Pointer { id, .. }
        | Input::Body { id, .. }
        | Input::Key { id, .. }
        | Input::Dismiss { id }
        | Input::Select { id, .. }
        | Input::Secret { id, .. }
        | Input::Action { id, .. } => validate_id_for_locale(id, locale)?,
        _ => {}
    }
    if let Input::Action { action, .. } = &payload {
        validate_id_for_locale(action, locale)?;
    }
    if matches!(&payload, Input::Select { value, .. } | Input::Secret { value, .. } if value.len() > crate::commands::MAX_CHAT_BYTES)
    {
        return Err("入力文が長すぎます".into());
    }
    host.input_view(payload).await?;
    Ok(IpcResult::success(()))
}
