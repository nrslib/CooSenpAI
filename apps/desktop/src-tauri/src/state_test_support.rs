use super::{DesktopState, WatchLifecycle};
use crate::tutorial::SetupAttempt;
use coosenpai_core::companion_storage::CompanionStorage;
use coosenpai_core::onboarding::TutorialStep;
use coosenpai_core::ports::{SpeechPermissionPort, SpeechPort};
use coosenpai_core::state::{ConversationEntry, ConversationRole};
use tokio_util::sync::CancellationToken;

#[allow(dead_code)]
impl DesktopState {
    pub(crate) fn install_speech_ports_for_test(
        &self,
        speech_port: std::sync::Arc<dyn SpeechPort>,
        permission_port: std::sync::Arc<dyn SpeechPermissionPort>,
    ) {
        self.speech
            .install_ports_for_test(speech_port, permission_port);
    }

    pub(crate) fn disable_speech_shortcut_refresh_for_test(&self) {
        self.speech.disable_shortcut_refresh_for_test();
    }

    pub(crate) async fn test_prepare_tutorial_finish(&self) -> anyhow::Result<()> {
        Ok(self.tutorial.lock().await.prepare_finish()?)
    }

    pub(crate) async fn test_begin_setup_connection(
        &self,
        provider_name: &str,
        status_key: &str,
    ) -> anyhow::Result<(SetupAttempt, String, String)> {
        let mut tutorial = self.tutorial.lock().await;
        let provider = tutorial
            .provider()
            .ok_or_else(|| anyhow::anyhow!("tutorial provider is unavailable"))?;
        let status = provider.render(status_key)?;
        let intro = provider.render("setup-intro")?;
        let attempt = tutorial.begin_setup_connection(provider_name, &self.cancellation)?;
        Ok((attempt, intro, status))
    }

    pub(crate) async fn test_setup_attempt_is_current(&self, attempt: &SetupAttempt) -> bool {
        self.tutorial.lock().await.setup_attempt_is_current(attempt)
    }

    pub(crate) async fn test_complete_setup_state(&self) -> anyhow::Result<()> {
        let mut tutorial = self.tutorial.lock().await;
        let provider = tutorial
            .provider()
            .ok_or_else(|| anyhow::anyhow!("tutorial provider is unavailable"))?;
        tutorial.start(provider)?;
        Ok(())
    }

    pub(crate) async fn test_install_permission_waiting_watch_start(
        &self,
        generation: u64,
    ) -> CancellationToken {
        let cancellation = CancellationToken::new();
        {
            let mut control = self.watch_control.lock().await;
            control.generation = generation;
            control.lifecycle = WatchLifecycle::Starting {
                generation,
                cancellation: cancellation.clone(),
            };
        }
        cancellation
    }

    pub(crate) async fn test_install_watch_start_commit_barrier(
        &self,
    ) -> (
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    ) {
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        self.watch_control.lock().await.start_commit_barrier =
            Some((entered.clone(), release.clone()));
        (entered, release)
    }

    pub(crate) async fn test_commit_installed_watch_start(
        self: &std::sync::Arc<Self>,
        generation: u64,
        cancellation: &CancellationToken,
    ) -> anyhow::Result<crate::snapshot::AppSnapshot> {
        self.commit_watch_start(generation, cancellation, || {
            Ok(crate::watch::WatchTask::pending_for_test())
        })
        .await
    }

    pub(crate) async fn test_watch_is_stopped(&self) -> bool {
        matches!(
            self.watch_control.lock().await.lifecycle,
            WatchLifecycle::Stopped
        )
    }

    pub(crate) async fn test_watch_start_is_alive(&self) -> bool {
        matches!(
            &self.watch_control.lock().await.lifecycle,
            WatchLifecycle::Starting { cancellation, .. } if !cancellation.is_cancelled()
        )
    }

    pub(crate) async fn test_advance_tutorial_to_watch(&self) -> anyhow::Result<()> {
        let mut tutorial = self.tutorial.lock().await;
        for step in [
            TutorialStep::Chat,
            TutorialStep::Text,
            TutorialStep::Image,
            TutorialStep::Voice,
            TutorialStep::Persona,
        ] {
            tutorial.finish_step(step, true)?;
        }
        drop(tutorial);
        self.publish_tutorial_state().await;
        Ok(())
    }

    pub(crate) async fn test_finish_tutorial_step(
        &self,
        step: TutorialStep,
        skipped: bool,
    ) -> anyhow::Result<()> {
        self.tutorial.lock().await.finish_step(step, skipped)?;
        self.publish_tutorial_state().await;
        Ok(())
    }

    pub(crate) async fn test_store_current_tutorial_response(
        &self,
        entry_id: &str,
    ) -> anyhow::Result<String> {
        let message = self
            .tutorial
            .lock()
            .await
            .expected_response_message()
            .ok_or_else(|| anyhow::anyhow!("current tutorial step has no response"))?;
        let storage = CompanionStorage::from_paths(
            &self.paths,
            self.runtime_config().retention.conversation_days,
        );
        storage.append_conversation_once_at(
            &ConversationEntry {
                schema_version: 1,
                id: entry_id.to_owned(),
                created_at: chrono::Utc::now().to_rfc3339(),
                role: ConversationRole::Companion,
                message: message.clone(),
                attachment_path: None,
                attachment_text: None,
                tutorial_response_key: None,
                screen_context: None,
                caused_by_ids: vec![format!("{entry_id}-input")],
                notification_priority: "none".to_owned(),
            },
            chrono::Utc::now(),
        )?;
        self.refresh_conversation().await;
        Ok(message)
    }

    pub(crate) fn test_pause_watch_stop_before_permit(
        &self,
    ) -> (
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<tokio::sync::Notify>,
    ) {
        self.command_firewall.test_pause_watch_stop_before_permit()
    }
}
