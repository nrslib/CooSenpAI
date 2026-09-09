use super::*;
use crate::bubbles::{BubbleAction, BubbleInteraction};
use crate::watch_presenter::WatchResult;
use coosenpai_core::locale::{text, Locale, TextKey};

const WATCH_FULLSCREEN_CONSENT_ID: &str = "watch-fullscreen-consent";
const WATCH_FULLSCREEN_CONFIRM_ACTION: &str = "watch-fullscreen-confirm";
const WATCH_FULLSCREEN_SETTINGS_ACTION: &str = "watch-fullscreen-settings";
const WATCH_START_REJECTION_ID: &str = "watch-start-rejected";

pub(super) enum WatchLifecycle {
    Stopped,
    Starting {
        generation: u64,
        cancellation: CancellationToken,
    },
    Running {
        generation: u64,
        task: crate::watch::WatchTask,
    },
    Stopping,
}

pub(super) struct WatchControl {
    pub(super) lifecycle: WatchLifecycle,
    pub(super) generation: u64,
    pub(super) resume_after_power: bool,
    #[cfg(test)]
    pub(super) start_commit_barrier: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
}

pub(crate) struct WatchStartIntent {
    generation: u64,
    cancellation: CancellationToken,
}

impl WatchControl {
    fn finish(&mut self, generation: u64) -> bool {
        match &self.lifecycle {
            WatchLifecycle::Running {
                generation: current,
                ..
            } if *current == generation => {
                self.lifecycle = WatchLifecycle::Stopped;
                true
            }
            WatchLifecycle::Stopping => {
                self.lifecycle = WatchLifecycle::Stopped;
                true
            }
            WatchLifecycle::Stopped
            | WatchLifecycle::Starting { .. }
            | WatchLifecycle::Running { .. } => false,
        }
    }

    pub(super) fn can_start(&self) -> bool {
        matches!(self.lifecycle, WatchLifecycle::Stopped)
    }

    pub(super) fn intent_active(&self) -> bool {
        matches!(
            self.lifecycle,
            WatchLifecycle::Starting { .. } | WatchLifecycle::Running { .. }
        )
    }

    pub(super) fn request_power_suspend(&mut self) -> bool {
        if self.resume_after_power
            || !matches!(
                self.lifecycle,
                WatchLifecycle::Starting { .. } | WatchLifecycle::Running { .. }
            )
        {
            return false;
        }
        self.resume_after_power = true;
        true
    }

    pub(super) fn take_power_resume(&mut self) -> bool {
        std::mem::take(&mut self.resume_after_power)
    }

    pub(super) fn accepts_generation(&self, generation: u64) -> bool {
        matches!(
            &self.lifecycle,
            WatchLifecycle::Running { generation: current, .. } if *current == generation
        )
    }

    pub(super) fn cancel_pending_start(&mut self) -> bool {
        let cancellation = match &self.lifecycle {
            WatchLifecycle::Starting { cancellation, .. } => cancellation.clone(),
            WatchLifecycle::Stopped | WatchLifecycle::Running { .. } | WatchLifecycle::Stopping => {
                return false
            }
        };
        cancellation.cancel();
        self.lifecycle = WatchLifecycle::Stopped;
        true
    }

    fn stop_target_and_cancel_pending_start(&mut self) -> Option<(u64, bool)> {
        match &self.lifecycle {
            WatchLifecycle::Starting {
                generation,
                cancellation,
            } => {
                let generation = *generation;
                cancellation.cancel();
                self.lifecycle = WatchLifecycle::Stopped;
                Some((generation, true))
            }
            WatchLifecycle::Running { generation, .. } => Some((*generation, false)),
            WatchLifecycle::Stopped | WatchLifecycle::Stopping => None,
        }
    }
}

impl DesktopState {
    pub(crate) async fn watch_intent_active(&self) -> bool {
        self.watch_control.lock().await.intent_active()
    }

    pub(crate) async fn watch_resource_phase(&self) -> crate::command_guard::ResourcePhase {
        match &self.watch_control.lock().await.lifecycle {
            WatchLifecycle::Stopped => crate::command_guard::ResourcePhase::Idle,
            WatchLifecycle::Running { .. } => crate::command_guard::ResourcePhase::Active,
            WatchLifecycle::Starting { .. } | WatchLifecycle::Stopping => {
                crate::command_guard::ResourcePhase::Transitioning
            }
        }
    }

