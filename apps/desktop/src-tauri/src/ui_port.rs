use crate::capture::manager::CapturePort;
use crate::capture::CaptureKind;
use crate::command_guard::{CommandSource, DesktopCommand};
use crate::state::DesktopState;
use crate::ui_events::{
    EffectResult, PresenterId, UiEffect, UiEvent, UiTask, ViewCommand, VoiceAction,
};
use crate::ui_root::UiPort;
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::RuntimeLogger;
use std::sync::Arc;
use tauri::{Emitter, Manager};

pub(crate) struct DesktopUiPort<P = NativeUiPort> {
    native: P,
}

impl DesktopUiPort {
    pub(crate) fn new(
        state: Arc<DesktopState>,
        popup: crate::capture::window::CapturePopupView,
    ) -> Self {
        Self {
            native: NativeUiPort::new(state, popup),
        }
    }
}

#[cfg(test)]
impl<P: UiPort> DesktopUiPort<P> {
    pub(crate) fn with_native(native: P) -> Self {
        Self { native }
    }
}

#[async_trait::async_trait]
impl<P: UiPort> UiPort for DesktopUiPort<P> {
    async fn execute(&self, effect: UiEffect) -> Result<EffectResult, String> {
        self.native.execute(effect).await
    }

    async fn run(&self, task: UiTask) -> Result<EffectResult, String> {
        let UiTask::BubbleClick { generation, target } = task else {
            return self.native.run(task).await;
        };
        let mut loaded = self
            .native
            .run(UiTask::Load {
                view: PresenterId::Chat,
                generation,
                request: crate::ui_load::WindowRequest::Main,
            })
            .await?;
        let Some(UiEvent::Window {
            view: PresenterId::Chat,
            event: crate::presentation::PresentationEvent::Loaded { result, .. },
        }) = loaded.events.pop()
        else {
            return Err("吹き出しクリックのメイン画面読込結果がありません".to_owned());
        };
        loaded.events.push(UiEvent::BubbleClickPrepared {
            generation,
            target,
            result,
        });
        Ok(loaded)
    }
}

pub(crate) struct NativeUiPort {
    state: Arc<DesktopState>,
    capture: crate::capture::port::DesktopCapturePort,
}

impl NativeUiPort {
    pub(crate) fn new(
        state: Arc<DesktopState>,
        popup: crate::capture::window::CapturePopupView,
    ) -> Self {
        Self {
            capture: crate::capture::port::DesktopCapturePort::new(state.clone(), popup),
            state,
        }
    }
}

