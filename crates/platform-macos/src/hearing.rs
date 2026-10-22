#[path = "hearing_process.rs"]
mod hearing_process;

use crate::hearing_command::{run_hearing_helper_command, HearingHelperCommandError};
use async_trait::async_trait;
use coosenpai_core::interactive_process::{InteractiveProcess, InteractiveProcessRequest};
use coosenpai_core::ports::{
    HearingCommand, HearingEvent, HearingPort, HearingSession, HearingSourceState,
    HearingStartOptions, HearingStopStatus, PortError, RuntimeLogger,
};
use coosenpai_core::state::AudioObservationSource;
use hearing_process::{
    process_error, register_helper, run_source_process, SourceProcessEvent, SourceProcessEventKind,
    SourceProcessSpec,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

type HearingEventResult = Result<HearingEvent, PortError>;
type TerminalEventPermit = mpsc::OwnedPermit<HearingEventResult>;

#[derive(Clone)]
pub struct MacHearing {
    helper: PathBuf,
    logger: Arc<dyn RuntimeLogger>,
}

impl MacHearing {
    pub fn new(helper: PathBuf, logger: Arc<dyn RuntimeLogger>) -> Self {
        Self {
            helper,
            logger,
        }
    }

}

#[async_trait]
impl HearingPort for MacHearing {
    async fn output_devices(
        &self,
    ) -> Result<Vec<coosenpai_core::ports::AudioOutputDevice>, PortError> {
        let output = run_hearing_helper_command(
            &self.helper,
            &["--list-output-devices".into()],
            std::time::Duration::from_secs(5),
        )
        .await
        .map_err(|error| match error {
            HearingHelperCommandError::Timeout => {
                PortError::Unavailable("出力デバイス一覧の取得が時間内に終わりません".to_owned())
            }
            HearingHelperCommandError::Io(_) => {
                PortError::Unavailable("出力デバイス一覧を取得できません".to_owned())
            }
        })?;
        if !output.status.success() {
            return Err(PortError::Unavailable(
                "出力デバイス一覧を取得できません".to_owned(),
            ));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|_| PortError::Unavailable("出力デバイス一覧を解析できません".to_owned()))
    }

    async fn start(
        &self,
        locale: &str,
        input_device: &str,
        sources: Vec<AudioObservationSource>,
        debug_dump_dir: Option<&str>,
        cancellation: CancellationToken,
    ) -> Result<HearingSession, PortError> {
        self.start_with_options(
            locale,
            input_device,
            sources,
            debug_dump_dir,
            HearingStartOptions::default(),
            cancellation,
        )
        .await
    }

    async fn start_with_options(
        &self,
        locale: &str,
        input_device: &str,
        sources: Vec<AudioObservationSource>,
        debug_dump_dir: Option<&str>,
        options: HearingStartOptions,
        cancellation: CancellationToken,
    ) -> Result<HearingSession, PortError> {
        if sources.is_empty() {
            return Err(PortError::Unavailable(
                "聴覚観察の入力源が選択されていません".to_owned(),
            ));
        }

        let mut specs = Vec::with_capacity(sources.len());
        let active_helpers = hearing_process::ActiveHelpers::default();
        let speaker_session_id = uuid::Uuid::new_v4().to_string();
        for source in sources {
            let mut args = helper_arguments_with_options(
                locale,
                input_device,
                source,
                debug_dump_dir,
                &options,
                Some(&speaker_session_id),
            );
            args.push("--parent-pid".to_owned());
            args.push(std::process::id().to_string());
            if source == AudioObservationSource::Speaker {
                for pid in specs.iter().filter_map(|spec: &SourceProcessSpec| {
                    spec.initial_process
                        .as_ref()
                        .and_then(|process| process.control().pid())
                }) {
                    args.push("--exclude-pid".to_owned());
                    args.push(pid.to_string());
                }
            }
            let started = std::time::Instant::now();
            let _ = self.logger.write(
                "INFO",
                &format!("hearing-start: source={source:?} stage=spawn phase=begin"),
            );
            let initial_process = InteractiveProcess::spawn(
                InteractiveProcessRequest {
                    executable: self.helper.clone(),
                    args: args.clone(),
                    env: Vec::new(),
                    cwd: None,
                },
                CancellationToken::new(),
            )
            .await;
            let (initial_process, initial_error) = match initial_process {
                Ok(process) => (Some(process), None),
                Err(error) => (None, Some(process_error(error))),
            };
            if let Some(process) = initial_process.as_ref() {
                register_helper(source, process.control(), &active_helpers).await;
            }
            let _ = self.logger.write("INFO", &format!("hearing-start: source={source:?} stage=spawn phase=end elapsed-ms={} success={}", started.elapsed().as_millis(), initial_process.is_some()));
            specs.push(SourceProcessSpec {
                source,
                executable: self.helper.clone(),
                args,
                initial_process,
                initial_error,
                parent_cancellation: cancellation.clone(),
                logger: self.logger.clone(),
                active_helpers: active_helpers.clone(),
            });
        }
        if specs.iter().all(|spec| spec.initial_process.is_none()) {
            let error = specs
                .iter_mut()
                .find_map(|spec| spec.initial_error.take())
                .expect("failed helper spawn must have an error");
            return Err(error);
        }

        let (command_tx, command_rx) = mpsc::channel(4);
        let (event_tx, event_rx) = mpsc::channel(64);
        let cancel_requested = CancellationToken::new();
        let force_requested = CancellationToken::new();
        let session_done = CancellationToken::new();
        let (stop_status_tx, stop_status_rx) = watch::channel(None);
        let force_helpers = active_helpers.clone();
        let force_signal = force_requested.clone();
        let force_done = session_done.clone();
        let session_done_for_task = session_done.clone();
        let session_cancel = cancel_requested.clone();
        let task_helpers = active_helpers.clone();
        let panic_force = force_requested.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = force_signal.cancelled() => hearing_process::force_kill_hearing_helpers(&force_helpers).await,
                _ = force_done.cancelled() => {},
            }
        });
        let session_task = tokio::spawn(async move {
            run_session(
                specs,
                SessionChannels {
                    commands: command_rx,
                    events: event_tx,
                    stop_status: stop_status_tx,
                },
                session_cancel,
                cancellation,
                options.speaker_identification_enabled,
            )
            .await;
        });
        tokio::spawn(async move {
            if session_task.await.is_err() {
                panic_force.cancel();
            }
            hearing_process::wait_for_hearing_helpers(&task_helpers).await;
            session_done_for_task.cancel();
        });
        Ok(HearingSession::from_channels_with_stop_status(
            command_tx,
            event_rx,
            cancel_requested,
            force_requested,
            session_done,
            stop_status_rx,
        ))
    }
}

