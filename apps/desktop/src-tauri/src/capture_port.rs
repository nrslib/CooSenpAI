use super::effects::{CaptureEffect, CaptureResult};
use super::manager::{CaptureFuture, CapturePort};
use super::{region, window, CaptureKind, ReadyAttachment, ReadyCapture};
use crate::command_guard::{CommandSource, DesktopCommand};
use crate::state::DesktopState;
use coosenpai_core::locale::Locale;
use coosenpai_core::ports::RuntimeLogger;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct DesktopCapturePort {
    state: Arc<DesktopState>,
    window: window::CapturePopupView,
    speech_view: crate::speech_view::SpeechPopupView,
    session: crate::selection_session::SelectionSessionHandle,
}

impl DesktopCapturePort {
    pub(crate) fn new(state: Arc<DesktopState>, window: window::CapturePopupView) -> Self {
        Self {
            window,
            session: crate::selection_session::SelectionSessionHandle::native(state.clone()),
            speech_view: crate::speech_view::SpeechPopupView::new(state.app.clone()),
            state,
        }
    }
}

impl CapturePort for DesktopCapturePort {
    fn execute(&self, event: CaptureEffect) -> CaptureFuture<CaptureResult> {
        let port = self.clone();
        Box::pin(async move {
            match event {
                CaptureEffect::OpenSelection {
                    kind,
                    source,
                    generation,
                    origin,
                } => {
                    return port
                        .select(kind, source, generation, origin)
                        .await
                        .map(CaptureResult::Selected);
                }
                CaptureEffect::CloseSelection {
                    generation,
                    shutdown,
                } => {
                    port.session.close(generation, shutdown).await?;
                    return Ok(CaptureResult::Selected(None));
                }
                CaptureEffect::Send { content, message } => {
                    return port.send(content, message).await.map(CaptureResult::Sent)
                }
                CaptureEffect::Voice {
                    action,
                    cancellation,
                } => {
                    let result =
                        super::effects::voice_control(action, cancellation, port.voice(action))
                            .await?;
                    if let Some(generation) = result {
                        return Ok(CaptureResult::VoiceStarted(generation));
                    }
                }
                CaptureEffect::VoiceSend { generation, text } => {
                    return port
                        .state
                        .command_speech_confirm(generation, text)
                        .await
                        .map(CaptureResult::Sent)
                }
                CaptureEffect::SpeechView(command) => port.speech_view.apply(command)?,
                CaptureEffect::LoadPopup {
                    generation,
                    content,
                } => {
                    let snapshot =
                        super::snapshot_content(&port.state, generation, &content).await?;
                    return Ok(CaptureResult::LoadedPopup(Box::new(snapshot)));
                }
                CaptureEffect::PopupSession(generation) => crate::webview_event::emit_to(
                    &port.state.app,
                    "capture-popup",
                    "coosenpai:capture-popup:session",
                    &generation,
                )
                .map_err(|error| error.to_string())?,
                CaptureEffect::ArmPopup(generation) => port.window.arm(generation).await?,
                CaptureEffect::RenderPopup(snapshot) => crate::webview_event::emit_to(
                    &port.state.app,
                    "capture-popup",
                    "coosenpai:capture-popup:load",
                    snapshot.as_ref(),
                )
                .map_err(|error| error.to_string())?,
                CaptureEffect::HidePopup { content, sent } => port.hide(&content, sent).await?,
                CaptureEffect::Input {
                    generation,
                    edit_revision,
                    message,
                    sending,
                    can_send,
                    preview_state,
                    error,
                } => {
                    crate::webview_event::emit_to(&port.state.app, "capture-popup", "coosenpai:capture-popup:input", &serde_json::json!({"generation": generation, "editRevision": edit_revision, "message": message, "sending": sending, "canSend": can_send, "previewState": preview_state, "error": error})).map_err(|error| error.to_string())?;
                }
                CaptureEffect::OpenAccessibilitySettings => {
                    use coosenpai_core::ports::SystemSettingsPort;
                    crate::platform::MacSystemSettings
                        .open(
                            coosenpai_core::ports::SystemSettingsPane::Accessibility,
                            port.state.cancellation.child_token(),
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                }
                CaptureEffect::Error(message) => port.report_error(&message).await,
            }
            Ok(CaptureResult::Done)
        })
    }
}

impl DesktopCapturePort {
    async fn voice(&self, action: crate::ui_events::VoiceAction) -> Result<Option<u64>, String> {
        use crate::ui_events::VoiceAction;
        let state = self.state.clone();
        let Some((source, command)) = voice_command(action) else {
            return state.apply_voice_cancel().await.map(|()| None);
        };
        let handler_state = state.clone();
        state
            .dispatch(source, command, move |context| async move {
                match action {
                    VoiceAction::Start(source) => {
                        handler_state
                            .command_speech_begin(&context, source)
                            .await
                            .map_err(crate::command_guard::DispatchError::handler)?;
                        Ok(Some(
                            context
                                .fence(crate::command_guard::GenerationResource::Speech)
                                .expect("voice start generation")
                                .value,
                        ))
                    }
                    VoiceAction::Finish => {
                        handler_state.command_speech_finish(&context);
                        Ok(None)
                    }
                    VoiceAction::Cancel => unreachable!(),
                }
            })
            .await
            .map_err(|error| {
                error.format_for_locale(Locale::from_config(&state.runtime_config().ui.language))
            })
    }
    fn select(
        &self,
        kind: CaptureKind,
        source: CommandSource,
        generation: u64,
        origin: super::CaptureOrigin,
    ) -> CaptureFuture<Option<Arc<ReadyCapture>>> {
        let state = self.state.clone();
        let port = self.clone();
        Box::pin(async move {
            let command = match kind {
                CaptureKind::Image => DesktopCommand::CaptureStartImage,
                CaptureKind::Text => DesktopCommand::CaptureStartText,
                CaptureKind::Voice => return Err("音声入力にはVoiceイベントが必要です".to_owned()),
            };
            let admission = state.dispatch(source, command, move |context| async move {
                Ok(context
                    .fence(crate::command_guard::GenerationResource::Conversation)
                    .expect("capture start conversation"))
            });
            let conversation = admission.await.map_err(|error| {
                error.format_for_locale(Locale::from_config(&state.runtime_config().ui.language))
            })?;
            let _ = state.logger.write("DEBUG", "範囲選択: 段階=prepare");
            let previous_error = super::current_shortcut_error_token(&state).await;
            let mut events = port.session.open(generation, kind)?;
            let selected = loop {
                match events.next().await? {
                    crate::selection_session::SessionEvent::Opened => state.ui.input(
                        crate::ui_events::UiView::Application,
                        crate::ui_events::UiEvent::CaptureCompleted(Box::new(
                            super::CaptureEvent::Selection {
                                generation,
                                event: super::manager::SelectionEvent::Opened,
                            },
                        )),
                    ),
                    crate::selection_session::SessionEvent::Completed(result) => break result?,
                }
            };
            let Some(selected) = selected else {
                return Ok(None);
            };
            let content = prepare_selected_capture(selected, origin, conversation).await?;
            super::clear_shortcut_error_if_current(&state, previous_error).await;
            Ok(Some(Arc::new(content)))
        })
    }