#[async_trait::async_trait]
impl UiPort for NativeUiPort {
    async fn execute(&self, effect: UiEffect) -> Result<EffectResult, String> {
        let state = &self.state;
        let mut result = EffectResult::done();
        match effect {
            UiEffect::PanelOutput { reply, output } => {
                let _ = reply.send(output);
            }
            UiEffect::AvatarSceneRender(view) => crate::webview_event::emit_to(
                &state.app,
                "avatar",
                "coosenpai:avatar-scene:view",
                &view,
            )
            .map_err(|e| e.to_string())?,
            UiEffect::MotionSettingsRender(view) => crate::webview_event::emit_to(
                &state.app,
                "main",
                "coosenpai:motion-settings:view",
                &view,
            )
            .map_err(|e| e.to_string())?,
            UiEffect::MotionStorage(command) => crate::webview_event::emit_to(
                &state.app,
                "main",
                "coosenpai:motion-settings:storage",
                &command,
            )
            .map_err(|e| e.to_string())?,
            UiEffect::AppRender(view) => {
                crate::webview_event::emit_to(&state.app, "main", "coosenpai:app:view", &view)
                    .map_err(|e| e.to_string())?
            }
            UiEffect::StatusRender(view) => {
                crate::webview_event::emit_to(&state.app, "main", "coosenpai:status:view", &view)
                    .map_err(|e| e.to_string())?
            }
            UiEffect::PersonasRender(personas) => crate::webview_event::emit_to(
                &state.app,
                "main",
                "coosenpai:personas:load",
                &personas,
            )
            .map_err(|e| e.to_string())?,
            UiEffect::WorkApprovalRender(view) => crate::webview_event::emit_to(
                &state.app,
                "main",
                "coosenpai:work-approval:view",
                &view,
            )
            .map_err(|e| e.to_string())?,
            UiEffect::ComposerRender(view) => {
                crate::webview_event::emit_to(&state.app, "main", "coosenpai:composer:view", &view)
                    .map_err(|e| e.to_string())?
            }
            UiEffect::ConversationRender(view) => crate::webview_event::emit_to(
                &state.app,
                "main",
                "coosenpai:conversation:view",
                &view,
            )
            .map_err(|e| e.to_string())?,
            UiEffect::BubbleControls(view) => crate::webview_event::emit_to(
                &state.app,
                "bubble",
                "coosenpai:bubble:controls",
                &view,
            )
            .map_err(|e| e.to_string())?,
            UiEffect::ModelPickerRender(view) => crate::webview_event::emit_to(
                &state.app,
                "model-popup",
                "coosenpai:model-picker:view",
                &view,
            )
            .map_err(|e| e.to_string())?,
            UiEffect::TrayRender(view) => crate::windows::render_tray(&state.app, &view)?,
            UiEffect::ChatInputActive(active) => state
                .input_active
                .store(active, std::sync::atomic::Ordering::Release),
            UiEffect::OsNotification {
                record,
                guard,
                reply,
            } => {
                let notifier = crate::platform::MacNotifier::new(
                    state.runtime_snapshot().companion_display_name,
                );
                let accepted = render_os(&notifier, &record, guard).await;
                let _ = reply.send(accepted);
            }

            UiEffect::AvatarRender(value) => crate::avatar_view::render(&state.app, &value)?,
            UiEffect::AvatarVisibility {
                visible,
                generation,
                position_initial,
                language,
            } => {
                result.events.push(UiEvent::AvatarApplied {
                    visible,
                    generation,
                    result: crate::avatar_view::visibility(
                        &state.app,
                        visible,
                        position_initial,
                        &language,
                    ),
                });
            }
            UiEffect::ChatProjection(projection) => render_chat(&state.app, projection)?,
            UiEffect::CaptureView {
                effects,
                completion,
            } => {
                return Ok(crate::capture::effects::apply(&self.capture, effects, completion).await)
            }
            UiEffect::Spawn(_) => unreachable!("Root starts capture tasks"),
            UiEffect::SelectConversation(id) => {
                state
                    .app
                    .emit_to("main", "coosenpai:conversation:selected", id)
                    .map_err(|error| error.to_string())?;
            }
            UiEffect::ForceShutdown => {
                coosenpai_core::process::force_kill_provider_processes();
                std::process::exit(0);
            }
            UiEffect::RejectInput => {
                return Err(
                    "送信ポップアップを送信するか、明示的に閉じてから切り替えてください".into(),
                )
            }
            UiEffect::SettingsFocus(section) => state
                .app
                .emit_to("main", "coosenpai:settings:focus", section)
                .map_err(|error| error.to_string())?,
            UiEffect::MainFocus(focused) => {
                state
                    .main_window_focused
                    .store(focused, std::sync::atomic::Ordering::Release);
            }
            UiEffect::BubbleFocused(focused) => state.set_bubble_focused(focused),
            UiEffect::Pointer(enabled) => state
                .app
                .get_webview_window("bubble")
                .ok_or("吹き出しがありません")?
                .set_ignore_cursor_events(!enabled)
                .map_err(|error| error.to_string())?,
            UiEffect::BubbleTyping { id, revealed } => crate::webview_event::emit_to(
                &state.app,
                "bubble",
                "coosenpai:bubble:typing",
                &serde_json::json!({"id": id, "revealed": revealed}),
            )
            .map_err(|error| error.to_string())?,
            UiEffect::BubbleRender {
                snapshot,
                display,
                layout,
            } => {
                let window = state
                    .app
                    .get_webview_window("bubble")
                    .ok_or("吹き出しがありません")?;
                if layout {
                    crate::window_bubble::update_layout(&window, &snapshot.position, &display)
                        .map_err(|error| error.to_string())?;
                }
                crate::webview_event::emit_to(
                    &state.app,
                    "bubble",
                    "coosenpai:bubble:show",
                    snapshot.as_ref(),
                )
                .map_err(|error| error.to_string())?;
            }
            UiEffect::BubbleResize {
                height,
                position,
                display,
            } => {
                let window = state
                    .app
                    .get_webview_window("bubble")
                    .ok_or("吹き出しがありません")?;
                crate::window_bubble::resize(&window, height, &position, &display)
                    .map_err(|error| error.to_string())?;
            }
            UiEffect::View { view, command } => {
                result = view_applied(view, command, self.apply_view(view, command).await)?;
            }
            UiEffect::RenderSnapshot { view, snapshot } => {
                let label = match view {
                    PresenterId::Chat => "main",
                    PresenterId::Details => "details",
                    PresenterId::ModelPicker => "model-popup",
                    _ => return Err(format!("未対応のsnapshot受信者: {view:?}")),
                };
                crate::webview_event::emit_to(
                    &state.app,
                    label,
                    "coosenpai:snapshot:updated",
                    &crate::snapshot::SnapshotEvent {
                        revision: snapshot.revision,
                        snapshot: (*snapshot).clone(),
                    },
                )
                .map_err(|error| error.to_string())?;
            }
            UiEffect::Log(message) => state
                .logger
                .write("INFO", &message)
                .map_err(|error| error.to_string())?,
            UiEffect::RenderWindow(content) => self.render_window(content)?,
            UiEffect::ActivationView(_)
            | UiEffect::Activation(_)
            | UiEffect::Run(_)
            | UiEffect::Complete(_)
            | UiEffect::Fail(_)
            | UiEffect::Deliver { .. } => unreachable!("UI scheduling belongs to UiRoot"),
        }
        Ok(result)
    }
    async fn run(&self, task: UiTask) -> Result<EffectResult, String> {
        let state = &self.state;
        let mut result = EffectResult::done();
        match task {
            UiTask::SettingsPreview(preview) => {
                let completed = state
                    .ui
                    .query(crate::ui_events::UiView::Application, |reply| {
                        UiEvent::UserCommand(crate::ui_commands::UserCommand::BubblePreview {
                            preview,
                            reply,
                        })
                    })
                    .await;
                result
                    .events
                    .push(UiEvent::SettingsPreviewCompleted(match completed {
                        Ok(result) => result,
                        Err(message) => crate::commands::IpcResult::failure(message),
                    }));
            }
            UiTask::MotionSettings(task) => result
                .events
                .push(crate::motion_settings_presenter::run(state.clone(), task).await),
            UiTask::App(task) => result
                .events
                .push(crate::app_presenter::run(state.clone(), task).await),
            UiTask::StatusDelay(deadline) => result
                .events
                .push(crate::status_presenter::wait(deadline).await),
            UiTask::WorkApproval(task) => result
                .events
                .push(crate::work_approval_presenter::run(state.clone(), task).await),
            UiTask::Composer(task) => result
                .events
                .push(crate::composer_presenter::run(state.clone(), task).await),
            UiTask::BubbleView {
                id,
                token,
                action,
                value,
                restart_setup,
            } => {
                let completed = match action {
                    Some(action) => {
                        crate::ui_command_effects::bubble_interact(
                            state.clone(),
                            id.clone(),
                            action,
                            value,
                        )
                        .await
                    }
                    None => {
                        crate::ui_command_effects::dismiss_bubble_result(
                            state.clone(),
                            id.clone(),
                            restart_setup,
                        )
                        .await
                    }
                };
                result.events.push(UiEvent::BubbleView(
                    crate::bubble_controls_presenter::BubbleViewEvent::Completed {
                        token,
                        id,
                        result: completed,
                    },
                ));
            }
            UiTask::ModelPicker(task) => result
                .events
                .push(crate::model_picker_presenter::run(state.clone(), task).await),
            UiTask::Conversation(task) => result
                .events
                .push(crate::conversation_presenter::run(state.clone(), task).await),
            UiTask::SaveModel {
                patch,
                generation,
                reply,
            } => result.events.push(
                crate::ui_command_effects::save_model(state.clone(), patch, generation, reply)
                    .await,
            ),
            UiTask::DismissBubble {
                id,
                restart_setup,
                reply,
            } => result.events.push(
                crate::ui_command_effects::dismiss_bubble(state.clone(), id, restart_setup, reply)
                    .await,
            ),
            UiTask::UserCommand(command) => result
                .events
                .push(crate::ui_command_effects::execute(state.clone(), command).await),
            UiTask::Tutorial(task) => result.events.push(UiEvent::Tutorial(Box::new(
                crate::tutorial_effects::run(state.clone(), task).await,
            ))),
            UiTask::RefreshConversation => state.refresh_conversation().await,
            UiTask::ReadFactCandidate(date) => result.events.push(UiEvent::FactCandidateLoaded(
                state
                    .load_fact_candidate(date)
                    .await
                    .map_err(|error| error.to_string()),
            )),
            UiTask::RuntimeFollowup => {
                state.refresh_conversation().await;
                result.events.push(UiEvent::Tutorial(Box::new(crate::tutorial_events::TutorialEvent::Response(crate::tutorial_response_presenter::ResponseEvent::RuntimeConversationLoaded))));
            }
            UiTask::ReadThoughtPresentation {
                generation,
                message,
            } => {
                result.events.push(UiEvent::ThoughtRequested {
                    generation,
                    message,
                    context: Box::new(state.notification_context().await),
                });
            }
            UiTask::ClearTemporary { expires_at } => {
                let cleared = state
                    .factory
                    .temporary_assertiveness()
                    .clear_if_expires_at(expires_at);
                result.events.push(UiEvent::SnapshotCompleted(Box::new(
                    crate::snapshot_presenter::SnapshotEvent::TemporaryCleared {
                        expires_at,
                        cleared,
                    },
                )));
            }
            UiTask::LoadAvatar { generation, path } => {
                let avatar = crate::avatar::load_with_status(&state.paths, path.as_deref());
                result.events.push(UiEvent::SnapshotCompleted(Box::new(
                    crate::snapshot_presenter::SnapshotEvent::AvatarLoaded {
                        generation,
                        result: avatar,
                    },
                )));
            }
            UiTask::SavePlacement { path, placement } => {
                result.events.push(UiEvent::Placement(
                    crate::placement_presenter::PlacementEvent::Saved(
                        crate::placement_presenter::save(path, placement).await,
                    ),
                ));
            }

            UiTask::Capture { effect, completion } => {
                let completed = completion.completed(self.capture.execute(effect).await);
                result
                    .events
                    .push(UiEvent::CaptureCompleted(Box::new(completed)));
            }

            UiTask::CompleteNotification {
                presentation,
                reply,
            } => {
                let accepted = crate::bubbles::complete_presentation(state.clone(), presentation)
                    .await
                    .is_ok_and(|outcome| {
                        outcome == crate::bubbles::BubblePresentationOutcome::Acknowledged
                    });
                let _ = reply.send(accepted);
            }
            UiTask::PrepareOsNotification { record, reply } => {
                let guard = state
                    .notification_generation_guard(record.conversation_generation)
                    .await;
                result.events.push(UiEvent::OsNotificationPrepared {
                    record,
                    guard,
                    reply,
                });
            }
            UiTask::EmptyClipboardNotice(target) => {
                crate::capture_notice::show_empty_clipboard(state.clone(), target).await
            }
            UiTask::BubbleClick { .. } => {
                unreachable!("DesktopUiPort returns click completion to Root")
            }
            UiTask::BubbleFastForward(id) => {
                let handler = state.clone();
                state
                    .dispatch(
                        CommandSource::IpcBubble,
                        DesktopCommand::BubbleFastForward,
                        move |_context| async move {
                            let (reply, _) = tokio::sync::oneshot::channel();
                            handler
                                .ui
                                .request(
                                    crate::ui_events::UiView::Bubble,
                                    UiEvent::BubbleMutation {
                                        mutation: crate::bubbles::BubbleMutation::FastForward(
                                            Some(id),
                                        ),
                                        reply,
                                    },
                                )
                                .await
                                .map(|_| ())
                                .map_err(crate::command_guard::DispatchError::handler)
                        },
                    )
                    .await
                    .map_err(|error| {
                        error.format_for_locale(Locale::from_config(
                            &state.runtime_config().ui.language,
                        ))
                    })?;
            }

            UiTask::AnnounceSetup => {
                let handler_state = state.clone();
                state
                    .dispatch(
                        CommandSource::TutorialAutomation,
                        DesktopCommand::SetupPrompt,
                        move |context| async move {
                            handler_state
                                .command_announce_initial_onboarding(&context)
                                .await
                                .map_err(crate::command_guard::DispatchError::handler)
                        },
                    )
                    .await
                    .map_err(|error| {
                        error.format_for_locale(Locale::from_config(
                            &state.runtime_config().ui.language,
                        ))
                    })?;
            }
            UiTask::MainOpened => {
                let handler_state = state.clone();
                state
                    .dispatch(
                        CommandSource::TutorialAutomation,
                        DesktopCommand::TutorialAdvance,
                        move |context| async move {
                            handler_state.command_tutorial_main_opened(&context).await;
                            Ok(())
                        },
                    )
                    .await
                    .map_err(|error| {
                        error.format_for_locale(Locale::from_config(
                            &state.runtime_config().ui.language,
                        ))
                    })?;
            }
            UiTask::ResolveShortcut { shortcut, pressed } => {
                result.events.push(UiEvent::ShortcutResolved {
                    action: state.shortcut_coordinator.action(&shortcut),
                    pressed,
                    voice_mode: state.runtime_config().speech.mode,
                    popup_generation: state
                        .shortcut_coordinator
                        .popup_cancel_target()
                        .map(|target| target.generation),
                });
            }
            UiTask::ToggleWatch => {
                state
                    .dispatch_watch_toggle(CommandSource::GlobalShortcut)
                    .await
                    .map_err(|error| {
                        error.format_for_locale(Locale::from_config(
                            &state.runtime_config().ui.language,
                        ))
                    })?;
            }
            UiTask::CopyLastReply => {
                crate::state::dispatch_copy_last_reply_shortcut(state.clone())
                    .await
                    .map_err(|error| {
                        error.format_for_locale(Locale::from_config(
                            &state.runtime_config().ui.language,
                        ))
                    })?;
            }
            UiTask::Activation(_) => unreachable!("Root runs opaque Activation tasks"),
            UiTask::StartOperation { kind, source } => {
                let generation = state.capture.start_from(kind, source).await?;
                result
                    .events
                    .push(UiEvent::OperationStarted { kind, generation });
            }
            UiTask::StopOperation { kind, generation } => {
                state.capture.interrupt(false).await?;
                result
                    .events
                    .push(UiEvent::OperationEnded { kind, generation });
            }
            UiTask::Delay { duration, event } => {
                tokio::time::sleep(duration).await;
                result.events.push(event);
            }
            UiTask::Voice(action) => {
                let generation = state.capture.voice(action).await?;
                match action {
                    VoiceAction::Start(_) => result.events.push(UiEvent::OperationStarted {
                        kind: CaptureKind::Voice,
                        generation,
                    }),
                    VoiceAction::Cancel => result.events.push(UiEvent::OperationEnded {
                        kind: CaptureKind::Voice,
                        generation,
                    }),
                    VoiceAction::Finish => {}
                }
            }
            UiTask::SubmitInput(input) => {
                use crate::state::user_input::UserMessageAttachment;
                use crate::ui_events::ChatInput;
                let (source, command, fence, message, attachment) = match input {
                    ChatInput::Capture { content, message } => {
                        let (command, attachment) = match &content.attachment {
                            crate::capture::ReadyAttachment::Image { path, .. } => (
                                DesktopCommand::CaptureSendImage,
                                UserMessageAttachment::Image(path.clone()),
                            ),
                            crate::capture::ReadyAttachment::Text(Some(text)) => (
                                DesktopCommand::CaptureSendText,
                                UserMessageAttachment::Text(text.text.clone()),
                            ),
                            crate::capture::ReadyAttachment::Text(None) => {
                                return Err("送信できる文章がありません".into())
                            }
                        };
                        (
                            CommandSource::IpcCapturePopup,
                            command,
                            content.conversation,
                            message.trim().to_owned(),
                            attachment,
                        )
                    }
                    ChatInput::Voice { generation, text } => (
                        CommandSource::SpeechCallback,
                        DesktopCommand::SpeechConfirm,
                        crate::command_guard::GenerationStamp {
                            resource: crate::command_guard::GenerationResource::Speech,
                            value: generation,
                        },
                        text,
                        UserMessageAttachment::None,
                    ),
                };
                let handler_state = state.clone();
                result.value = Some(
                    state
                        .dispatch_with_fence(source, command, fence, move |context| async move {
                            handler_state
                                .command_enqueue_user_message(
                                    &context,
                                    message,
                                    Vec::new(),
                                    attachment,
                                )
                                .await
                                .map_err(crate::command_guard::DispatchError::handler)
                        })
                        .await
                        .map_err(|error| {
                            error.format_for_locale(Locale::from_config(
                                &state.runtime_config().ui.language,
                            ))
                        })?,
                );
            }
            UiTask::SubmitChat(message) => {
                let handler_state = state.clone();
                result.value = Some(
                    state
                        .dispatch(
                            CommandSource::IpcMain,
                            DesktopCommand::ChatSend,
                            move |context| async move {
                                let locale = Locale::from_config(
                                    &handler_state.runtime_config().ui.language,
                                );
                                if message.trim().is_empty() {
                                    return Err(crate::command_guard::DispatchError::handler(
                                        text(TextKey::MessageEmpty, locale),
                                    ));
                                }
                                if message.len() > crate::commands::MAX_CHAT_BYTES {
                                    return Err(crate::command_guard::DispatchError::handler(
                                        text(TextKey::MessageTooLong, locale),
                                    ));
                                }
                                handler_state
                                    .command_enqueue_user_message(
                                        &context,
                                        message,
                                        Vec::new(),
                                        crate::state::user_input::UserMessageAttachment::None,
                                    )
                                    .await
                                    .map_err(crate::command_guard::DispatchError::handler)
                            },
                        )
                        .await
                        .map_err(|error| {
                            error.format_for_locale(Locale::from_config(
                                &state.runtime_config().ui.language,
                            ))
                        })?,
                );
            }
            UiTask::InterruptCapture(selection_only) => {
                state.capture.interrupt(selection_only).await?;
            }
            UiTask::NativeShutdown(kind) => {
                state
                    .app
                    .try_state::<Arc<crate::shutdown::ShutdownCoordinator>>()
                    .ok_or("終了処理が初期化されていません")?
                    .request(kind);
            }
            UiTask::Shutdown => {
                state
                    .app
                    .try_state::<Arc<crate::shutdown::ShutdownCoordinator>>()
                    .ok_or("終了処理が初期化されていません")?
                    .request(crate::shutdown::ExitKind::Application);
            }
            UiTask::Load {
                view,
                generation,
                request,
            } => {
                let loaded = crate::ui_load::load(state, request).await;
                result.events.push(UiEvent::Window {
                    view,
                    event: crate::presentation::PresentationEvent::Loaded {
                        generation,
                        result: loaded,
                    },
                });
            }
            UiTask::Refresh(view) => {
                let loaded = crate::ui_load::refresh(state, view).await;
                result.events.push(UiEvent::Refreshed {
                    view,
                    result: loaded,
                });
            }
        }
        Ok(result)
    }
}