    async fn begin_watch_start(
        self: &Arc<Self>,
        generation: u64,
    ) -> Result<Option<WatchStartIntent>> {
        let config = self.runtime.config();
        if !watch_has_enabled_target(&config.watch) {
            self.show_watch_fullscreen_consent(&config).await;
            return Ok(None);
        }
        let cancellation = CancellationToken::new();
        {
            let mut control = self.watch_control.lock().await;
            let stamp = crate::command_guard::GenerationStamp {
                resource: crate::command_guard::GenerationResource::Watch,
                value: generation,
            };
            if self.ensure_command_generation(stamp).is_err() {
                return Ok(None);
            }
            if !control.can_start() {
                return Ok(None);
            }
            control.generation = generation;
            control.lifecycle = WatchLifecycle::Starting {
                generation,
                cancellation: cancellation.clone(),
            };
        }
        if watch_start_is_blocked_by_config(
            self.is_runtime_active(),
            self.snapshot().await.last_error.as_ref(),
        ) {
            self.finish_pending_watch_start(generation).await;
            self.show_watch_start_rejection(text(
                TextKey::WatchConfigFix,
                Locale::from_config(&config.ui.language),
            ))
            .await;
            anyhow::bail!(text(
                TextKey::WatchConfigInvalid,
                Locale::from_config(&config.ui.language),
            ))
        }
        Ok(Some(WatchStartIntent {
            generation,
            cancellation,
        }))
    }

    async fn show_watch_fullscreen_consent(self: &Arc<Self>, config: &Config) {
        let conversation_generation = self.bubbles.lock().await.conversation_generation();
        crate::bubbles::show_best_effort(
            self.clone(),
            watch_fullscreen_consent_record(config, conversation_generation),
            config.notification.bubble_duration_ms,
        )
        .await;
    }

