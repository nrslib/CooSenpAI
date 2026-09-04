use crate::command_guard::{
    CommandContext, CommandEnvelope, CommandSource, DesktopCommand, DispatchError,
    ExclusiveTransition, GenerationStamp, LifecyclePhase, ManagerState, PolicyContext,
    RejectReason, ResourcePhase, ResourcePhases,
};
use crate::state::DesktopState;
use std::future::Future;
use std::sync::Arc;

impl DesktopState {
    pub(crate) async fn dispatch<T, F, Fut>(
        self: &Arc<Self>,
        source: CommandSource,
        command: DesktopCommand,
        handler: F,
    ) -> Result<T, DispatchError>
    where
        F: FnOnce(CommandContext) -> Fut,
        Fut: Future<Output = Result<T, DispatchError>>,
    {
        self.command_firewall
            .execute(self, CommandEnvelope::new(source, command), handler)
            .await
    }

    pub(crate) async fn dispatch_with_fence<T, F, Fut>(
        self: &Arc<Self>,
        source: CommandSource,
        command: DesktopCommand,
        fence: GenerationStamp,
        handler: F,
    ) -> Result<T, DispatchError>
    where
        F: FnOnce(CommandContext) -> Fut,
        Fut: Future<Output = Result<T, DispatchError>>,
    {
        self.command_firewall
            .execute(
                self,
                CommandEnvelope::new(source, command).with_fence(fence),
                handler,
            )
            .await
    }

    pub(crate) async fn dispatch_with_fences<T, F, Fut>(
        self: &Arc<Self>,
        source: CommandSource,
        command: DesktopCommand,
        fences: impl IntoIterator<Item = GenerationStamp>,
        handler: F,
    ) -> Result<T, DispatchError>
    where
        F: FnOnce(CommandContext) -> Fut,
        Fut: Future<Output = Result<T, DispatchError>>,
    {
        let mut envelope = CommandEnvelope::new(source, command);
        for fence in fences {
            envelope = envelope.with_fence(fence);
        }
        self.command_firewall.execute(self, envelope, handler).await
    }

    pub(crate) async fn dispatch_watch_start(
        self: &Arc<Self>,
        source: CommandSource,
    ) -> Result<crate::snapshot::AppSnapshot, DispatchError> {
        self.dispatch_watch_start_internal(source, true, true).await
    }

    pub(crate) async fn dispatch_watch_restore(
        self: &Arc<Self>,
    ) -> Result<crate::snapshot::AppSnapshot, DispatchError> {
        self.dispatch_watch_start_internal(CommandSource::Startup, false, false)
            .await
    }