impl NativeUiPort {
    fn render_window(&self, content: crate::ui_load::WindowContent) -> Result<(), String> {
        use crate::ui_load::WindowContent;
        let app = &self.state.app;
        match content {
            WindowContent::Main(content) => {
                crate::webview_event::emit_to(
                    app,
                    "main",
                    "coosenpai:personas:load",
                    &content.personas,
                )
                .map_err(|error| error.to_string())?;
                self.render_window_snapshot("main", &content.snapshot)
            }
            WindowContent::Settings { resources, .. } => crate::webview_event::emit_to(
                app,
                "main",
                "coosenpai:settings:resources",
                &resources,
            )
            .map_err(|error| error.to_string()),
            WindowContent::Details { snapshot, history } => {
                crate::webview_event::emit_to(app, "details", "coosenpai:dataflow:load", &history)
                    .map_err(|error| error.to_string())?;
                self.render_window_snapshot("details", &snapshot)
            }
            WindowContent::ModelPicker { snapshot, catalog } => {
                crate::webview_event::emit_to(
                    app,
                    "model-popup",
                    "coosenpai:model-catalog:load",
                    &catalog,
                )
                .map_err(|error| error.to_string())?;
                self.render_window_snapshot("model-popup", &snapshot)
            }
        }
    }

    fn render_window_snapshot(
        &self,
        label: &str,
        snapshot: &crate::snapshot::AppSnapshot,
    ) -> Result<(), String> {
        crate::webview_event::emit_to(
            &self.state.app,
            label,
            "coosenpai:snapshot:updated",
            &crate::snapshot::SnapshotEvent {
                revision: snapshot.revision,
                snapshot: snapshot.clone(),
            },
        )
        .map_err(|error| error.to_string())
    }

