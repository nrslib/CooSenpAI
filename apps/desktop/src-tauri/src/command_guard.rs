pub(crate) use crate::command_policy::{admit_command, permit_class, PermitClass};
use crate::command_source_policy::source_allows;
pub(crate) use crate::command_types::*;
use crate::state::DesktopState;
use coosenpai_core::ports::RuntimeLogger;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use tokio::sync::Notify;
use tokio::sync::RwLock;

tokio::task_local! {
    static DISPATCH_ACTIVE: ();
}

struct ResourceGenerations {
    changed: Notify,
    conversation: AtomicU64,
    speech: AtomicU64,
    bubble: AtomicU64,
    config: AtomicU64,
    finish: AtomicU64,
    watch: AtomicU64,
}

impl Default for ResourceGenerations {
    fn default() -> Self {
        Self {
            changed: Notify::new(),
            conversation: AtomicU64::new(0),
            speech: AtomicU64::new(0),
            bubble: AtomicU64::new(0),
            config: AtomicU64::new(0),
            finish: AtomicU64::new(0),
            watch: AtomicU64::new(0),
        }
    }
}

impl ResourceGenerations {
    fn current(&self, resource: GenerationResource) -> u64 {
        self.atomic(resource).load(Ordering::Acquire)
    }

    fn stamp(&self, resource: GenerationResource) -> GenerationStamp {
        GenerationStamp {
            resource,
            value: self.current(resource),
        }
    }

    fn bump(&self, resource: GenerationResource) -> GenerationStamp {
        let stamp = GenerationStamp {
            resource,
            value: self.atomic(resource).fetch_add(1, Ordering::AcqRel) + 1,
        };
        self.changed.notify_waiters();
        stamp
    }

    fn is_current(&self, stamp: GenerationStamp) -> bool {
        self.current(stamp.resource) == stamp.value
    }

    fn atomic(&self, resource: GenerationResource) -> &AtomicU64 {
        match resource {
            GenerationResource::Conversation => &self.conversation,
            GenerationResource::Speech => &self.speech,
            GenerationResource::Bubble => &self.bubble,
            GenerationResource::Config => &self.config,
            GenerationResource::Finish => &self.finish,
            GenerationResource::Watch => &self.watch,
        }
    }
}

pub(crate) struct CommandContext {
    command_id: String,
    command: DesktopCommand,
    reservation: Reservation,
    fences: GenerationFences,
}

impl CommandContext {
    pub(crate) fn command(&self) -> DesktopCommand {
        self.command
    }

    pub(crate) fn command_id(&self) -> &str {
        &self.command_id
    }

    pub(crate) fn fence(&self, resource: GenerationResource) -> Option<GenerationStamp> {
        self.fences.get(resource)
    }

    pub(crate) fn completion(&self) -> CompletionPoint {
        self.reservation.completion
    }

    pub(crate) fn tutorial_response(&self) -> Option<&'static str> {
        self.reservation.tutorial_response
    }
}

pub(crate) struct CommandFirewall {
    pub(super) permit: RwLock<()>,
    audio_permit: RwLock<()>,
    transition: Mutex<Option<TransitionOperation>>,
    generations: ResourceGenerations,
    #[cfg(test)]
    watch_stop_before_permit: Mutex<Option<WatchStopTestBarrier>>,
}

#[cfg(test)]
struct WatchStopTestBarrier {
    reached: std::sync::Arc<Notify>,
    release: std::sync::Arc<Notify>,
}

impl Default for CommandFirewall {
    fn default() -> Self {
        Self {
            permit: RwLock::new(()),
            audio_permit: RwLock::new(()),
            transition: Mutex::new(None),
            generations: ResourceGenerations::default(),
            #[cfg(test)]
            watch_stop_before_permit: Mutex::new(None),
        }
    }
}

impl CommandFirewall {
    pub(crate) fn transition(&self) -> Option<TransitionOperation> {
        *lock(&self.transition)
    }

    pub(crate) fn generation_is_current(&self, stamp: GenerationStamp) -> bool {
        self.generations.is_current(stamp)
    }

    #[cfg(test)]
    pub(crate) fn test_pause_watch_stop_before_permit(
        &self,
    ) -> (std::sync::Arc<Notify>, std::sync::Arc<Notify>) {
        let reached = std::sync::Arc::new(Notify::new());
        let release = std::sync::Arc::new(Notify::new());
        *lock(&self.watch_stop_before_permit) = Some(WatchStopTestBarrier {
            reached: reached.clone(),
            release: release.clone(),
        });
        (reached, release)
    }

