use super::*;
use crate::tutorial_progress_presenter::{
    event, ProgressAction, ProgressEvent, ProgressResult, ProgressTask,
};

impl DesktopState {
    pub(crate) async fn run_tutorial_progress_task(
        self: &Arc<Self>,
        task: ProgressTask,
    ) -> ProgressEvent {
        let result = match task.action {
            ProgressAction::BeginSettings => {
                ProgressResult::Settings(self.tutorial.lock().await.begin_settings_presentation())
            }
            ProgressAction::CompleteSettings => {
                let result = self
                    .tutorial
                    .lock()
                    .await
                    .complete_settings_presentation()
                    .map_err(|error| RuntimeError::Factory(error.to_string()));
                self.publish_tutorial_state().await;
                ProgressResult::Done(result.map(|_| ()))
            }
            ProgressAction::FailSettings => {
                self.tutorial.lock().await.settings_presentation_failed();
                self.publish_tutorial_state().await;
                ProgressResult::Done(Ok(()))
            }
            ProgressAction::WatchSequence => {
                ProgressResult::Presented(self.emit_watch_intro_sequence().await)
            }
            ProgressAction::BeginWatch => ProgressResult::Watch(
                self.tutorial
                    .lock()
                    .await
                    .begin_watch_capture_presentation(),
            ),
            ProgressAction::PresentAfterWatch => {
                let ids = self
                    .tutorial_notice_ids(&["watch-intro"])
                    .await
                    .unwrap_or_default();
                ProgressResult::Presented(
                    self.emit_tutorial_message_replacing(
                        "after-watch",
                        ids,
                        self.cancellation.clone(),
                    )
                    .await,
                )
            }
            ProgressAction::FailWatch => {
                self.tutorial
                    .lock()
                    .await
                    .watch_capture_presentation_failed();
                ProgressResult::Done(Ok(()))
            }
            ProgressAction::Current => ProgressResult::Current(self.tutorial_current_step().await),
            ProgressAction::FinishWatch => ProgressResult::Done(
                self.finish_tutorial_step(coosenpai_core::onboarding::TutorialStep::Watch, false)
                    .await,
            ),

            ProgressAction::Finish { step, skipped } => {
                let result = self
                    .tutorial
                    .lock()
                    .await
                    .finish_step(step, skipped)
                    .map_err(|error| RuntimeError::Factory(error.to_string()));
                self.publish_tutorial_state().await;
                ProgressResult::Step(result)
            }
            ProgressAction::AcceptResponse { entry_id, message } => {
                let step = self
                    .tutorial
                    .lock()
                    .await
                    .response_presentation_accepted(&entry_id, &message);
                self.publish_tutorial_state().await;
                ProgressResult::Response(step)
            }
            ProgressAction::GuideDetails(key) => {
                let tutorial = self.tutorial.lock().await;
                let details = tutorial
                    .state()
                    .tutorial_notice_id(key)
                    .map_err(|error| error.to_string())
                    .and_then(|id| {
                        tutorial
                            .state()
                            .tutorial
                            .notices
                            .get(key)
                            .map(|notice| (id, notice.message.clone()))
                            .ok_or_else(|| {
                                format!("チュートリアル案内が prepare されていません: {key}")
                            })
                    });
                ProgressResult::Guide(details)
            }
            ProgressAction::Clear => ProgressResult::Cleared {
                cleared: crate::bubbles::clear_tutorial_progress(self).await,
                cancellation: self.cancellation.clone(),
            },
            ProgressAction::Read { notice_id, delay } => ProgressResult::Read(
                crate::bubbles::wait_for_reading(self, &notice_id, delay).await,
            ),
            ProgressAction::Permission(step) => {
                match step {
                    coosenpai_core::onboarding::TutorialStep::Image => {
                        let _ = self.request_screen_permission_for_watch().await;
                    }
                    coosenpai_core::onboarding::TutorialStep::Voice => {
                        self.request_speech_permissions().await
                    }
                    _ => {}
                }
                ProgressResult::Permission
            }
            ProgressAction::Present(key) => {
                ProgressResult::Presented(self.emit_tutorial_message(key).await)
            }
            ProgressAction::Advance {
                step,
                notice_id,
                guide,
            } => {
                let handler = self.clone();
                let result = self
                    .dispatch(
                        crate::command_guard::CommandSource::TutorialAutomation,
                        crate::command_guard::DesktopCommand::TutorialAdvance,
                        move |context| async move {
                            let current = handler.tutorial_current_step().await;
                            let presented = if guide {
                                handler
                                    .tutorial
                                    .lock()
                                    .await
                                    .guide_presentation_is_current(step, &notice_id)
                            } else {
                                handler
                                    .tutorial_response_presentation_is_current(step, &notice_id)
                                    .await
                            };
                            let ready = handler
                                .ui
                                .query(crate::ui_events::UiView::Application, |reply| {
                                    event(ProgressEvent::AdvanceChecked {
                                        step,
                                        current,
                                        presented,
                                        guide,
                                        reply,
                                    })
                                })
                                .await
                                .map_err(crate::command_guard::DispatchError::handler)?;
                            if !ready {
                                return Ok(false);
                            }
                            if guide {
                                handler
                                    .command_finish_tutorial_step_without_guide_auto_advance(
                                        &context, step, false,
                                    )
                                    .await
                            } else {
                                handler
                                    .command_finish_tutorial_step(&context, step, false)
                                    .await
                            }
                            .map(|()| true)
                            .map_err(crate::command_guard::DispatchError::handler)
                        },
                    )
                    .await
                    .map_err(|error| RuntimeError::Factory(error.to_string()));
                ProgressResult::Advanced(result)
            }
            ProgressAction::FinishTutorial => {
                let handler = self.clone();
                let result = self
                    .dispatch(
                        crate::command_guard::CommandSource::TutorialAutomation,
                        crate::command_guard::DesktopCommand::TutorialFinish,
                        move |context| async move {
                            handler
                                .command_finish_tutorial(
                                    &context,
                                    super::tutorial_state::TutorialFinishEntry::Automatic,
                                )
                                .await
                                .map_err(crate::command_guard::DispatchError::handler)
                        },
                    )
                    .await
                    .map_err(|error| RuntimeError::Factory(error.to_string()));
                ProgressResult::Done(result)
            }
        };
        ProgressEvent::Completed {
            id: task.id,
            result,
        }
    }
}