    async fn apply_view(&self, view: PresenterId, command: ViewCommand) -> Result<(), String> {
        let state = &self.state;
        match (view, command) {
            (PresenterId::Bubble, ViewCommand::Hide) => {
                crate::webview_event::emit_to(&state.app, "bubble", "coosenpai:bubble:clear", &())
                    .map_err(|error| error.to_string())?;
                state
                    .app
                    .get_webview_window("bubble")
                    .ok_or("吹き出しがありません")?
                    .hide()
                    .map_err(|error| error.to_string())
            }
            (PresenterId::Bubble, ViewCommand::Show) => {
                let window = state
                    .app
                    .get_webview_window("bubble")
                    .ok_or("吹き出しがありません")?;
                crate::windows::window_focus::present_window(&window).await
            }
            (PresenterId::Bubble, ViewCommand::Front) => {
                let window = state
                    .app
                    .get_webview_window("bubble")
                    .ok_or("吹き出しがありません")?;
                let main = state
                    .app
                    .get_webview_window("main")
                    .ok_or("メイン画面がありません")?;
                window
                    .set_focusable(true)
                    .map_err(|error| error.to_string())?;
                let focused = crate::windows::activate_and_focus_window(
                    &main,
                    &window,
                    state.bubble_focus_events(),
                    None,
                )
                .await
                .map_err(|error| error.message)?;
                if focused.focused {
                    Ok(())
                } else {
                    Err("吹き出しをキーボード操作できる状態にできません".into())
                }
            }
            (PresenterId::Chat, ViewCommand::Show | ViewCommand::Front) => {
                crate::windows::apply_main_show(&state.app).await
            }
            (PresenterId::Chat, ViewCommand::Hide) => crate::windows::apply_main_hide(&state.app),
            (PresenterId::Chat, ViewCommand::FocusInput) => state
                .app
                .emit_to("main", "coosenpai:composer:focus", ())
                .map_err(|error| error.to_string()),
            (PresenterId::Settings, ViewCommand::Show | ViewCommand::Front) => state
                .app
                .emit_to("main", "coosenpai:settings:requested", ())
                .map_err(|error| error.to_string()),
            (PresenterId::Settings, ViewCommand::Hide) => state
                .app
                .emit_to("main", "coosenpai:settings:closed", ())
                .map_err(|error| error.to_string()),
            (PresenterId::Details, ViewCommand::Show | ViewCommand::Front) => {
                crate::windows::show_details(&state.app).map_err(|error| error.to_string())
            }
            (PresenterId::ModelPicker, ViewCommand::Show | ViewCommand::Front) => {
                crate::windows::show_model_popup(&state.app).map_err(|error| error.to_string())
            }
            (view, command) => {
                let label = match view {
                    PresenterId::Bubble => "bubble",
                    PresenterId::Details => "details",
                    PresenterId::ModelPicker => "model-popup",
                    _ => return Err(format!("未対応のView指示: {view:?} {command:?}")),
                };
                let window = state
                    .app
                    .get_webview_window(label)
                    .ok_or_else(|| format!("ウインドウがありません: {label}"))?;
                match command {
                    ViewCommand::Show => window.show(),
                    ViewCommand::Hide => window.hide(),
                    ViewCommand::Front | ViewCommand::FocusInput => window.set_focus(),
                }
                .map_err(|error| error.to_string())
            }
        }
    }
}

