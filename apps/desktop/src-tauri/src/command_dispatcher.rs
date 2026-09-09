use crate::command_guard::{
    CommandContext, CommandEnvelope, CommandSource, DesktopCommand, DispatchError,
    ExclusiveTransition, GenerationStamp, LifecyclePhase, ManagerState, PolicyContext,
    RejectReason, ResourcePhase, ResourcePhases,
};
use crate::state::DesktopState;
use coosenpai_core::locale::Locale;
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
                self.show_watch_start_rejection(
                    &error
                        .format_for_locale(Locale::from_config(&self.runtime_config().ui.language)),
                )
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
    ) -> Result<(), DispatchError> {
        if prepare_input_transition(&self.ui, command).await? {
            self.voice_output.stop().await;
        }
        Ok(())
    }

    pub(crate) async fn command_policy_context(&self) -> PolicyContext {
        let onboarding = self.onboarding_policy_phase().await;
        let capture = match self.capture.view().phase {
            crate::capture::CapturePhase::Idle => ResourcePhase::Idle,
            crate::capture::CapturePhase::Popup => ResourcePhase::Active,
            crate::capture::CapturePhase::Selecting
            | crate::capture::CapturePhase::Sending
            | crate::capture::CapturePhase::Closing
            | crate::capture::CapturePhase::CloseFailed => ResourcePhase::Transitioning,
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

// ライフサイクル変更は、Rootが現入力の終了を認めた後だけ続行する。
pub(crate) async fn prepare_input_transition(
    ui: &crate::ui_root::UiHandle,
    command: DesktopCommand,
) -> Result<bool, DispatchError> {
    if !matches!(
        command,
        DesktopCommand::ConversationReset
            | DesktopCommand::ConversationSelect
            | DesktopCommand::TutorialFinish
    ) {
        return Ok(false);
    }
    ui.request(
        crate::ui_events::UiView::Application,
        crate::ui_events::UiEvent::InterruptCapture(false),
    )
    .await
    .map_err(DispatchError::handler)?;
    Ok(true)
}