    pub(crate) async fn command_accept_watch_fullscreen_consent(
        self: &Arc<Self>,
        permit: &crate::command_guard::CommandContext,
        bubble_id: &str,
    ) -> Result<Option<WatchStartIntent>, ConfigCommitError> {
        let _watch_intent = self.watch_intent_lock.lock().await;
        if !self.bubbles.lock().await.accepts_interaction(
            bubble_id,
            WATCH_FULLSCREEN_CONFIRM_ACTION,
            None,
        ) {
            return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                text(
                    TextKey::SetupExpired,
                    Locale::from_config(&self.runtime_config().ui.language),
                )
                .to_owned(),
            )));
        }
        self.command_update_config_with(permit, |mut config| {
            config.watch.enabled = true;
            config.watch.fullscreen = true;
            Ok(config)
        })
        .await?;
        crate::bubbles::complete_action(self, bubble_id).await;
        self.command_begin_watch_start(permit)
            .await
            .map_err(|error| ConfigCommitError::Runtime(RuntimeError::Factory(error.to_string())))
    }

    pub(crate) async fn complete_watch_start(
        self: &Arc<Self>,
        intent: WatchStartIntent,
    ) -> Result<AppSnapshot> {
        let WatchStartIntent {
            generation,
            cancellation,
        } = intent;
        let permission = self.request_screen_permission_for_watch().await;
        if cancellation.is_cancelled() {
            return Ok(self.snapshot().await);
        }
        let locale = Locale::from_config(&self.runtime_config().ui.language);
        let presentation = permission.presentation_for_locale(locale);
        if presentation.status != "granted" {
            self.finish_pending_watch_start(generation).await;
            let message = presentation
                .message
                .unwrap_or(text(TextKey::WatchPermissionRequired, locale));
            self.show_watch_start_rejection(message).await;
            anyhow::bail!("{message}")
        }
        let tutorial_active = self.tutorial_is_active().await;
        let result = self
            .commit_watch_start(generation, &cancellation, || {
                crate::watch::spawn(self.clone(), generation, tutorial_active)
            })
            .await;
        if result.is_err() {
            self.show_watch_start_rejection(text(
                TextKey::WatchStartFailed,
                Locale::from_config(&self.runtime_config().ui.language),
            ))
            .await;
        }
        result
    }

    pub(super) async fn commit_watch_start<F>(
        self: &Arc<Self>,
        generation: u64,
        cancellation: &CancellationToken,
        spawn: F,
    ) -> Result<AppSnapshot>
    where
        F: FnOnce() -> Result<crate::watch::WatchTask>,
    {
        let mut control = self.watch_control.lock().await;
        if !matches!(
            control.lifecycle,
            WatchLifecycle::Starting { generation: current, .. } if current == generation
        ) || cancellation.is_cancelled()
        {
            drop(control);
            return Ok(self.snapshot().await);
        }
        match spawn() {
            Ok(task) => {
                control.lifecycle = WatchLifecycle::Running { generation, task };
            }
            Err(error) => {
                control.lifecycle = WatchLifecycle::Stopped;
                return Err(error);
            }
        }
        #[cfg(test)]
        if let Some((entered, release)) = control.start_commit_barrier.take() {
            entered.notify_one();
            release.notified().await;
        }
        let _ = self.logger.write("INFO", "見守りを開始しました。");
        let snapshot = self
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Watch {
                generation,
                event: WatchResult::Started,
            })
            .await;
        drop(control);
        crate::bubbles::complete_action(self, WATCH_START_REJECTION_ID).await;
        Ok(snapshot)
    }

    async fn start_watch_with_generation(self: &Arc<Self>, generation: u64) -> Result<AppSnapshot> {
        match self.begin_watch_start(generation).await? {
            Some(intent) => self.complete_watch_start(intent).await,
            None => Ok(self.snapshot().await),
        }
    }

    async fn finish_pending_watch_start(&self, generation: u64) {
        let mut control = self.watch_control.lock().await;
        if matches!(control.lifecycle, WatchLifecycle::Starting { generation: current, .. } if current == generation)
        {
            control.lifecycle = WatchLifecycle::Stopped;
        }
    }

    pub(crate) async fn command_begin_watch_start(
        self: &Arc<Self>,
        permit: &crate::command_guard::CommandContext,
    ) -> Result<Option<WatchStartIntent>> {
        let generation = permit
            .fence(crate::command_guard::GenerationResource::Watch)
            .ok_or_else(|| {
                anyhow::anyhow!(text(
                    TextKey::WatchGenerationMissing,
                    Locale::from_config(&self.runtime_config().ui.language),
                ))
            })?
            .value;
        self.begin_watch_start(generation).await
    }

    pub(crate) async fn command_start_watch(
        self: &Arc<Self>,
        permit: &crate::command_guard::CommandContext,
    ) -> Result<Option<WatchStartIntent>, ConfigCommitError> {
        let _watch_intent = self.watch_intent_lock.lock().await;
        self.command_update_config_with(permit, |mut config| {
            config.watch.enabled = true;
            Ok(config)
        })
        .await?;
        self.command_begin_watch_start(permit)
            .await
            .map_err(|error| ConfigCommitError::Runtime(RuntimeError::Factory(error.to_string())))
    }

    pub(super) async fn start_watch(
        self: &Arc<Self>,
        permit: &crate::command_guard::CommandContext,
    ) -> Result<AppSnapshot> {
        match self.command_begin_watch_start(permit).await? {
            Some(intent) => self.complete_watch_start(intent).await,
            None => Ok(self.snapshot().await),
        }
    }

    pub(crate) async fn cancel_pending_watch_start_intent(
        &self,
    ) -> Option<crate::command_guard::GenerationStamp> {
        let (generation, _) = self
            .watch_control
            .lock()
            .await
            .stop_target_and_cancel_pending_start()?;
        Some(crate::command_guard::GenerationStamp {
            resource: crate::command_guard::GenerationResource::Watch,
            value: generation,
        })
    }

    pub(super) async fn stop_watch(
        &self,
        permit: &crate::command_guard::CommandContext,
    ) -> (AppSnapshot, bool) {
        let Some(target) = permit.fence(crate::command_guard::GenerationResource::Watch) else {
            return (self.snapshot().await, true);
        };
        self.stop_watch_generation(true, Some(target.value)).await
    }

    pub(super) async fn stop_watch_internal(&self, manual: bool) -> AppSnapshot {
        self.stop_watch_generation(manual, None).await.0
    }

    pub(super) async fn stop_watch_internal_and_wait(&self, manual: bool) -> AppSnapshot {
        self.stop_watch_generation_and_wait(manual, None).await.0
    }

    // 停止済み世代から遅れて届く投影は、新しい世代の表示を上書きさせない。
    pub(crate) async fn publish_watch_view(
        &self,
        generation: u64,
        event: crate::watch_presenter::WatchResult,
    ) {
        let control = self.watch_control.lock().await;
        if !control.accepts_generation(generation) {
            let _ = self.logger.write(
                "INFO",
                &format!(
                    "見守り: 世代の古い状態更新を破棄しました: error-type=watch-stale-generation generation={generation}"
                ),
            );
            return;
        }
        self.publish_event(crate::snapshot_presenter::SnapshotEvent::Watch { generation, event })
            .await;
    }

    // 停止は取消と Stopped 投影で完了とし、worker の回収はバックグラウンドに委ねる。
    async fn stop_watch_generation(
        &self,
        manual: bool,
        expected_generation: Option<u64>,
    ) -> (AppSnapshot, bool) {
        let mut control = self.watch_control.lock().await;
        if expected_generation.is_some_and(|expected| control.generation != expected) {
            drop(control);
            return (self.snapshot().await, false);
        }
        if manual {
            control.resume_after_power = false;
        }
        let task = if control.cancel_pending_start() {
            None
        } else {
            let previous = std::mem::replace(&mut control.lifecycle, WatchLifecycle::Stopped);
            match previous {
                WatchLifecycle::Running { task, .. } => Some(task),
                WatchLifecycle::Starting { .. } => {
                    unreachable!("pending start handled above")
                }
                WatchLifecycle::Stopped | WatchLifecycle::Stopping => {
                    control.lifecycle = previous;
                    None
                }
            }
        };
        if let Some(task) = task {
            task.cancel();
            tauri::async_runtime::spawn(async move { task.wait().await });
            let _ = self.logger.write("INFO", "見守りを停止しました。");
        }
        let snapshot = self
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Watch {
                generation: control.generation,
                event: WatchResult::Stopped,
            })
            .await;
        drop(control);
        (snapshot, true)
    }

    // 終了処理では worker の回収まで待つ。
    async fn stop_watch_generation_and_wait(
        &self,
        manual: bool,
        expected_generation: Option<u64>,
    ) -> (AppSnapshot, bool) {
        let (target_is_current, task, target_generation) = {
            let mut control = self.watch_control.lock().await;
            if expected_generation.is_some_and(|expected| control.generation != expected) {
                (false, None, None)
            } else {
                let target_generation = control.generation;
                if manual {
                    control.resume_after_power = false;
                }
                let task = if control.cancel_pending_start() {
                    None
                } else {
                    let previous =
                        std::mem::replace(&mut control.lifecycle, WatchLifecycle::Stopping);
                    match previous {
                        WatchLifecycle::Running { task, .. } => Some(task),
                        WatchLifecycle::Starting { .. } => {
                            unreachable!("pending start handled above")
                        }
                        WatchLifecycle::Stopped | WatchLifecycle::Stopping => {
                            control.lifecycle = previous;
                            None
                        }
                    }
                };
                (true, task, Some(target_generation))
            }
        };
        if !target_is_current {
            return (self.snapshot().await, false);
        }
        if let Some(task) = task {
            task.cancel();
            task.wait().await;
            let _ = self.logger.write("INFO", "見守りを停止しました。");
        }
        let mut control = self.watch_control.lock().await;
        if target_generation.is_some_and(|target| control.generation != target) {
            return (self.snapshot().await, false);
        }
        if matches!(control.lifecycle, WatchLifecycle::Stopping) {
            control.lifecycle = WatchLifecycle::Stopped;
        }
        let snapshot = self
            .publish_event(crate::snapshot_presenter::SnapshotEvent::Watch {
                generation: control.generation,
                event: WatchResult::Stopped,
            })
            .await;
        drop(control);
        (snapshot, true)
    }

    pub async fn watch_finished(&self, generation: u64, failed: bool) {
        let mut control = self.watch_control.lock().await;
        if control.finish(generation) {
            self.publish_event(crate::snapshot_presenter::SnapshotEvent::Watch {
                generation,
                event: WatchResult::Finished { failed },
            })
            .await;
        }
        drop(control);
    }

    pub(super) fn spawn_power_monitor(state: Arc<Self>) {
        tauri::async_runtime::spawn(async move {
            use coosenpai_core::ports::{PowerEvent, PowerEventPort};
            let mut events = match crate::platform::MacPowerEvents::new() {
                Ok(events) => events,
                Err(_) => {
                    let _ = state.logger.write(
                        "WARN",
                        "電源通知の購読に失敗しました: error-type=power-events",
                    );
                    return;
                }
            };
            loop {
                tokio::select! {
                    _ = state.cancellation.cancelled() => break,
                    event = events.next() => {
                        let Ok(Some(event)) = event else { break };
                        let (command, handler_state) = match event {
                            PowerEvent::Sleep | PowerEvent::Lock => (
                                crate::command_guard::DesktopCommand::WatchPowerSuspend,
                                state.clone(),
                            ),
                            PowerEvent::Wake | PowerEvent::Unlock => (
                                crate::command_guard::DesktopCommand::WatchPowerResume,
                                state.clone(),
                            ),
                        };
                        let _ = state.dispatch(
                            crate::command_guard::CommandSource::PowerEvent,
                            command,
                            move |context| async move {
                                if command == crate::command_guard::DesktopCommand::WatchPowerSuspend {
                                    handler_state.command_suspend_for_power(&context).await;
                                } else {
                                    handler_state.command_resume_after_power(&context).await;
                                }
                                Ok(())
                            },
                        ).await;
                    }
                }
            }
        });
    }

    pub(super) async fn suspend_for_power(&self) {
        if let Err(error) = self
            .ui
            .request(
                crate::ui_events::UiView::Application,
                crate::ui_events::UiEvent::InterruptCapture(true),
            )
            .await
        {
            let _ = self.logger.write("WARN", &error);
        }
        let (should_stop, generation) = {
            let mut control = self.watch_control.lock().await;
            (control.request_power_suspend(), control.generation)
        };
        if should_stop {
            self.stop_watch_internal(false).await;
            self.publish_event(crate::snapshot_presenter::SnapshotEvent::Watch {
                generation,
                event: WatchResult::Suspended,
            })
            .await;
        }
    }

    pub(super) async fn resume_after_power(
        self: &Arc<Self>,
        permit: &crate::command_guard::CommandContext,
    ) {
        let _watch_intent = self.watch_intent_lock.lock().await;
        let should_resume = {
            let mut control = self.watch_control.lock().await;
            control.take_power_resume()
        };
        if should_resume && self.runtime_config().watch.enabled {
            if let Some(stamp) = permit.fence(crate::command_guard::GenerationResource::Watch) {
                if self.ensure_command_generation(stamp).is_ok() {
                    let _ = self.start_watch_with_generation(stamp.value).await;
                }
            }
        }
    }

    pub(crate) async fn show_watch_start_rejection(self: &Arc<Self>, message: &str) {
        let config = self.runtime_config();
        let conversation_generation = self.bubbles.lock().await.conversation_generation();
        crate::bubbles::show_best_effort(
            self.clone(),
            BubbleRecord {
                id: WATCH_START_REJECTION_ID.to_owned(),
                created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                message: message.to_owned(),
                message_kind: "notice".to_owned(),
                notification_priority: "warning".to_owned(),
                caused_by: None,
                display_name: config.companion.display_name,
                persona: config.companion.persona,
                avatar_color: config.ui.avatar_color,
                conversation_generation,
                persistent: true,
                interaction: None,
            },
            config.notification.bubble_duration_ms,
        )
        .await;
    }
}

