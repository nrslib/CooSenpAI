use super::*;
use crate::tutorial_lifecycle_presenter::{LifecycleEvent, LifecycleTask};
impl DesktopState {
    pub(crate) async fn run_tutorial_lifecycle_task(
        self: &Arc<Self>,
        task: LifecycleTask,
    ) -> LifecycleEvent {
        match task {
            LifecycleTask::Intro {
                follows_setup_ok,
                reopen,
                reply,
            } => {
                let result = async {
                    if reopen {
                        self.reopen_tutorial_notice("intro-click").await?;
                    }
                    self.emit_tutorial_intro_sequence(follows_setup_ok).await
                }
                .await;
                LifecycleEvent::Done { result, reply }
            }
            LifecycleTask::ResumeSettings(reply) => {
                let result = async {
                    self.tutorial_settings_opened().await?;
                    self.tutorial_settings_presented().await
                }
                .await;
                LifecycleEvent::Done { result, reply }
            }
            LifecycleTask::ResumeGuide { step, key, reply } => {
                let result = async {
                    self.reopen_tutorial_notice(key).await?;
                    self.emit_tutorial_message(key).await
                }
                .await;
                LifecycleEvent::GuidePresented {
                    step,
                    result,
                    reply,
                }
            }
            LifecycleTask::MainGuide(reply) => LifecycleEvent::MainGuidePresented {
                result: self.emit_tutorial_message("after-open").await,
                reply,
            },
            LifecycleTask::RecordMainGuide { accepted, reply } => {
                if !accepted {
                    self.tutorial.lock().await.chat_open_presentation_failed();
                }
                self.publish_tutorial_state().await;
                LifecycleEvent::MainGuideRecorded(reply)
            }
            LifecycleTask::ShowComplete(reply) => {
                crate::bubble_conversation::show_tutorial_complete(self.clone()).await;
                LifecycleEvent::CompletionShown(reply)
            }
        }
    }
    async fn reopen_tutorial_notice(&self, key: &str) -> Result<(), RuntimeError> {
        let mut tutorial = self.tutorial.lock().await;
        if tutorial.state().tutorial.notices.contains_key(key) {
            tutorial
                .reopen_notice_bubble(key)
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
        }
        Ok(())
    }
}