    async fn hide(&self, _content: &ReadyCapture, _sent: bool) -> Result<(), String> {
        self.window.hide().await?;
        let _ = self
            .state
            .logger
            .write("INFO", "送信ポップアップを閉じました");
        Ok(())
    }

    fn send(&self, content: Arc<ReadyCapture>, message: String) -> CaptureFuture<String> {
        let state = self.state.clone();
        Box::pin(async move {
            state
                .ui
                .request(
                    crate::ui_events::UiView::CapturePopup,
                    crate::ui_events::UiEvent::SubmitInput(crate::ui_events::ChatInput::Capture {
                        content,
                        message,
                    }),
                )
                .await?
                .ok_or_else(|| "送信の受理結果がありません".to_owned())
        })
    }

    async fn report_error(&self, error: &str) {
        let _ = self
            .state
            .logger
            .write("WARN", &format!("範囲選択: 段階=failed error={error}"));
        self.state.ui.input(
            crate::ui_events::UiView::Application,
            crate::ui_events::UiEvent::SnapshotCompleted(Box::new(
                crate::snapshot_presenter::SnapshotEvent::Shortcut(
                    crate::capture::ShortcutErrorEvent::Transient {
                        speech_generation: None,
                        message: error.to_owned(),
                    },
                ),
            )),
        );
    }
}

pub(super) async fn prepare_selected_capture(
    selected: crate::selection_session::SelectionData,
    origin: super::CaptureOrigin,
    conversation: crate::command_guard::GenerationStamp,
) -> Result<ReadyCapture, String> {
    use crate::selection_session::SelectionData;
    match selected {
        SelectionData::Image(image) => region::prepare_image(image, origin, conversation).await,
        SelectionData::Text {
            attachment,
            permission_required,
        } => Ok(ReadyCapture {
            conversation,
            id: uuid::Uuid::new_v4().to_string(),
            attachment: ReadyAttachment::Text(attachment),
            origin,
            accessibility_permission_required: permission_required,
        }),
    }
}

pub(super) fn voice_command(
    action: crate::ui_events::VoiceAction,
) -> Option<(CommandSource, DesktopCommand)> {
    use crate::ui_events::VoiceAction;
    match action {
        VoiceAction::Start(source) => Some((
            match source {
                crate::speech::SpeechSource::Composer => CommandSource::IpcMain,
                crate::speech::SpeechSource::Shortcut => CommandSource::GlobalShortcut,
            },
            DesktopCommand::SpeechStart,
        )),
        VoiceAction::Finish => Some((CommandSource::GlobalShortcut, DesktopCommand::SpeechFinish)),
        VoiceAction::Cancel => None,
    }
}