fn watch_start_is_blocked_by_config(
    runtime_active: bool,
    last_error: Option<&coosenpai_core::runtime::RuntimeLastError>,
) -> bool {
    !runtime_active && last_error.is_some_and(|error| error.kind == RuntimeErrorKind::Config)
}

fn watch_has_enabled_target(config: &coosenpai_core::config::WatchConfig) -> bool {
    config.fullscreen || config.apps.iter().any(|application| application.enabled)
}

fn watch_fullscreen_consent_record(config: &Config, conversation_generation: u64) -> BubbleRecord {
    watch_fullscreen_consent_record_for_locale(
        config,
        conversation_generation,
        Locale::from_config(&config.ui.language),
    )
}

fn watch_fullscreen_consent_record_for_locale(
    config: &Config,
    conversation_generation: u64,
    locale: Locale,
) -> BubbleRecord {
    BubbleRecord {
        id: WATCH_FULLSCREEN_CONSENT_ID.to_owned(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: text(TextKey::WatchFullscreenConsent, locale).to_owned(),
        message_kind: "notice".to_owned(),
        notification_priority: "none".to_owned(),
        caused_by: None,
        display_name: config.companion.display_name.clone(),
        persona: config.companion.persona.clone(),
        avatar_color: config.ui.avatar_color.clone(),
        conversation_generation,
        persistent: true,
        interaction: Some(BubbleInteraction {
            select: None,
            secret_input: None,
            actions: vec![
                BubbleAction {
                    id: WATCH_FULLSCREEN_CONFIRM_ACTION.to_owned(),
                    label: text(TextKey::CommonYes, locale).to_owned(),
                },
                BubbleAction {
                    id: WATCH_FULLSCREEN_SETTINGS_ACTION.to_owned(),
                    label: text(TextKey::CommonNo, locale).to_owned(),
                },
            ],
            detail: Some(text(TextKey::WatchFullscreenDetail, locale).to_owned()),
            technical_detail: None,
        }),
    }
}