struct SourceStatus {
    source: AudioObservationSource,
    required: bool,
    exhausted: bool,
    ready: Option<HearingEvent>,
    generation_offset: u64,
    latest_generation: u64,
    input_generation_offset: u64,
    latest_input_generation: u64,
    speaker_capture_failed: bool,
    last_error: Option<PortError>,
}

struct HearingAggregation {
    sources: Vec<SourceStatus>,
    aggregate_ready: bool,
    microphone_bluetooth: Option<bool>,
    speaker_low_rate: Option<bool>,
    bluetooth_warning_active: bool,
}

impl HearingAggregation {
    fn new(sources: &[AudioObservationSource]) -> Self {
        Self {
            sources: sources
                .iter()
                .copied()
                .map(|source| SourceStatus {
                    source,
                    required: true,
                    exhausted: false,
                    ready: None,
                    generation_offset: 0,
                    latest_generation: 0,
                    input_generation_offset: 0,
                    latest_input_generation: 0,
                    speaker_capture_failed: false,
                    last_error: None,
                })
                .collect(),
            aggregate_ready: false,
            microphone_bluetooth: None,
            speaker_low_rate: None,
            bluetooth_warning_active: false,
        }
    }

    fn mark_restarting(&mut self, source: AudioObservationSource) {
        match source {
            AudioObservationSource::Microphone => self.microphone_bluetooth = None,
            AudioObservationSource::Speaker => self.speaker_low_rate = None,
        }
        if let Some(status) = self.status_mut(source) {
            status.generation_offset = status.latest_generation;
            status.input_generation_offset = status.latest_input_generation;
        }
        self.suspend_source(source);
    }

    fn bluetooth_warning(&mut self) -> Option<HearingEvent> {
        let active = self.microphone_bluetooth == Some(true) && self.speaker_low_rate == Some(true);
        if active == self.bluetooth_warning_active {
            return None;
        }
        self.bluetooth_warning_active = active;
        Some(HearingEvent::Warning {
            kind: if active {
                "bluetooth-call-mode"
            } else {
                "bluetooth-call-mode-ended"
            }
            .to_owned(),
            message: if active {
                "Bluetooth マイク使用中にスピーカー入力が通話用モードになりました"
            } else {
                "Bluetooth 通話用モードは終了しました"
            }
            .to_owned(),
        })
    }

    fn suspend_source(&mut self, source: AudioObservationSource) {
        if let Some(status) = self.status_mut(source) {
            if status.ready.take().is_some() {
                self.aggregate_ready = false;
            }
        }
    }