    async fn dispatch_watch_start_internal(
        self: &Arc<Self>,
        source: CommandSource,
        persist_enabled: bool,
        show_rejection: bool,
    ) -> Result<crate::snapshot::AppSnapshot, DispatchError> {
        let handler_state = self.clone();
        let intent = match self
            .dispatch(
                source,
                DesktopCommand::WatchStart,
                move |context| async move {
                    if persist_enabled {
                        handler_state
                            .command_start_watch(&context)
                            .await
                            .map_err(DispatchError::handler)
                    } else {
                        handler_state
                            .command_begin_watch_start(&context)
                            .await
                            .map_err(DispatchError::handler)
                    }
                },
            )
            .await
        {
            Ok(intent) => intent,
            Err(error @ DispatchError::Rejected(_)) if show_rejection => {
                self.show_watch_start_rejection(&error.format_for_user())
                    .await;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        self.complete_dispatched_watch_start(intent).await
    }

    pub(crate) async fn dispatch_watch_fullscreen_consent(
        self: &Arc<Self>,
        source: CommandSource,
        bubble_id: String,
    ) -> Result<crate::snapshot::AppSnapshot, DispatchError> {
        let handler_state = self.clone();
        let intent = self
            .dispatch(
                source,
                DesktopCommand::ConfigWatchUpdate,
                move |context| async move {
                    handler_state
                        .command_accept_watch_fullscreen_consent(&context, &bubble_id)
                        .await
                        .map_err(DispatchError::handler)
                },
            )
            .await?;
        self.complete_dispatched_watch_start(intent).await
    }

    async fn complete_dispatched_watch_start(
        self: &Arc<Self>,
        intent: Option<crate::state::WatchStartIntent>,
    ) -> Result<crate::snapshot::AppSnapshot, DispatchError> {
        let snapshot = match intent {
            Some(intent) => self
                .complete_watch_start(intent)
                .await
                .map_err(DispatchError::handler)?,
            None => self.snapshot().await,
        };
        Ok(snapshot)
    }

    pub(crate) async fn dispatch_watch_toggle(
        self: &Arc<Self>,
        source: CommandSource,
    ) -> Result<crate::snapshot::AppSnapshot, DispatchError> {
        if !self.watch_intent_active().await {
            return self.dispatch_watch_start(source).await;
        }
        let handler_state = self.clone();
        self.dispatch(
            source,
            DesktopCommand::WatchStop,
            move |context| async move {
                handler_state
                    .command_stop_watch(&context)
                    .await
                    .map_err(DispatchError::handler)
            },
        )
        .await
    }

    pub(crate) fn ensure_command_generation(
        &self,
        stamp: GenerationStamp,
    ) -> Result<(), DispatchError> {
        if self.command_firewall.generation_is_current(stamp) {
            Ok(())
        } else {
            Err(DispatchError::Rejected(RejectReason::StaleGeneration))
        }
    }

    pub(crate) async fn prepare_command_execution(
        &self,
        command: DesktopCommand,
        _context: &CommandContext,
    ) {
        match command {
            DesktopCommand::ConversationReset | DesktopCommand::TutorialFinish => {
                crate::capture::cancel(self, _context).await;
                self.cancel_speech_and_wait().await;
            }
            DesktopCommand::ChatSend
            | DesktopCommand::ChatCancel
            | DesktopCommand::ChatRetry
            | DesktopCommand::CaptureStartImage
            | DesktopCommand::CaptureStartText
            | DesktopCommand::CaptureSendImage
            | DesktopCommand::CaptureSendText
            | DesktopCommand::CaptureCancel
            | DesktopCommand::SpeechStart
            | DesktopCommand::SpeechFinish
            | DesktopCommand::SpeechCancel
            | DesktopCommand::SpeechConfirm
            | DesktopCommand::ConfigDisplayUpdate
            | DesktopCommand::ConfigProviderUpdate
            | DesktopCommand::ProviderApiKeyUpdate
            | DesktopCommand::ConfigWatchUpdate
            | DesktopCommand::ConfigKeymapUpdate
            | DesktopCommand::WatchTargetUpdate
            | DesktopCommand::PersonaSelect
            | DesktopCommand::PersonaSave
            | DesktopCommand::PersonaDelete
            | DesktopCommand::PersonaRestore
            | DesktopCommand::PersonaReload
            | DesktopCommand::MemoryConfirm
            | DesktopCommand::MemoryReject
            | DesktopCommand::MemoryConfirmUpdate
            | DesktopCommand::MemoryRejectUpdate
            | DesktopCommand::MemoryDelete
            | DesktopCommand::MemoryConsolidate
            | DesktopCommand::ConversationResetDismiss
            | DesktopCommand::BubbleDismiss
            | DesktopCommand::TutorialInteract
            | DesktopCommand::TutorialFastForward
            | DesktopCommand::SettingsAppearancePreview
            | DesktopCommand::TutorialAdvance
            | DesktopCommand::TutorialSettingsPresented
            | DesktopCommand::TutorialResume
            | DesktopCommand::TutorialRestart
            | DesktopCommand::SetupPrompt
            | DesktopCommand::SetupRestart
            | DesktopCommand::SettingsOpen
            | DesktopCommand::WatchStart
            | DesktopCommand::WatchStop
            | DesktopCommand::WatchPowerSuspend
            | DesktopCommand::WatchPowerResume
            | DesktopCommand::PresentTutorialResponse => {}
            DesktopCommand::CompanionPresence | DesktopCommand::CopyLastReply => {}
        }
    }

    pub(crate) async fn command_policy_context(&self) -> PolicyContext {
        let onboarding = self.onboarding_policy_phase().await;
        let capture = match &*self.capture_popup_read().await {
            crate::capture::CapturePopupState::Idle => ResourcePhase::Idle,
            crate::capture::CapturePopupState::Ready(_) => ResourcePhase::Active,
            crate::capture::CapturePopupState::Capturing { .. }
            | crate::capture::CapturePopupState::Sending { .. } => ResourcePhase::Transitioning,
        };
        let watch = self.watch_resource_phase().await;
        PolicyContext {
            manager: ManagerState {
                lifecycle: if self.is_shutting_down() {
                    LifecyclePhase::ShuttingDown
                } else {
                    LifecyclePhase::Running
                },
                onboarding,
                transition: self
                    .command_firewall
                    .transition()
                    .map(ExclusiveTransition::InProgress)
                    .unwrap_or(ExclusiveTransition::Idle),
                resources: ResourcePhases {
                    runtime_available: self.is_runtime_active(),
                    speech: self.speech_resource_phase(),
                    capture,
                    watch,
                },
            },
        }
    }
}