    pub(crate) async fn execute<T, F, Fut>(
        &self,
        state: &std::sync::Arc<DesktopState>,
        envelope: CommandEnvelope,
        handler: F,
    ) -> Result<T, DispatchError>
    where
        F: FnOnce(CommandContext) -> Fut,
        Fut: Future<Output = Result<T, DispatchError>>,
    {
        reject_dispatch_reentry()?;
        if envelope.command == DesktopCommand::SetupRestart
            && source_allows(envelope.source, envelope.command)
        {
            state.cancel_setup_attempt_before_restart().await;
        }
        let watch_stop_target = if envelope.command == DesktopCommand::WatchStop
            && source_allows(envelope.source, envelope.command)
        {
            state.cancel_pending_watch_start_intent().await
        } else {
            None
        };
        #[cfg(test)]
        self.pause_watch_stop_before_permit(watch_stop_target).await;
        DISPATCH_ACTIVE
            .scope((), async {
                match permit_class(envelope.command) {
                    PermitClass::Audio => {
                        let _permit = self.audio_permit.read().await;
                        self.execute_permitted(state, envelope, watch_stop_target, handler)
                            .await
                    }
                    PermitClass::Shared => {
                        let _permit =
                            self.read_permit(&envelope.expected)
                                .await
                                .map_err(|reason| {
                                    self.log_rejection(state, &envelope, reason);
                                    DispatchError::Rejected(reason)
                                })?;
                        self.execute_permitted(state, envelope, watch_stop_target, handler)
                            .await
                    }
                    PermitClass::Exclusive => {
                        let _audio = if envelope.command == DesktopCommand::ConfigProviderUpdate {
                            None
                        } else {
                            Some(self.audio_permit.write().await)
                        };
                        let _permit = self.permit.write().await;
                        self.execute_permitted(state, envelope, watch_stop_target, handler)
                            .await
                    }
                }
            })
            .await
    }

