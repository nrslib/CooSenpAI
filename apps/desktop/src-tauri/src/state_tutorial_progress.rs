use super::*;
use crate::bubbles;
use crate::tutorial_notice;
use coosenpai_core::companion_storage::CompanionStorage;
use coosenpai_core::onboarding::TutorialStep;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TutorialResponseStatus {
    None,
    Pending,
    SavedForPresentation,
}

fn response_status_from_entries(
    conversation: &[coosenpai_core::state::ConversationEntry],
    expected: Option<&str>,
    pending: bool,
) -> TutorialResponseStatus {
    if expected.is_some_and(|expected| {
        conversation.iter().any(|entry| {
            entry.role == coosenpai_core::state::ConversationRole::Companion
                && !entry.caused_by_ids.is_empty()
                && entry.message == expected
        })
    }) {
        return TutorialResponseStatus::SavedForPresentation;
    }
    if pending {
        TutorialResponseStatus::Pending
    } else {
        TutorialResponseStatus::None
    }
}

impl DesktopState {
    pub(crate) async fn tutorial_response_status(
        &self,
        step: TutorialStep,
    ) -> Result<TutorialResponseStatus, RuntimeError> {
        let Some(response_key) = step.response_key() else {
            return Ok(TutorialResponseStatus::None);
        };
        let expected = self.tutorial.lock().await.expected_response_message();
        if expected.is_some() {
            let storage = CompanionStorage::from_paths(
                &self.paths,
                self.runtime.config().retention.conversation_days,
            );
            let conversation = storage
                .load_conversation()
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
            let status = response_status_from_entries(
                &conversation,
                expected.as_deref(),
                self.runtime.has_pending_tutorial_response(response_key)?,
            );
            if status != TutorialResponseStatus::None {
                return Ok(status);
            }
        }
        if self.runtime.has_pending_tutorial_response(response_key)? {
            return Ok(TutorialResponseStatus::Pending);
        }
        Ok(TutorialResponseStatus::None)
    }

    pub(super) async fn tutorial_response_presented(
        self: &Arc<Self>,
        entry_id: String,
        message: String,
    ) {
        let state = self.clone();
        let handler_state = state.clone();
        let _ = state
            .dispatch(
                crate::command_guard::CommandSource::RuntimeMonitor,
                crate::command_guard::DesktopCommand::PresentTutorialResponse,
                move |_context| async move {
                    handler_state
                        .advance_after_tutorial_response(&entry_id, &message)
                        .await;
                    Ok(())
                },
            )
            .await;
    }

