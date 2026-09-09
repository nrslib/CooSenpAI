use super::*;
use crate::watch_presenter::WatchResult;

#[async_trait::async_trait]
impl WatchHost for DesktopState {
    type Worker = DesktopWatchWorker;

    fn logger(&self) -> &dyn RuntimeLogger {
        self.logger.as_ref()
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    fn frame_buffer(&self) -> FrameBuffer {
        FrameBuffer::new(self.paths.frame_buffer.clone())
    }

    async fn create_worker(
        self: Arc<Self>,
        generation: u64,
        cancellation: CancellationToken,
        publication: coosenpai_core::persistence::PublicationGate,
    ) -> Result<Self::Worker> {
        DesktopWatchWorker::new(self, generation, cancellation, publication).await
    }

    async fn clear_watch_error(&self, generation: u64) {
        self.publish_watch_view(generation, WatchResult::Recovered)
            .await;
    }

    async fn watch_finished(&self, generation: u64, failed: bool) {
        DesktopState::watch_finished(self, generation, failed).await;
    }
}

pub(crate) struct DesktopWatchWorker {
    state: Arc<DesktopState>,
    generation: u64,
    cancellation: CancellationToken,
    screen_capture: platform::MacScreenCapture,
    activity: platform::MacActivity,
    helper: Option<std::path::PathBuf>,
    ocr: platform::MacOcr,
    ocr_enabled: bool,
    semaphore: Arc<Semaphore>,
    trigger_coordinator: TriggerCoordinator,
    application_watch: ApplicationWatchSet,
    application_capture: platform::MacApplicationCapture,
    on_battery: bool,
    memory: WatchMemory,
    tutorial_initial_capture_pending: bool,
}

impl DesktopWatchWorker {
    async fn new(
        state: Arc<DesktopState>,
        generation: u64,
        cancellation: CancellationToken,
        publication: coosenpai_core::persistence::PublicationGate,
    ) -> Result<Self> {
        let screen_capture = platform::MacScreenCapture::with_logger(state.logger.clone());
        let activity = platform::MacActivity;
        let initial_config = state.runtime_config();
        let helper = resolve_desktop_ocr_helper(&state, &initial_config);
        state.logger.write(
            "INFO",
            if helper.is_some() {
                "Vision OCR: subprocess-helper"
            } else {
                "Vision OCR: disabled-no-helper"
            },
        )?;
        let ocr = platform::MacOcr::new(helper.clone());
        let ocr_enabled = initial_config.watch.ocr_gate.enabled && helper.is_some();
        state
            .publish_watch_view(generation, WatchResult::OcrConfigured(ocr_enabled))
            .await;
        let semaphore = Arc::new(Semaphore::new(2));
        let now = Instant::now();
        let now_utc = chrono::Utc::now();
        let initial = activity.read_activity().await.ok();
        let stagnation_store = WatchStagnationStore::new(state.paths.watch_stagnation.clone())
            .with_publication_gate(publication.clone());
        let stagnation_snapshot = match stagnation_store.load(now_utc) {
            Ok(value) => value,
            Err(error) => {
                let _ = state.logger.write(
                    "WARN",
                    &format!("停滞状態を読み込めませんでした: error-type=persistence ({error})"),
                );
                coosenpai_core::watch_coordinator::StagnationSnapshot {
                    last_meaningful_change_at: now_utc,
                    reported: false,
                    pending_report: None,
                    fingerprints: Default::default(),
                }
            }
        };
        let trigger_coordinator =
            TriggerCoordinator::new(&state.runtime_config(), initial.as_ref());
        let application_watch = ApplicationWatchSet::new(
            &initial_config,
            initial.as_ref(),
            now,
            &stagnation_snapshot.fingerprints,
        );
        let application_capture =
            platform::MacApplicationCapture::with_logger(state.logger.clone());
        let on_battery =
            state.runtime_config().watch.battery.enabled && platform::is_on_battery().await;
        let initial_interval = effective_max_interval_ms(&state.runtime_config(), on_battery);
        let memory = WatchMemory {
            publication,
            frames: Vec::new(),
            directories: Vec::new(),
            last_hash: stagnation_snapshot
                .fingerprints
                .get("fullscreen")
                .map(|value| value.image_hash.clone()),
            last_ocr: stagnation_snapshot
                .fingerprints
                .get("fullscreen")
                .and_then(|value| value.ocr_signature.clone()),
            last_capture: now
                .checked_sub(Duration::from_millis(initial_interval))
                .unwrap_or(now),
            last_observation: now,
            window_start: now,
            last_accepted: None,
            front_app: initial.as_ref().and_then(|value| value.front_app.clone()),
            stagnation: StagnationTracker::resume(
                now,
                stagnation_snapshot.elapsed(now_utc),
                initial.as_ref(),
                stagnation_snapshot.reported,
            ),
            stagnation_store,
            pending_stagnation_report: stagnation_snapshot.pending_report,
            last_meaningful_change_at: stagnation_snapshot.last_meaningful_change_at,
        };
        let tutorial_initial_capture_pending =
            state.tutorial_current_step().await == Some(TutorialStep::Watch);
        Ok(Self {
            state,
            generation,
            cancellation,
            screen_capture,
            activity,
            helper,
            ocr,
            ocr_enabled,
            semaphore,
            trigger_coordinator,
            application_watch,
            application_capture,
            on_battery,
            memory,
            tutorial_initial_capture_pending,
        })
    }
}

#[async_trait::async_trait]
impl WatchWorker for DesktopWatchWorker {
    async fn poll(&mut self) -> Result<ControlFlow<()>> {
        let Self {
            state,
            generation,
            cancellation,
            screen_capture,
            activity,
            helper,
            ocr,
            ocr_enabled,
            semaphore,
            trigger_coordinator,
            application_watch,
            application_capture,
            on_battery,
            memory,
            tutorial_initial_capture_pending,
        } = self;
        let generation = *generation;
        let config = state.runtime_config();
        let next_helper = resolve_desktop_ocr_helper(state, &config);
        if next_helper != *helper {
            ocr.set_helper(next_helper.clone())?;
            *helper = next_helper;
        }
        let next_ocr_enabled = config.watch.ocr_gate.enabled && helper.is_some();
        if next_ocr_enabled != *ocr_enabled {
            *ocr_enabled = next_ocr_enabled;
            state
                .publish_watch_view(generation, WatchResult::OcrConfigured(*ocr_enabled))
                .await;
        }
        let poll_delay = if *tutorial_initial_capture_pending {
            Duration::ZERO
        } else {
            Duration::from_millis(config.watch.triggers.poll_ms)
        };
        tokio::select! {
            _ = cancellation.cancelled() => return Ok(ControlFlow::Break(())),
            _ = tokio::time::sleep(poll_delay) => {}
        }
        if config.watch.battery.enabled {
            *on_battery = platform::is_on_battery().await;
        }
        let activity_snapshot = activity.read_activity().await.ok();
        let fresh_activity = memory.stagnation.observe_activity(
            activity_snapshot.as_ref(),
            config.watch.triggers.active_threshold_ms,
        );
        if fresh_activity && memory.stagnation.is_reported() {
            let reacted_at = chrono::Utc::now();
            match memory.stagnation_store.record_reaction(reacted_at) {
                Ok(true) => {
                    memory.stagnation.mark_meaningful_change(Instant::now());
                    memory.pending_stagnation_report = None;
                    memory.last_meaningful_change_at = reacted_at;
                }
                Ok(false) => {}
                Err(error) => {
                    let _ = state.logger.write(
                        "WARN",
                        &format!("停滞エピソードの操作反応を保存できませんでした: error-type=persistence ({error})"),
                    );
                }
            }
        }
        let effective_interval = effective_max_interval_ms(&config, *on_battery);
        let tutorial_initial_capture = *tutorial_initial_capture_pending
            && state.tutorial_current_step().await == Some(TutorialStep::Watch);
        let now = Instant::now();
        let decision = evaluate_activity_poll(
            trigger_coordinator,
            &config,
            activity_snapshot.as_ref(),
            now,
            memory.last_capture.elapsed().as_millis() as u64,
            effective_interval,
        );
        memory.front_app = decision.front_app;
        let trigger = config
            .watch
            .fullscreen
            .then(|| capture_trigger(tutorial_initial_capture, decision.trigger))
            .flatten();
        if let Some(trigger) = trigger {
            let elapsed = memory.last_capture.elapsed().as_millis() as u64;
            if capture_is_allowed(
                tutorial_initial_capture,
                elapsed,
                config.watch.triggers.min_spacing_ms,
            ) {
                let disposition = capture(
                    state,
                    &config,
                    *ocr_enabled,
                    screen_capture,
                    ocr,
                    semaphore,
                    memory,
                    trigger,
                    generation,
                    cancellation.clone(),
                )
                .await
                .context("画面の観察準備")?;
                record_capture_decision(state, generation, trigger, &memory.front_app, disposition)
                    .await;
            } else {
                record_gate(
                    state,
                    &config,
                    trigger,
                    None,
                    None,
                    false,
                    "最低間隔",
                    &memory.publication,
                )?;
                record_capture_decision(
                    state,
                    generation,
                    trigger,
                    &memory.front_app,
                    CaptureDisposition::MinSpacing,
                )
                .await;
            }
        }
        application_watch
            .poll(
                state,
                &config,
                activity_snapshot.as_ref(),
                effective_interval,
                tutorial_initial_capture,
                *ocr_enabled,
                application_capture,
                ocr,
                semaphore,
                memory,
                generation,
                cancellation.clone(),
            )
            .await
            .context("アプリ観察")?;
        *tutorial_initial_capture_pending = false;
        flush_if_due(state, &config, memory, generation, cancellation.clone())
            .await
            .context("観察の送信と配達")?;
        heartbeat_if_due(
            state,
            &config,
            memory,
            effective_interval,
            generation,
            cancellation.clone(),
        )
        .await
        .context("見守り heartbeat")?;
        Ok(ControlFlow::Continue(()))
    }
}
