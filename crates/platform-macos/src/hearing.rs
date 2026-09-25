#[path = "hearing_process.rs"]
mod hearing_process;

use async_trait::async_trait;
use coosenpai_core::interactive_process::{InteractiveProcess, InteractiveProcessRequest};
use coosenpai_core::ports::{
    HearingCommand, HearingEvent, HearingPort, HearingSession, HearingStartOptions, PortError,
    RuntimeLogger,
};
use coosenpai_core::state::AudioObservationSource;
use hearing_process::{
    process_error, run_source_process, SourceProcessEventKind, SourceProcessSpec,
};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
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
        for source in sources {
            let args = helper_arguments_with_options(
                locale,
                input_device,
                source,
                debug_dump_dir,
                &options,
            );
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
                cancellation.clone(),
            )
            .await;
            let (initial_process, initial_error) = match initial_process {
                Ok(process) => (Some(process), None),
                Err(error) => (None, Some(process_error(error))),
            };
            let _ = self.logger.write("INFO", &format!("hearing-start: source={source:?} stage=spawn phase=end elapsed-ms={} success={}", started.elapsed().as_millis(), initial_process.is_some()));
            specs.push(SourceProcessSpec {
                source,
                executable: self.helper.clone(),
                args,
                initial_process,
                initial_error,
                parent_cancellation: cancellation.clone(),
                logger: self.logger.clone(),
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
        tokio::spawn(run_session(
            specs,
            command_rx,
            event_tx,
            cancel_requested.clone(),
            cancellation,
            options.speaker_identification_enabled,
        ));
        Ok(HearingSession::from_channels_with_cancellation(
            command_tx,
            event_rx,
            cancel_requested,
        ))
    }
}

struct SourceStatus {
    source: AudioObservationSource,
    required: bool,
    ready: Option<HearingEvent>,
    generation_offset: u64,
    latest_generation: u64,
    system_audio_failed: bool,
    last_error: Option<PortError>,
}