    pub(super) async fn advance_after_tutorial_response(
        self: &Arc<Self>,
        entry_id: &str,
        message: &str,
    ) {
        let accepted_step = {
            self.tutorial
                .lock()
                .await
                .response_presentation_accepted(entry_id, message)
        };
        let Some(step) = accepted_step else {
            return;
        };
        self.publish_tutorial_state().await;
        if !tutorial_response_auto_advance_step(step) {
            return;
        }
        let state = self.clone();
        let delay = super::tutorial_sequence::tutorial_reading_delay(message);
        if step == TutorialStep::Chat {
            let _ = self.logger.write(
                "INFO",
                &format!(
                    "チュートリアルのチャット返答を表示しました: entry-id={entry_id} read-delay-ms={}",
                    delay.as_millis()
                ),
            );
        } else {
            let _ = self.logger.write(
                "INFO",
                &format!(
                    "チュートリアルの返答を表示しました: step={step:?} entry-id={entry_id} read-delay-ms={}",
                    delay.as_millis()
                ),
            );
        }
        let entry_id = entry_id.to_owned();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(delay).await;
            let handler_state = state.clone();
            let result = state
                .dispatch(
                    crate::command_guard::CommandSource::TutorialAutomation,
                    crate::command_guard::DesktopCommand::TutorialAdvance,
                    move |context| async move {
                        if !tutorial_response_auto_advance_ready(
                            step,
                            handler_state.tutorial_current_step().await,
                            handler_state
                                .tutorial_response_presentation_is_current(step, &entry_id)
                                .await,
                        ) {
                            return Ok(());
                        }
                        handler_state
                            .command_finish_tutorial_step(&context, step, false)
                            .await
                            .map_err(crate::command_guard::DispatchError::handler)
                    },
                )
                .await;
            if let Err(error) = result {
                if step == TutorialStep::Chat {
                    let _ = state.logger.write(
                        "WARN",
                        &format!(
                            "チュートリアルのチャット返答後に自動遷移できませんでした: error-type=tutorial-advance ({error})"
                        ),
                    );
                } else {
                    let _ = state.logger.write(
                        "WARN",
                        &format!(
                            "チュートリアルの返答後に自動遷移できませんでした: step={step:?} error-type=tutorial-advance ({error})"
                        ),
                    );
                }
            } else {
                if step == TutorialStep::Chat {
                    let _ = state.logger.write(
                        "INFO",
                        "チュートリアルのチャット返答を読み終え、コピー練習へ進みました",
                    );
                } else {
                    let _ = state.logger.write(
                        "INFO",
                        &format!(
                            "チュートリアルの返答を読み終え、次の案内へ進みました: step={step:?}"
                        ),
                    );
                }
            }
        });
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) async fn test_present_current_tutorial_response(
        self: &Arc<Self>,
        entry_id: &str,
    ) -> anyhow::Result<()> {
        let message = self
            .tutorial
            .lock()
            .await
            .expected_response_message()
            .ok_or_else(|| anyhow::anyhow!("current tutorial step has no response"))?;
        let should_present = self
            .tutorial
            .lock()
            .await
            .begin_response_presentation(entry_id, &message);
        if should_present {
            self.advance_after_tutorial_response(entry_id, &message)
                .await;
        } else {
            anyhow::ensure!(
                self.tutorial.lock().await.step_response_presented(),
                "tutorial response presentation could not begin"
            );
        }
        Ok(())
    }

    pub(super) fn schedule_tutorial_guide_auto_advance(self: &Arc<Self>, step: TutorialStep) {
        let Some(key) = tutorial_guide_intro_key(step) else {
            return;
        };
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            let (notice_id, message) = match state.tutorial_guide_auto_advance_details(key).await {
                Ok(details) => details,
                Err(error) => {
                    let _ = state.logger.write(
                        "WARN",
                        &format!(
                            "チュートリアル案内の自動進行を登録できませんでした: step={step:?} ({error})"
                        ),
                    );
                    return;
                }
            };
            let delay = super::tutorial_sequence::tutorial_reading_delay(&message);
            let _ = state.logger.write(
                "INFO",
                &format!(
                    "チュートリアルの案内を表示しました: step={step:?} notice-id={notice_id} read-delay-ms={}",
                    delay.as_millis()
                ),
            );
            tokio::time::sleep(delay).await;
            let handler_state = state.clone();
            let result = state
                .dispatch(
                    crate::command_guard::CommandSource::TutorialAutomation,
                    crate::command_guard::DesktopCommand::TutorialAdvance,
                    move |context| async move {
                        if !handler_state
                            .tutorial_guide_auto_advance_ready(step, &notice_id)
                            .await
                        {
                            return Ok(false);
                        }
                        handler_state
                            .command_finish_tutorial_step_without_guide_auto_advance(
                                &context, step, false,
                            )
                            .await
                            .map_err(crate::command_guard::DispatchError::handler)?;
                        Ok(true)
                    },
                )
                .await;
            match result {
                Ok(true) => {
                    let _ = state.logger.write(
                        "INFO",
                        &format!(
                            "チュートリアルの案内を読み終え、自動で次へ進みました: step={step:?}"
                        ),
                    );
                }
                Ok(false) => {}
                Err(error) => {
                    let _ = state.logger.write(
                        "WARN",
                        &format!(
                            "チュートリアルの案内後に自動遷移できませんでした: step={step:?} error-type=tutorial-advance ({error})"
                        ),
                    );
                }
            }
        });
    }

    async fn tutorial_guide_auto_advance_details(
        &self,
        key: &str,
    ) -> Result<(String, String), String> {
        let tutorial = self.tutorial.lock().await;
        let notice_id = tutorial
            .state()
            .tutorial_notice_id(key)
            .map_err(|error| error.to_string())?;
        let message = tutorial
            .state()
            .tutorial
            .notices
            .get(key)
            .map(|notice| notice.message.clone())
            .ok_or_else(|| format!("チュートリアル案内が prepare されていません: {key}"))?;
        Ok((notice_id, message))
    }

    async fn tutorial_guide_auto_advance_ready(&self, step: TutorialStep, notice_id: &str) -> bool {
        self.tutorial
            .lock()
            .await
            .guide_presentation_is_current(step, notice_id)
    }

    pub(super) async fn tutorial_settings_opened(self: &Arc<Self>) -> Result<bool, RuntimeError> {
        let highlight_requested = self.tutorial.lock().await.request_settings_highlight();
        if highlight_requested.is_some() {
            self.publish_tutorial_state().await;
        }
        Ok(true)
    }

    pub(super) async fn tutorial_settings_presented(self: &Arc<Self>) -> Result<(), RuntimeError> {
        let Some(highlight) = self.tutorial.lock().await.begin_settings_presentation() else {
            return Ok(());
        };
        if highlight == crate::tutorial::TutorialSettingsHighlight::Watch {
            self.tutorial
                .lock()
                .await
                .complete_settings_presentation()
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
            self.publish_tutorial_state().await;
            return Ok(());
        }
        let cleared = bubbles::clear_tutorial_progress(self).await;
        if cleared && !bubbles::wait_for_tutorial_bubble_transition(&self.cancellation).await {
            return Ok(());
        }
        match self.emit_watch_intro_sequence().await {
            Ok(tutorial_notice::TutorialBubbleOutcome::Acknowledged) => {}
            Ok(tutorial_notice::TutorialBubbleOutcome::Dismissed) => {
                self.tutorial.lock().await.settings_presentation_failed();
                self.publish_tutorial_state().await;
                return Ok(());
            }
            Err(error) => {
                self.tutorial.lock().await.settings_presentation_failed();
                self.publish_tutorial_state().await;
                return Err(error);
            }
        }
        self.tutorial
            .lock()
            .await
            .complete_settings_presentation()
            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        self.publish_tutorial_state().await;
        Ok(())
    }

    pub(super) async fn tutorial_watch_started(self: &Arc<Self>) -> Result<(), ConfigCommitError> {
        if !self
            .tutorial
            .lock()
            .await
            .begin_watch_capture_presentation()
        {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let watch_intro_ids = self
            .tutorial_notice_ids(&["watch-intro"])
            .await
            .unwrap_or_default();
        let outcome = self
            .emit_tutorial_message_replacing(
                "after-watch",
                watch_intro_ids,
                self.cancellation.clone(),
            )
            .await;
        if !matches!(
            outcome,
            Ok(tutorial_notice::TutorialBubbleOutcome::Acknowledged)
        ) {
            self.tutorial
                .lock()
                .await
                .watch_capture_presentation_failed();
            return outcome.map(|_| ()).map_err(ConfigCommitError::Runtime);
        }
        let message = self
            .tutorial
            .lock()
            .await
            .provider()
            .ok_or_else(|| RuntimeError::Factory("チュートリアルが開始されていません".to_owned()))?
            .render("after-watch")
            .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        tokio::time::sleep(super::tutorial_sequence::tutorial_reading_delay(&message)).await;
        if self.tutorial_current_step().await == Some(TutorialStep::Watch) {
            self.finish_tutorial_step(TutorialStep::Watch, false)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn finish_tutorial_step(
        self: &Arc<Self>,
        step: TutorialStep,
        skipped: bool,
    ) -> Result<(), RuntimeError> {
        let next = self.finish_tutorial_step_inner(step, skipped).await?;
        if let Some(next_step) = next {
            self.schedule_tutorial_guide_auto_advance(next_step);
        } else {
            self.schedule_automatic_tutorial_finish();
        }
        Ok(())
    }

    fn schedule_automatic_tutorial_finish(self: &Arc<Self>) {
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            let handler_state = state.clone();
            let _ = state
                .dispatch(
                    crate::command_guard::CommandSource::TutorialAutomation,
                    crate::command_guard::DesktopCommand::TutorialFinish,
                    move |context| async move {
                        handler_state
                            .command_finish_tutorial(
                                &context,
                                super::tutorial_state::TutorialFinishEntry::Automatic,
                            )
                            .await
                            .map_err(crate::command_guard::DispatchError::handler)
                    },
                )
                .await;
        });
    }

    pub(super) async fn finish_tutorial_step_without_guide_auto_advance(
        self: &Arc<Self>,
        step: TutorialStep,
        skipped: bool,
    ) -> Result<(), RuntimeError> {
        self.finish_tutorial_step_inner(step, skipped)
            .await
            .map(|_| ())
    }

    async fn finish_tutorial_step_inner(
        self: &Arc<Self>,
        step: TutorialStep,
        skipped: bool,
    ) -> Result<Option<TutorialStep>, RuntimeError> {
        let next = {
            let mut tutorial = self.tutorial.lock().await;
            tutorial
                .finish_step(step, skipped)
                .map_err(|error| RuntimeError::Factory(error.to_string()))?
        };
        self.publish_tutorial_state().await;
        let presentation = tutorial_step_presentation(step, skipped, next);
        let cleared = bubbles::clear_tutorial_progress(self).await;
        if presentation.next_intro.is_some()
            && cleared
            && !bubbles::wait_for_tutorial_bubble_transition(&self.cancellation).await
        {
            return Ok(next);
        }
        if presentation.hide_main {
            crate::windows::hide_main(&self.app);
        }
        if let Some(key) = presentation.next_intro {
            if next == Some(TutorialStep::Image) {
                let _ = self.request_screen_permission_for_watch().await;
            }
            if next == Some(TutorialStep::Voice) {
                self.request_speech_permissions().await;
            }
            self.emit_tutorial_message(key).await?;
        }
        Ok(next)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TutorialStepPresentation {
    hide_main: bool,
    next_intro: Option<&'static str>,
}

fn tutorial_step_presentation(
    completed: TutorialStep,
    skipped: bool,
    next: Option<TutorialStep>,
) -> TutorialStepPresentation {
    TutorialStepPresentation {
        hide_main: completed == TutorialStep::Chat && !skipped,
        next_intro: next.and_then(super::tutorial_state::step_intro_key),
    }
}

fn tutorial_response_auto_advance_step(step: TutorialStep) -> bool {
    matches!(
        step,
        TutorialStep::Chat | TutorialStep::Text | TutorialStep::Image | TutorialStep::Voice
    )
}

fn tutorial_guide_intro_key(step: TutorialStep) -> Option<&'static str> {
    match step {
        TutorialStep::Persona => Some("persona-intro"),
        TutorialStep::Chat
        | TutorialStep::Text
        | TutorialStep::Image
        | TutorialStep::Voice
        | TutorialStep::Watch => None,
    }
}

fn tutorial_response_auto_advance_ready(
    step: TutorialStep,
    current: Option<TutorialStep>,
    response_presented: bool,
) -> bool {
    tutorial_response_auto_advance_step(step) && current == Some(step) && response_presented
}

