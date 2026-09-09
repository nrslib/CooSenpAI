use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::{
    authorize_window, dispatch_result, CommandOrigin, IpcResult, TauriIpcResult,
};
use crate::state::DesktopState;
use crate::voice_output::VoiceOutputSnapshot;
use coosenpai_core::locale::{text, Locale, TextKey};
use std::sync::Arc;

#[tauri::command]
pub(crate) async fn voice_output_voices(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<Vec<crate::platform::VoicevoxVoice>> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    Ok(
        match crate::platform::voicevox_voices(locale, state.cancellation.child_token()).await {
            Ok(voices) => IpcResult::success(voices),
            Err(message) => IpcResult::failure(message),
        },
    )
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoiceOutputLinkPayload {
    kind: String,
}

fn voice_output_link(kind: &str) -> Result<&'static str, &'static str> {
    match kind {
        "download" => Ok("https://voicevox.hiroshiba.jp/"),
        "terms" => Ok("https://voicevox.hiroshiba.jp/term/"),
        _ => Err("未対応のリンクです"),
    }
}

#[tauri::command]
pub(crate) async fn voice_output_open_link(
    window: tauri::WebviewWindow,
    payload: VoiceOutputLinkPayload,
    state: tauri::State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let url = match voice_output_link(&payload.kind) {
        Ok(url) => url,
        Err(_) => {
            return Ok(IpcResult::failure(text(
                TextKey::VoicevoxUnsupportedLink,
                locale,
            )))
        }
    };
    Ok(match crate::platform::open_external_url(url).await {
        Ok(()) => IpcResult::success(()),
        Err(_) => IpcResult::failure(text(TextKey::VoicevoxOpenLinkFailed, locale)),
    })
}

#[tauri::command]
pub(crate) fn voice_output_status(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<DesktopState>>,
) -> crate::commands::TauriIpcResult<VoiceOutputSnapshot> {
    crate::commands::authorize_window(&window, crate::commands::CommandOrigin::Main)?;
    Ok(crate::commands::IpcResult::success(
        state.voice_output.snapshot(),
    ))
}

#[tauri::command]
pub(crate) async fn voice_output_stop(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<VoiceOutputSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::VoiceOutputStop,
        move |_context| async move {
            handler.voice_output.stop().await;
            IpcResult::success(handler.voice_output.snapshot())
        },
    )
    .await)
}

#[tauri::command]
pub(crate) async fn voice_output_test(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<VoiceOutputSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    let state = state.inner().clone();
    let handler = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::VoiceOutputTest,
        move |_context| async move {
            let generation = handler.voice_conversation_generation().await;
            let locale = Locale::from_config(&handler.runtime_config().ui.language);
            match handler
                .voice_output
                .enqueue(
                    &handler,
                    text(TextKey::VoiceOutputTestText, locale).to_owned(),
                    generation,
                )
                .await
            {
                Ok(()) => IpcResult::success(handler.voice_output.snapshot()),
                Err(message) => IpcResult::failure(message),
            }
        },
    )
    .await)
}

