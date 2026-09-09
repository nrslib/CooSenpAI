use super::*;
use crate::command_guard::{CommandSource, DesktopCommand, GenerationResource};
use crate::tutorial_response_presenter::{ResponseCompletion, ResponseEvent, ResponseTask};

impl DesktopState {
    pub(crate) async fn run_tutorial_response_task(
        self: &Arc<Self>,
        task: ResponseTask,
    ) -> ResponseEvent {
        match task {
            ResponseTask::Main { entry, reply } => {
                let handler = self.clone();
                let _ = self
                    .dispatch(
                        CommandSource::RuntimeMonitor,
                        DesktopCommand::PresentTutorialResponse,
                        move |_context| async move {
                            let started = handler
                                .tutorial
                                .lock()
                                .await
                                .begin_response_presentation(&entry.id, &entry.message);
                            if started {
                                handler
                                    .advance_after_tutorial_response(&entry.id, &entry.message)
                                    .await;
                            }
                            Ok(())
                        },
                    )
                    .await;
                ResponseEvent::Done(reply)
            }
            ResponseTask::Prepare {
                record,
                duration_ms,
                reply,
            } => {
                let handler = self.clone();
                let presentation = self
                    .dispatch(
                        CommandSource::RuntimeMonitor,
                        DesktopCommand::PresentTutorialResponse,
                        move |context| async move {
                            let started = handler
                                .tutorial
                                .lock()
                                .await
                                .begin_response_presentation(&record.id, &record.message);
                            if !started {
                                return Ok(None);
                            }
                            let entry_id = record.id.clone();
                            let message = record.message.clone();
                            let presentation =
                                crate::bubbles::register(&handler.ui, *record, duration_ms).await;
                            let Ok(presentation) = presentation else {
                                handler
                                    .tutorial
                                    .lock()
                                    .await
                                    .response_presentation_failed(&entry_id);
                                return Ok(None);
                            };
                            Ok(context
                                .fence(GenerationResource::Conversation)
                                .zip(context.fence(GenerationResource::Bubble))
                                .map(|(conversation, bubble)| {
                                    super::tutorial_state::TutorialPresentation {
                                        completion: ResponseCompletion {
                                            entry_id,
                                            message,
                                            conversation,
                                            bubble,
                                        },
                                        presentation,
                                    }
                                }))
                        },
                    )
                    .await
                    .ok()
                    .flatten();
                ResponseEvent::Prepared {
                    presentation,
                    reply,
                }
            }
            ResponseTask::Complete {
                presentation,
                reply,
            } => {
                let outcome =
                    crate::bubbles::complete_presentation(self.clone(), presentation.presentation)
                        .await
                        .map_err(|error| error.to_string());
                ResponseEvent::Presented {
                    completion: presentation.completion,
                    outcome,
                    reply,
                }
            }
            ResponseTask::Accept { completion, reply } => {
                let handler = self.clone();
                let _ = self
                    .dispatch_with_fences(
                        CommandSource::RuntimeMonitor,
                        DesktopCommand::PresentTutorialResponse,
                        [completion.conversation, completion.bubble],
                        move |_context| async move {
                            handler
                                .advance_after_tutorial_response(
                                    &completion.entry_id,
                                    &completion.message,
                                )
                                .await;
                            Ok(())
                        },
                    )
                    .await;
                ResponseEvent::Done(reply)
            }
            ResponseTask::Reject { completion, reply } => {
                let handler = self.clone();
                let _ = self
                    .dispatch_with_fences(
                        CommandSource::RuntimeMonitor,
                        DesktopCommand::PresentTutorialResponse,
                        [completion.conversation, completion.bubble],
                        move |_context| async move {
                            handler
                                .tutorial
                                .lock()
                                .await
                                .response_presentation_failed(&completion.entry_id);
                            Ok(())
                        },
                    )
                    .await;
                ResponseEvent::Done(reply)
            }
            ResponseTask::NotificationAccepted { id, message, reply } => {
                self.tutorial_response_presented(id, message).await;
                ResponseEvent::NotificationRecorded(reply)
            }
            ResponseTask::RefreshNotification {
                id,
                message,
                chat,
                reply,
            } => {
                self.refresh_conversation().await;
                ResponseEvent::NotificationRefreshed {
                    id,
                    message,
                    chat,
                    reply,
                }
            }
            ResponseTask::Poll => {
                self.present_pending_tutorial_response().await;
                ResponseEvent::Finished
            }
        }
    }
}