    async fn read_permit(
        &self,
        expected: &GenerationFences,
    ) -> Result<tokio::sync::RwLockReadGuard<'_, ()>, RejectReason> {
        loop {
            let changed = self.generations.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if expected
                .iter()
                .any(|stamp| !self.generations.is_current(stamp))
            {
                return Err(RejectReason::StaleGeneration);
            }
            tokio::select! {
                permit = self.permit.read() => return Ok(permit),
                () = &mut changed => {},
            }
        }
    }

    async fn execute_permitted<T, F, Fut>(
        &self,
        state: &std::sync::Arc<DesktopState>,
        envelope: CommandEnvelope,
        watch_stop_target: Option<GenerationStamp>,
        handler: F,
    ) -> Result<T, DispatchError>
    where
        F: FnOnce(CommandContext) -> Fut,
        Fut: Future<Output = Result<T, DispatchError>>,
    {
        let policy = state.command_policy_context().await;
        let (reservation, mut fences) = match admit_command(&policy, &envelope) {
            Admission::Accept {
                reservation,
                fences,
            } => (reservation, fences),
            Admission::Reject(reason) => {
                self.log_rejection(state, &envelope, reason);
                return Err(DispatchError::Rejected(reason));
            }
        };
        if envelope
            .expected
            .iter()
            .any(|stamp| !self.generations.is_current(stamp))
        {
            self.log_rejection(state, &envelope, RejectReason::StaleGeneration);
            return Err(DispatchError::Rejected(RejectReason::StaleGeneration));
        }
        for stamp in envelope.expected.iter() {
            fences.insert(stamp);
        }
        state.prepare_command_execution(envelope.command).await?;
        let _transition = reservation
            .transition
            .map(|operation| self.enter_transition(operation));
        for stamp in self
            .fences_for_with_state(state, envelope.command)
            .await
            .iter()
        {
            fences.insert(stamp);
        }
        if let Some(target) = watch_stop_target {
            fences.insert(target);
        }
        let context = CommandContext {
            command_id: envelope.command_id.clone(),
            command: envelope.command,
            reservation,
            fences,
        };
        let _ = state.logger.write(
            "INFO",
            &format!(
                "desktop commandを受理しました: command_id={} command={:?} source={:?} completion={:?}",
                context.command_id(), envelope.command, envelope.source, context.completion()
            ),
        );
        let result = handler(context).await;
        let level = if result.is_ok() { "INFO" } else { "WARN" };
        let outcome = if result.is_ok() {
            "completed"
        } else {
            "failed"
        };
        let _ = state.logger.write(
            level,
            &format!(
                "desktop commandを完了しました: command_id={} command={:?} outcome={outcome}",
                envelope.command_id, envelope.command
            ),
        );
        result
    }

    fn log_rejection(
        &self,
        state: &DesktopState,
        envelope: &CommandEnvelope,
        reason: RejectReason,
    ) {
        let _ = state.logger.write(
            "INFO",
            &format!(
                "desktop commandを拒否しました: command_id={} command={:?} source={:?} reason={reason:?}",
                envelope.command_id, envelope.command, envelope.source
            ),
        );
    }

    fn fences_for(&self, command: DesktopCommand) -> GenerationFences {
        let mut fences = GenerationFences::default();
        match command {
            DesktopCommand::ConversationReset | DesktopCommand::ConversationSelect => {
                self.generations.bump(GenerationResource::Bubble);
                self.generations.bump(GenerationResource::Speech);
                fences.insert(self.generations.bump(GenerationResource::Conversation));
            }
            DesktopCommand::TutorialFinish => {
                self.generations.bump(GenerationResource::Conversation);
                self.generations.bump(GenerationResource::Bubble);
                self.generations.bump(GenerationResource::Speech);
                self.generations.bump(GenerationResource::Watch);
                fences.insert(self.generations.bump(GenerationResource::Finish));
            }
            DesktopCommand::WorkConfigure
            | DesktopCommand::ConfigDisplayUpdate
            | DesktopCommand::ConfigAudioUpdate
            | DesktopCommand::ConfigProviderUpdate
            | DesktopCommand::ProviderApiKeyUpdate
            | DesktopCommand::ConfigKeymapUpdate
            | DesktopCommand::PersonaSelect
            | DesktopCommand::SetupPersonaSelect
            | DesktopCommand::PersonaSave
            | DesktopCommand::PersonaDelete
            | DesktopCommand::PersonaRestore
            | DesktopCommand::PersonaReload
            | DesktopCommand::SetupRestart
            | DesktopCommand::TutorialRestart
            | DesktopCommand::TutorialInteract => {
                fences.insert(self.generations.bump(GenerationResource::Config));
            }
            DesktopCommand::ConfigWatchUpdate | DesktopCommand::WatchTargetUpdate => {
                fences.insert(self.generations.bump(GenerationResource::Config));
                fences.insert(self.generations.bump(GenerationResource::Watch));
            }
            DesktopCommand::CaptureStartImage | DesktopCommand::CaptureStartText => {
                fences.insert(self.generations.stamp(GenerationResource::Conversation));
                fences.insert(self.generations.bump(GenerationResource::Speech));
            }
            DesktopCommand::SpeechStart | DesktopCommand::SpeechCancel => {
                fences.insert(self.generations.bump(GenerationResource::Speech));
            }
            DesktopCommand::SpeechConfirm => {
                fences.insert(self.generations.stamp(GenerationResource::Speech));
            }
            DesktopCommand::WatchStart => {
                fences.insert(self.generations.bump(GenerationResource::Watch));
            }
            DesktopCommand::WatchStop => {}
            DesktopCommand::WatchPowerSuspend | DesktopCommand::WatchPowerResume => {
                fences.insert(self.generations.stamp(GenerationResource::Watch));
            }
            DesktopCommand::PresentTutorialResponse => {
                fences.insert(self.generations.stamp(GenerationResource::Conversation));
                fences.insert(self.generations.stamp(GenerationResource::Bubble));
            }
            DesktopCommand::WorkApprove
            | DesktopCommand::ChatSend
            | DesktopCommand::ChatCancel
            | DesktopCommand::ChatRetry
            | DesktopCommand::CaptureSendImage
            | DesktopCommand::CaptureSendText
            | DesktopCommand::SpeechFinish
            | DesktopCommand::VoiceOutputTest
            | DesktopCommand::VoiceOutputStop
            | DesktopCommand::MemoryConfirm
            | DesktopCommand::MemoryReject
            | DesktopCommand::MemoryConfirmUpdate
            | DesktopCommand::MemoryRejectUpdate
            | DesktopCommand::MemoryDelete
            | DesktopCommand::MemoryConsolidate
            | DesktopCommand::ConversationResetDismiss
            | DesktopCommand::CompanionEmotionsReset
            | DesktopCommand::BubbleDismiss
            | DesktopCommand::BubbleFastForward
            | DesktopCommand::BubbleNavigate
            | DesktopCommand::SettingsAppearancePreview
            | DesktopCommand::TutorialAdvance
            | DesktopCommand::TutorialSettingsPresented
            | DesktopCommand::TutorialResume
            | DesktopCommand::SetupPrompt
            | DesktopCommand::SettingsOpen
            | DesktopCommand::LicenseDocumentOpen => {}
            DesktopCommand::CompanionPresence | DesktopCommand::CopyLastReply => {}
        }
        fences
    }

    async fn fences_for_with_state(
        &self,
        state: &DesktopState,
        command: DesktopCommand,
    ) -> GenerationFences {
        if command != DesktopCommand::SpeechStart {
            return self.fences_for(command);
        }
        let action = crate::input_popup::speech_start_action(state.input_popup_kind().await);
        let stamp = if action == crate::input_popup::InputPopupStartAction::Focus {
            self.generations.stamp(GenerationResource::Speech)
        } else {
            self.generations.bump(GenerationResource::Speech)
        };
        let mut fences = GenerationFences::default();
        fences.insert(stamp);
        fences
    }

    fn enter_transition(&self, operation: TransitionOperation) -> TransitionGuard<'_> {
        *lock(&self.transition) = Some(operation);
        TransitionGuard { firewall: self }
    }
}

fn reject_dispatch_reentry() -> Result<(), DispatchError> {
    if DISPATCH_ACTIVE.try_with(|()| ()).is_ok() {
        debug_assert!(false, "permit 保持中に desktop dispatcher へ再入しました");
        Err(DispatchError::Failed(
            "desktop command の再入を拒否しました".to_owned(),
        ))
    } else {
        Ok(())
    }
}

struct TransitionGuard<'a> {
    firewall: &'a CommandFirewall,
}

impl Drop for TransitionGuard<'_> {
    fn drop(&mut self) {
        *lock(&self.firewall.transition) = None;
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