    fn normalize_generation(
        &mut self,
        source: AudioObservationSource,
        event: &mut HearingEvent,
    ) -> Result<(), PortError> {
        if let HearingEvent::Ready {
            input_generation, ..
        }
        | HearingEvent::Final {
            input_generation, ..
        } = event
        {
            if *input_generation == 0 {
                return Err(invalid_event_protocol_error(source));
            }
            let status = self.status_mut(source).ok_or_else(|| {
                PortError::Unavailable("聴覚イベントの音源が登録されていません".to_owned())
            })?;
            let mapped = input_generation
                .checked_add(status.input_generation_offset)
                .ok_or_else(|| {
                    PortError::Unavailable("聴覚入力の世代が上限に達しました".to_owned())
                })?;
            status.latest_input_generation = status.latest_input_generation.max(mapped);
            *input_generation = mapped;
        }
        let (event_source, generation, sequence) = match event {
            HearingEvent::Recognizing {
                source,
                generation,
                sequence,
                ..
            }
            | HearingEvent::Final {
                source,
                generation,
                sequence,
                ..
            }
            | HearingEvent::NoSpeech {
                source,
                generation,
                sequence,
            } => (source, generation, sequence),
            _ => return Ok(()),
        };
        if *event_source != source || *generation == 0 || *sequence == 0 {
            return Err(invalid_event_protocol_error(source));
        }
        let status = self.status_mut(source).ok_or_else(|| {
            PortError::Unavailable("聴覚イベントの音源が登録されていません".to_owned())
        })?;
        let mapped = generation
            .checked_add(status.generation_offset)
            .ok_or_else(|| {
                PortError::Unavailable("聴覚イベントの世代が上限に達しました".to_owned())
            })?;
        status.latest_generation = status.latest_generation.max(mapped);
        *generation = mapped;
        Ok(())
    }

    fn mark_ready(&mut self, source: AudioObservationSource, event: HearingEvent) {
        if let Some(status) = self.status_mut(source) {
            let became_required = !status.required;
            if status.ready.is_none()
                && matches!(status.last_error, Some(PortError::SpeakerOverflow(_)))
            {
                status.last_error = None;
            }
            status.required = true;
            status.ready = Some(event);
            if became_required {
                self.aggregate_ready = false;
            }
        }
    }

    fn mark_recovered(&mut self, source: AudioObservationSource, event: &HearingEvent) {
        let is_recovery_event = matches!(
            event,
            HearingEvent::Recognizing { .. }
                | HearingEvent::NoSpeech { .. }
                | HearingEvent::Final { .. }
        );
        if !is_recovery_event {
            return;
        }
        if let Some(status) = self.status_mut(source) {
            if status.ready.is_none() {
                return;
            }
            if matches!(
                status.last_error.as_ref(),
                Some(PortError::SpeakerOverflow(_))
            ) {
                return;
            }
            let protocol_error = status.last_error.as_ref().is_some_and(is_protocol_error);
            if !protocol_error || matches!(event, HearingEvent::Final { .. }) {
                status.last_error = None;
            }
        }
    }

    fn mark_unavailable(
        &mut self,
        source: AudioObservationSource,
        error: Option<PortError>,
        terminal: bool,
    ) {
        match source {
            AudioObservationSource::Microphone => self.microphone_bluetooth = None,
            AudioObservationSource::Speaker => self.speaker_low_rate = None,
        }
        if let Some(error) = error {
            self.record_error(source, error);
        }
        if let Some(status) = self.status_mut(source) {
            if terminal {
                status.required = false;
                status.exhausted = true;
            }
        }
        self.suspend_source(source);
    }

    fn mark_waiting(&mut self, source: AudioObservationSource) {
        if source == AudioObservationSource::Speaker {
            self.speaker_low_rate = None;
        }
        let changed = if let Some(status) = self.status_mut(source) {
            let changed = status.required || status.ready.is_some();
            status.required = false;
            status.ready = None;
            changed
        } else {
            false
        };
        if changed {
            self.aggregate_ready = false;
        }
    }

    fn record_error(&mut self, source: AudioObservationSource, error: PortError) {
        if let Some(status) = self.status_mut(source) {
            prefer_error(&mut status.last_error, Some(error));
        }
    }

    fn all_required_sources_are_ready(&self) -> bool {
        self.sources.iter().any(|status| status.required)
            && self
                .sources
                .iter()
                .filter(|status| status.required)
                .all(|status| status.ready.is_some())
    }

    fn all_sources_exhausted(&self) -> bool {
        self.sources.iter().all(|status| status.exhausted)
    }