pub(crate) fn render_chat<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    projection: crate::ui_events::ChatProjection,
) -> Result<(), String> {
    match projection {
        crate::ui_events::ChatProjection::Update(snapshot) => {
            app.emit_to("main", "coosenpai:update:changed", snapshot)
        }
        crate::ui_events::ChatProjection::VoiceOutput(snapshot) => {
            app.emit_to("main", "coosenpai:voice-output:changed", snapshot)
        }
    }
    .map_err(|error| error.to_string())
}

pub(crate) async fn render_os(
    notifier: &dyn coosenpai_core::ports::NotificationPort,
    record: &coosenpai_core::notification::NotificationRecord,
    guard: tokio::sync::OwnedMutexGuard<()>,
) -> bool {
    let accepted = notifier
        .show(
            &record.message,
            &record.priority,
            std::time::Duration::from_secs(5),
        )
        .await
        .is_ok();
    drop(guard);
    accepted
}

pub(crate) fn view_applied(
    view: PresenterId,
    command: ViewCommand,
    result: Result<(), String>,
) -> Result<EffectResult, String> {
    result?;
    let mut result = EffectResult::done();
    if view == PresenterId::Chat {
        match command {
            ViewCommand::Show | ViewCommand::Front => {
                result.events.push(UiEvent::MainVisibility(true))
            }
            ViewCommand::Hide => result.events.push(UiEvent::MainVisibility(false)),
            _ => {}
        }
    }
    Ok(result)
}