struct HearingAggregation {
    sources: Vec<SourceStatus>,
    ready_sent: bool,
    pending_recognition_events: VecDeque<HearingEvent>,
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
                    ready: None,
                    generation_offset: 0,
                    latest_generation: 0,
                    system_audio_failed: false,
                    last_error: None,
                })
                .collect(),
            ready_sent: false,
            pending_recognition_events: VecDeque::new(),
        }
    }

    fn mark_restarting(&mut self, source: AudioObservationSource) {
        if let Some(status) = self.status_mut(source) {
            status.ready = None;
            status.generation_offset = status.latest_generation;
        }
    }

    fn normalize_generation(
        &mut self,
        source: AudioObservationSource,
        event: &mut HearingEvent,
    ) -> Result<(), PortError> {
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
            status.required = true;
            status.ready = Some(event);
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
            let protocol_error = status.last_error.as_ref().is_some_and(is_protocol_error);
            if !protocol_error || matches!(event, HearingEvent::Final { .. }) {
                status.last_error = None;
            }
        }
    }

    fn mark_unavailable(&mut self, source: AudioObservationSource, error: Option<PortError>) {
        if let Some(error) = error {
            self.record_error(source, error);
        }
        if let Some(status) = self.status_mut(source) {
            status.required = false;
            status.ready = None;
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

    fn no_required_sources(&self) -> bool {
        self.sources.iter().all(|status| !status.required)
    }

    fn ready_event(&self) -> Option<HearingEvent> {
        self.sources
            .iter()
            .filter(|status| status.required)
            .find_map(|status| status.ready.clone())
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
    if current
        .as_ref()
        .is_some_and(|existing| is_protocol_error(existing) && !is_protocol_error(&candidate))
    {
        return;
    }
    *current = Some(candidate);
}

async fn run_session(
    specs: Vec<SourceProcessSpec>,
    mut commands: mpsc::Receiver<HearingCommand>,
    events: mpsc::Sender<HearingEventResult>,
    cancel_requested: CancellationToken,
    parent_cancellation: CancellationToken,
    speaker_identification_enabled: bool,
) {
    let mut terminal_event = match events.clone().reserve_owned().await {
        Ok(permit) => Some(permit),
        Err(_) => return,
    };
    let source_list = specs.iter().map(|spec| spec.source).collect::<Vec<_>>();
    let (source_events_tx, mut source_events_rx) = mpsc::channel(64);
    let worker_cancellation = CancellationToken::new();
    let mut workers = Vec::with_capacity(specs.len());
    for spec in specs {
        let source_events = source_events_tx.clone();
        let worker_stop = worker_cancellation.clone();
        workers.push(tokio::spawn(async move {
            run_source_process(spec, source_events, worker_stop).await
        }));
    }
    drop(source_events_tx);

    let mut aggregation = HearingAggregation::new(&source_list);
    let mut terminal_error = None;
    loop {
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                Some(HearingCommand::Cancel { completed }) => {
                    let result = stop_workers(&worker_cancellation, workers).await;
                    send_terminal_event(&mut terminal_event, Ok(HearingEvent::Closed));
                    let _ = completed.send(result);
                    return;
                }
                None => {
                    let _ = stop_workers(&worker_cancellation, workers).await;
                    return;
                }
            },
            _ = cancel_requested.cancelled() => {
                let result = stop_workers(&worker_cancellation, workers).await;
                send_terminal_event(&mut terminal_event, Ok(HearingEvent::Closed));
                if let Some(HearingCommand::Cancel { completed }) = commands.recv().await {
                    let _ = completed.send(result);
                }
                return;
            }
            _ = parent_cancellation.cancelled() => {
                let result = stop_workers(&worker_cancellation, workers).await;
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
                    }
                    SourceProcessEventKind::Event(mut event) => {
                        if let Err(error) = aggregation.normalize_generation(source_event.source, &mut event) {
                            let _ = stop_workers(&worker_cancellation, workers).await;
                            send_terminal_event(&mut terminal_event, Err(error));
                            return;
                        }
                        aggregation.mark_recovered(source_event.source, &event);
                        match event {
                        event @ HearingEvent::Ready { .. } => {
                            if source_event.source == AudioObservationSource::Speaker
                                && speaker_identification_enabled
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
                                let _ = stop_workers(&worker_cancellation, workers).await;
                                return;
                            }
                            let restored = aggregation.status_mut(source_event.source)
                                .is_some_and(|status| std::mem::take(&mut status.system_audio_failed));
                            aggregation.mark_ready(source_event.source, event);
                            if restored && !send_hearing_event(
                                &events,
                                Ok(HearingEvent::Warning {
                                    kind: "system-audio-restored".to_owned(),
                                    message: "スピーカー音声の取得を再開しました".to_owned(),
                                }),
                                &cancel_requested,
                                &parent_cancellation,
                            ).await {
                                let _ = stop_workers(&worker_cancellation, workers).await;
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
                                let _ = stop_workers(&worker_cancellation, workers).await;
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
                                let _ = stop_workers(&worker_cancellation, workers).await;
                                return;
                            }
                        }
                        event @ HearingEvent::Recognizing { .. }
                        | event @ HearingEvent::NoSpeech { .. }
                        | event @ HearingEvent::Final { .. } => {
                            if aggregation.ready_sent {
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
                                    let _ = stop_workers(&worker_cancellation, workers).await;
                                    return;
                                }
                            } else {
                                aggregation.pending_recognition_events.push_back(event);
                            }
                        }
                        HearingEvent::Error { ref kind, .. } if kind == "no-input-source" => {}
                        event @ HearingEvent::Warning { .. }
                        | event @ HearingEvent::Error { .. } => {
                            if let HearingEvent::Error { ref kind, ref message } = event {
                                aggregation.record_error(
                                    source_event.source,
                                    PortError::Unavailable(format!(
                                        "source={} kind={kind}: {message}",
                                        source_name(source_event.source)
                                    )),
                                );
                                if source_event.source == AudioObservationSource::Speaker
                                    && (kind == "system-audio" || kind.starts_with("system-audio-"))
                                {
                                    if let Some(status) = aggregation.status_mut(source_event.source) {
                                        status.system_audio_failed = true;
                                    }
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
                                    let _ = stop_workers(&worker_cancellation, workers).await;
                                    return;
                                }
                        }
                        HearingEvent::Closed => {}
                    }},
                    SourceProcessEventKind::Unavailable { error } => {
                        let protocol_warning = error
                            .as_ref()
                            .and_then(protocol_warning_for_error);
                        aggregation.mark_unavailable(source_event.source, error);
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
                                let _ = stop_workers(&worker_cancellation, workers).await;
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
                                let _ = stop_workers(&worker_cancellation, workers).await;
                                return;
                            }
                    }
                    SourceProcessEventKind::Exhausted {
                        error: process_error,
                    } => {
                        let source_error = aggregation
                            .status_mut(source_event.source)
                            .and_then(|status| status.last_error.take());
                        let mut error = source_error;
                        prefer_error(&mut error, process_error);
                        aggregation.mark_unavailable(source_event.source, None);
                        prefer_error(&mut terminal_error, error);
                        if aggregation.no_required_sources() {
                            let mut error = terminal_error.take();
                            for status in &mut aggregation.sources {
                                prefer_error(&mut error, status.last_error.take());
                            }
                            let event = error
                                .map(Err)
                                .unwrap_or_else(|| Ok(HearingEvent::Closed));
                            let _ = stop_workers(&worker_cancellation, workers).await;
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
                            let _ = stop_workers(&worker_cancellation, workers).await;
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
    if aggregation.ready_sent || !aggregation.all_required_sources_are_ready() {
        return true;
    }
    let Some(ready) = aggregation.ready_event() else {
        return false;
    };
    if !send_hearing_event(events, Ok(ready), cancel_requested, parent_cancellation).await {
        return false;
    }
    aggregation.ready_sent = true;
    while let Some(event) = aggregation.pending_recognition_events.pop_front() {
        if !send_hearing_event(events, Ok(event), cancel_requested, parent_cancellation).await {
            return false;
        }
    }
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
    workers: Vec<tokio::task::JoinHandle<Result<(), PortError>>>,
) -> Result<(), PortError> {
    cancellation.cancel();
    let mut result = Ok(());
    for worker in workers {
        match worker.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if result.is_ok() => result = Err(error),
            Ok(Err(_)) => {}
            Err(error) if result.is_ok() => {
                result = Err(PortError::Unavailable(format!(
                    "聴覚観察 source worker が停止しました: {error}"
                )))
            }
            Err(_) => {}
        }
    }
    result
}

fn helper_arguments_with_options(
    locale: &str,
    input_device: &str,
    source: AudioObservationSource,
    debug_dump_dir: Option<&str>,
    options: &HearingStartOptions,
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
    if source == AudioObservationSource::Speaker && options.speaker_identification_enabled {
        args.push("--speaker-identification".to_owned());
        if let Some(model) = &options.speaker_model {
            args.push("--speaker-model".to_owned());
            args.push(model.to_string_lossy().into_owned());
        }
        if let Some(ledger) = &options.speaker_ledger {
            args.push("--speaker-ledger".to_owned());
            args.push(ledger.to_string_lossy().into_owned());
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