    fn ready_event(&self) -> Option<HearingEvent> {
        let mut event = self
            .sources
            .iter()
            .filter(|status| status.required)
            .find_map(|status| status.ready.clone())?;
        if let HearingEvent::Ready { ready_sources, .. } = &mut event {
            *ready_sources = self
                .sources
                .iter()
                .filter(|status| status.ready.is_some())
                .map(|status| status.source)
                .collect();
        }
        Some(event)
    }

    fn source_is_ready(&self, source: AudioObservationSource) -> bool {
        self.sources
            .iter()
            .any(|status| status.source == source && status.ready.is_some())
    }

    fn status_mut(&mut self, source: AudioObservationSource) -> Option<&mut SourceStatus> {
        self.sources
            .iter_mut()
            .find(|status| status.source == source)
    }
}

fn is_protocol_error(error: &PortError) -> bool {
    matches!(
        error,
        PortError::Protocol(_) | PortError::SpeakerProtocol(_)
    )
}

fn prefer_error(current: &mut Option<PortError>, candidate: Option<PortError>) {
    let Some(candidate) = candidate else {
        return;
    };
    fn stop_failure_priority(error: &PortError) -> u8 {
        match error {
            PortError::HearingStopTimeout => 3,
            PortError::HearingStopCleanupFailed => 2,
            PortError::HearingStopUnconfirmed => 1,
            _ => 0,
        }
    }
    let incoming_priority = stop_failure_priority(&candidate);
    let current_priority = current.as_ref().map_or(0, stop_failure_priority);
    if incoming_priority > 0 || current_priority > 0 {
        if incoming_priority > current_priority {
            *current = Some(candidate);
        }
        return;
    }
    if matches!(candidate, PortError::SpeakerCaptureExhausted(_)) {
        if !current.as_ref().is_some_and(is_protocol_error) {
            *current = Some(candidate);
        }
        return;
    }
    if current.as_ref().is_some_and(|existing| {
        matches!(
            existing,
            PortError::SpeakerOverflow(_) | PortError::SpeakerCaptureExhausted(_)
        ) || (is_protocol_error(existing) && !is_protocol_error(&candidate))
    }) {
        return;
    }
    *current = Some(candidate);
}

struct SessionChannels {
    commands: mpsc::Receiver<HearingCommand>,
    events: mpsc::Sender<HearingEventResult>,
    stop_status: watch::Sender<Option<HearingStopStatus>>,
}

async fn run_session(
    specs: Vec<SourceProcessSpec>,
    channels: SessionChannels,
    cancel_requested: CancellationToken,
    parent_cancellation: CancellationToken,
    speaker_identification_enabled: bool,
) {
    let SessionChannels {
        mut commands,
        events,
        stop_status,
    } = channels;
    let mut terminal_event = match events.clone().reserve_owned().await {
        Ok(permit) => Some(permit),
        Err(_) => {
            for mut spec in specs {
                if let Some(process) = spec.initial_process.take() {
                    let _ = process.control().terminate(true).await;
                    process.wait_for_completion().await;
                }
            }
            return;
        }
    };
    let source_list = specs.iter().map(|spec| spec.source).collect::<Vec<_>>();
    let (source_events_tx, mut source_events_rx) = mpsc::channel(64);
    let worker_cancellation = CancellationToken::new();
    let mut workers = JoinSet::new();
    for spec in specs {
        let source_events = source_events_tx.clone();
        let worker_stop = worker_cancellation.clone();
        workers.spawn(async move { run_source_process(spec, source_events, worker_stop).await });
    }
    drop(source_events_tx);

    let mut aggregation = HearingAggregation::new(&source_list);
    let mut terminal_error = None;
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                Some(HearingCommand::Cancel { completed }) => {
                    let result = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                    send_terminal_event(&mut terminal_event, Ok(HearingEvent::Closed));
                    let _ = completed.send(result);
                    return;
                }
                None => {
                    let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                    return;
                }
            },
            _ = cancel_requested.cancelled() => {
                let result = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                send_terminal_event(&mut terminal_event, Ok(HearingEvent::Closed));
                if let Some(HearingCommand::Cancel { completed }) = commands.recv().await {
                    let _ = completed.send(result);
                }
                return;
            }
            _ = parent_cancellation.cancelled() => {
                let result = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                send_terminal_event(&mut terminal_event, Ok(HearingEvent::Closed));
                if let Ok(HearingCommand::Cancel { completed }) = commands.try_recv() {
                    let _ = completed.send(result);
                }
                return;
            }
            source_event = source_events_rx.recv() => {
                let Some(source_event) = source_event else {
                    let event = terminal_error
                        .map(Err)
                        .unwrap_or_else(|| Ok(HearingEvent::Closed));
                    send_terminal_event(&mut terminal_event, event);
                    return;
                };
                match source_event.kind {
                    SourceProcessEventKind::Restarting => {
                        aggregation.mark_restarting(source_event.source);
                        if let Some(warning) = aggregation.bluetooth_warning() {
                            if !send_hearing_event(&events, Ok(warning), &cancel_requested, &parent_cancellation).await {
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                        }
                    }
                    SourceProcessEventKind::Event { mut event, source_retrying } => {
                        if let Err(error) = aggregation.normalize_generation(source_event.source, &mut event) {
                            let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                            send_terminal_event(&mut terminal_event, Err(error));
                            return;
                        }
                        aggregation.mark_recovered(source_event.source, &event);
                        match event {
                        HearingEvent::MicrophoneDevice { bluetooth } => {
                            aggregation.microphone_bluetooth = Some(bluetooth);
                            if let Some(warning) = aggregation.bluetooth_warning() {
                                if !send_hearing_event(&events, Ok(warning), &cancel_requested, &parent_cancellation).await {
                                    let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                    return;
                                }
                            }
                        }
                        HearingEvent::SpeakerFormat { sample_rate } => {
                            aggregation.speaker_low_rate = Some(sample_rate <= 16_000);
                            if let Some(warning) = aggregation.bluetooth_warning() {
                                if !send_hearing_event(&events, Ok(warning), &cancel_requested, &parent_cancellation).await {
                                    let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                    return;
                                }
                            }
                        }
                        event @ HearingEvent::Ready { input_generation, .. } => {
                            if speaker_identification_enabled
                                && matches!(
                                    &event,
                                    HearingEvent::Ready {
                                        speaker_identification: false,
                                        ..
                                    }
                                )
                                && !send_hearing_event(
                                    &events,
                                    Ok(HearingEvent::Warning {
                                        kind: "speaker-identification-unavailable".to_owned(),
                                        message: "話者識別を利用できないため、話者 ID なしで文字起こしを続けます".to_owned(),
                                    }),
                                    &cancel_requested,
                                    &parent_cancellation,
                                )
                                .await
                            {
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                            let restored = aggregation.status_mut(source_event.source)
                                .is_some_and(|status| std::mem::take(&mut status.speaker_capture_failed));
                            aggregation.mark_ready(source_event.source, event);
                            if !send_hearing_event(
                                &events,
                                Ok(HearingEvent::SourceState {
                                    source: source_event.source,
                                    state: HearingSourceState::Ready { input_generation },
                                }),
                                &cancel_requested,
                                &parent_cancellation,
                            ).await {
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                            if restored && !send_hearing_event(
                                &events,
                                Ok(HearingEvent::Warning {
                                    kind: "system-audio-restored".to_owned(),
                                    message: "スピーカー音声の取得を再開しました".to_owned(),
                                }),
                                &cancel_requested,
                                &parent_cancellation,
                            ).await {
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                            if !emit_ready_if_possible(
                                &mut aggregation,
                                &events,
                                &cancel_requested,
                                &parent_cancellation,
                            )
                            .await
                            {
                                if cancel_requested.is_cancelled()
                                    || parent_cancellation.is_cancelled()
                                {
                                    continue;
                                }
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                        }
                        event @ HearingEvent::SpeakerIdentification { .. } => {
                            if !send_hearing_event(
                                &events,
                                Ok(event),
                                &cancel_requested,
                                &parent_cancellation,
                            )
                            .await
                            {
                                if cancel_requested.is_cancelled()
                                    || parent_cancellation.is_cancelled()
                                {
                                    continue;
                                }
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                        }
                        event @ HearingEvent::Recognizing { .. }
                        | event @ HearingEvent::NoSpeech { .. }
                        | event @ HearingEvent::Final { .. } => {
                            if !aggregation.source_is_ready(source_event.source) {
                                continue;
                            }
                            if !send_hearing_event(
                                &events, Ok(event), &cancel_requested, &parent_cancellation,
                            ).await
                            {
                                if cancel_requested.is_cancelled()
                                    || parent_cancellation.is_cancelled()
                                {
                                    continue;
                                }
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                        }
                        HearingEvent::Error { ref kind, .. } if kind == "no-input-source" => {}
                        event @ HearingEvent::Warning { .. }
                        | event @ HearingEvent::Error { .. } => {
                            let terminal_source_error = matches!(&event, HearingEvent::Error { .. }) && !source_retrying;
                            let source_state = match &event {
                                HearingEvent::Warning { kind, message } if kind == "speaker-device-waiting" => {
                                    Some(HearingSourceState::Waiting { kind: kind.clone(), message: message.clone() })
                                }
                                HearingEvent::Error { kind, message } => {
                                    Some(HearingSourceState::Failed { kind: kind.clone(), message: message.clone(), retrying: source_retrying })
                                }
                                _ => None,
                            };
                            let speaker_waiting = source_event.source == AudioObservationSource::Speaker
                                && matches!(&event, HearingEvent::Warning { kind, .. } if kind == "speaker-device-waiting");
                            if speaker_waiting {
                                aggregation.mark_waiting(source_event.source);
                                if let Some(warning) = aggregation.bluetooth_warning() {
                                    if !send_hearing_event(&events, Ok(warning), &cancel_requested, &parent_cancellation).await {
                                        let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                        return;
                                    }
                                }
                            }
                            if let HearingEvent::Error { ref kind, ref message } = event {
                                let error = if source_event.source == AudioObservationSource::Speaker
                                    && kind == "system-audio-overflow"
                                {
                                    PortError::SpeakerOverflow(message.clone())
                                } else {
                                    PortError::Unavailable(format!(
                                        "source={} kind={kind}: {message}",
                                        source_name(source_event.source)
                                    ))
                                };
                                aggregation.record_error(
                                    source_event.source,
                                    error,
                                );
                                if source_event.source == AudioObservationSource::Speaker
                                    && (kind == "screen-capture" || kind == "system-audio" || kind.starts_with("system-audio-"))
                                {
                                    aggregation.suspend_source(source_event.source);
                                    if let Some(status) = aggregation.status_mut(source_event.source) {
                                    status.speaker_capture_failed = true;
                                    }
                                }
                            }
                            if let Some(state) = source_state {
                                if !send_hearing_event(
                                    &events,
                                    Ok(HearingEvent::SourceState { source: source_event.source, state }),
                                    &cancel_requested,
                                    &parent_cancellation,
                                ).await {
                                    let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                    return;
                                }
                            }
                            if !send_hearing_event(
                                &events,
                                Ok(event),
                                &cancel_requested,
                                &parent_cancellation,
                                )
                                .await
                                {
                                    if cancel_requested.is_cancelled()
                                        || parent_cancellation.is_cancelled()
                                    {
                                        continue;
                                    }
                                    let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                    return;
                                }
                            if terminal_source_error {
                                aggregation.mark_unavailable(source_event.source, None, true);
                                if aggregation.all_sources_exhausted() {
                                    let mut error = terminal_error.take();
                                    for status in &mut aggregation.sources {
                                        prefer_error(&mut error, status.last_error.take());
                                    }
                                    let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                    send_terminal_event(&mut terminal_event, error.map(Err).unwrap_or_else(|| Ok(HearingEvent::Closed)));
                                    return;
                                }
                            }
                            if (speaker_waiting || terminal_source_error) && !emit_ready_if_possible(
                                &mut aggregation,
                                &events,
                                &cancel_requested,
                                &parent_cancellation,
                            ).await {
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                        }
                        HearingEvent::Closed => {}
                        HearingEvent::SourceState { .. } => {
                            let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                            send_terminal_event(&mut terminal_event, Err(invalid_event_protocol_error(source_event.source)));
                            return;
                        }
                    }},
                    SourceProcessEventKind::Unavailable { error, retrying } => {
                        let capture_failure_reported = aggregation.status_mut(source_event.source)
                            .is_some_and(|status| status.speaker_capture_failed);
                        if retrying && !capture_failure_reported && !send_hearing_event(
                            &events,
                            Ok(HearingEvent::SourceState {
                                source: source_event.source,
                                state: failed_source_state(source_event.source, error.as_ref()),
                            }),
                            &cancel_requested,
                            &parent_cancellation,
                        ).await {
                            let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                            return;
                        }
                        if retrying && source_event.source == AudioObservationSource::Speaker
                            && !matches!(error.as_ref(), Some(PortError::SpeakerProtocol(_)))
                            && !aggregation.status_mut(source_event.source)
                                .is_some_and(|status| status.speaker_capture_failed)
                        {
                            let kind = if matches!(error.as_ref(), Some(PortError::SpeakerOverflow(_))) {
                                "system-audio-overflow"
                            } else {
                                "system-audio-helper-exit"
                            };
                            if !send_hearing_event(
                                &events,
                                Ok(HearingEvent::Warning {
                                    kind: kind.to_owned(),
                                    message: "スピーカー取り込み helper が終了し、再起動しています".to_owned(),
                                }),
                                &cancel_requested,
                                &parent_cancellation,
                            )
                            .await
                            {
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                            if let Some(status) = aggregation.status_mut(source_event.source) {
                                status.speaker_capture_failed = true;
                            }
                        }
                        let protocol_warning = error
                            .as_ref()
                            .and_then(protocol_warning_for_error);
                        aggregation.mark_unavailable(source_event.source, error, false);
                        if let Some(warning) = aggregation.bluetooth_warning() {
                            if !send_hearing_event(&events, Ok(warning), &cancel_requested, &parent_cancellation).await {
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                        }
                        if let Some((kind, message)) = protocol_warning {
                            if !send_hearing_event(
                                &events,
                                Ok(HearingEvent::Warning { kind, message }),
                                &cancel_requested,
                                &parent_cancellation,
                            )
                            .await
                            {
                                if cancel_requested.is_cancelled()
                                    || parent_cancellation.is_cancelled()
                                {
                                    continue;
                                }
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                        }
                        if !emit_ready_if_possible(
                            &mut aggregation,
                            &events,
                            &cancel_requested,
                            &parent_cancellation,
                            )
                            .await
                            {
                                if cancel_requested.is_cancelled()
                                    || parent_cancellation.is_cancelled()
                                {
                                    continue;
                                }
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                    }
                    SourceProcessEventKind::Exhausted {
                        error: process_error,
                        stop_reason,
                        capture_failure,
                    } => {
                        let source_error = aggregation
                            .status_mut(source_event.source)
                            .and_then(|status| status.last_error.take());
                        let stop_reason = if capture_failure {
                            stop_reason
                        } else if let Some(ref source_error) = source_error {
                            format!("{stop_reason}; 原因: {source_error}")
                        } else {
                            stop_reason
                        };
                        let mut error = source_error;
                        prefer_error(&mut error, process_error);
                        if capture_failure && !error.as_ref().is_some_and(is_protocol_error) {
                            error = Some(PortError::SpeakerCaptureExhausted(stop_reason.clone()));
                        }
                        let stop_kind = if capture_failure {
                            "system-audio-exhausted"
                        } else {
                            "helper-exhausted"
                        };
                        let failed_state = HearingSourceState::Failed {
                            kind: stop_kind.to_owned(),
                            message: stop_reason.clone(),
                            retrying: false,
                        };
                        aggregation.mark_unavailable(source_event.source, None, true);
                        if let Some(warning) = aggregation.bluetooth_warning() {
                            if !send_hearing_event(&events, Ok(warning), &cancel_requested, &parent_cancellation).await {
                                let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                                return;
                            }
                        }
                        prefer_error(&mut terminal_error, error);
                        if !send_hearing_event(
                            &events,
                            Ok(HearingEvent::SourceState {
                                source: source_event.source,
                                state: failed_state,
                            }),
                            &cancel_requested,
                            &parent_cancellation,
                        ).await {
                            let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                            return;
                        }
                        if source_event.source == AudioObservationSource::Speaker
                            && !send_hearing_event(
                                &events,
                                Ok(HearingEvent::Warning {
                                    kind: stop_kind.to_owned(),
                                    message: stop_reason,
                                }),
                                &cancel_requested,
                                &parent_cancellation,
                            ).await {
                            let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                            return;
                        }
                        if aggregation.all_sources_exhausted() {
                            let mut error = terminal_error.take();
                            for status in &mut aggregation.sources {
                                prefer_error(&mut error, status.last_error.take());
                            }
                            let event = error
                                .map(Err)
                                .unwrap_or_else(|| Ok(HearingEvent::Closed));
                            let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                            send_terminal_event(&mut terminal_event, event);
                            return;
                        }
                        if !emit_ready_if_possible(
                            &mut aggregation,
                            &events,
                            &cancel_requested,
                            &parent_cancellation,
                        )
                        .await
                        {
                            if cancel_requested.is_cancelled()
                                || parent_cancellation.is_cancelled()
                            {
                                continue;
                            }
                            let _ = stop_workers(&worker_cancellation, workers, &mut source_events_rx, &stop_status).await;
                            return;
                        }
                    }
                }
            }
        }
    }
}

fn protocol_warning_for_error(error: &PortError) -> Option<(String, String)> {
    match error {
        PortError::SpeakerProtocol(message) => {
            Some(("speaker-protocol".to_owned(), message.clone()))
        }
        PortError::Protocol(message) => Some(("hearing-protocol".to_owned(), message.clone())),
        _ => None,
    }
}

fn failed_source_state(
    source: AudioObservationSource,
    error: Option<&PortError>,
) -> HearingSourceState {
    let (kind, message) = match error {
        Some(PortError::SpeakerCaptureExhausted(message)) => {
            ("system-audio-exhausted", message.clone())
        }
        Some(PortError::SpeakerOverflow(message)) => ("system-audio-overflow", message.clone()),
        Some(PortError::SpeakerProtocol(message)) => ("speaker-protocol", message.clone()),
        Some(PortError::Protocol(message)) => ("hearing-protocol", message.clone()),
        Some(PortError::ScreenCapturePermission(message)) => ("screen-capture", message.clone()),
        _ if source == AudioObservationSource::Speaker => (
            "system-audio-helper-exit",
            "スピーカー取り込み helper が終了し、再起動しています".to_owned(),
        ),
        _ => ("helper-closed", "聴覚観察 helper が終了しました".to_owned()),
    };
    HearingSourceState::Failed {
        kind: kind.to_owned(),
        message,
        retrying: true,
    }
}

fn invalid_event_protocol_error(source: AudioObservationSource) -> PortError {
    if source == AudioObservationSource::Speaker {
        PortError::SpeakerProtocol(
            "聴覚観察 helper の話者イベントの音源・世代・更新順序が不正です".to_owned(),
        )
    } else {
        PortError::Protocol("聴覚観察 helper のイベントの音源・世代・更新順序が不正です".to_owned())
    }
}

async fn emit_ready_if_possible(
    aggregation: &mut HearingAggregation,
    events: &mpsc::Sender<HearingEventResult>,
    cancel_requested: &CancellationToken,
    parent_cancellation: &CancellationToken,
) -> bool {
    if aggregation.aggregate_ready || !aggregation.all_required_sources_are_ready() {
        return true;
    }
    let Some(ready) = aggregation.ready_event() else {
        return false;
    };
    if !send_hearing_event(events, Ok(ready), cancel_requested, parent_cancellation).await {
        return false;
    }
    aggregation.aggregate_ready = true;
    true
}

async fn send_hearing_event(
    events: &mpsc::Sender<HearingEventResult>,
    event: HearingEventResult,
    cancel_requested: &CancellationToken,
    parent_cancellation: &CancellationToken,
) -> bool {
    tokio::select! {
        result = events.send(event) => result.is_ok(),
        _ = cancel_requested.cancelled() => false,
        _ = parent_cancellation.cancelled() => false,
    }
}

fn send_terminal_event(permit: &mut Option<TerminalEventPermit>, event: HearingEventResult) {
    if let Some(permit) = permit.take() {
        permit.send(event);
    }
}

async fn stop_workers(
    cancellation: &CancellationToken,
    mut workers: JoinSet<Result<(), PortError>>,
    source_events: &mut mpsc::Receiver<SourceProcessEvent>,
    stop_status: &watch::Sender<Option<HearingStopStatus>>,
) -> Result<(), PortError> {
    cancellation.cancel();
    source_events.close();
    let mut error = None;
    while let Some(worker) = workers.join_next().await {
        match worker {
            Ok(Ok(())) => {}
            Ok(Err(worker_error)) => prefer_error(&mut error, Some(worker_error)),
            Err(worker_error) => prefer_error(
                &mut error,
                Some(PortError::Unavailable(format!(
                    "聴覚観察 source worker が停止しました: {worker_error}"
                ))),
            ),
        }
    }
    let result = error.map_or(Ok(()), Err);
    stop_status.send_replace(Some(HearingStopStatus::from_result(&result)));
    result
}

fn helper_arguments_with_options(
    locale: &str,
    input_device: &str,
    source: AudioObservationSource,
    debug_dump_dir: Option<&str>,
    options: &HearingStartOptions,
    speaker_session_id: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "--locale".to_owned(),
        locale.to_owned(),
        "--input-device".to_owned(),
        input_device.to_owned(),
        "--sources".to_owned(),
        source_name(source).to_owned(),
    ];
    if let Some(directory) = debug_dump_dir {
        args.push("--debug-dump-appended".to_owned());
        args.push(directory.to_owned());
    }
    if options.speaker_identification_enabled {
        args.push("--speaker-identification".to_owned());
        if let Some(model) = &options.speaker_model {
            args.push("--speaker-model".to_owned());
            args.push(model.to_string_lossy().into_owned());
        }
        if let Some(ledger) = &options.speaker_ledger {
            args.push("--speaker-ledger".to_owned());
            args.push(ledger.to_string_lossy().into_owned());
        }
        if let Some(session_id) = speaker_session_id {
            args.push("--speaker-session-id".to_owned());
            args.push(session_id.to_owned());
        }
    }
    if source == AudioObservationSource::Speaker {
        for uid in &options.speaker_devices {
            args.push(format!("--speaker-device={uid}"));
        }
    }
    args
}

fn source_name(source: AudioObservationSource) -> &'static str {
    match source {
        AudioObservationSource::Microphone => "microphone",
        AudioObservationSource::Speaker => "speaker",
    }
}
