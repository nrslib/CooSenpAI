use crate::config::{local_date_at, AgentConfig, ConfigPaths, ObserverConfig};
use crate::debug::{ocr_preview, DebugStore, ObserverDebugCall};
use crate::frame_buffer::FrameBuffer;
use crate::mailbox::{Mailbox, MailboxError};
use crate::outbox::{DurableOutbox, OutboxError};
use crate::persistence::{prune_daily_jsonl_at, JsonlStore, PersistenceError};
use crate::ports::{Clock, RuntimeLogger, SystemClock};
use crate::prompts::{
    build_observer_prompt, observer_schema, observer_system_prompt, ObserverPromptFrame,
    PromptAudioSegment,
};
use crate::provider::{
    ProviderCall, ProviderClient, ProviderError, ProviderErrorKind, ProviderResult,
    ProviderSession, SessionRequest,
};
use crate::state::{
    parse_observation, parse_visual_observation, ActivityTriggerKind, AudioObservation,
    ObservationFrame, ObservationLimits, ObservationRecord, VisualObservation,
};
use crate::usage::{try_reserve_observer_role, ObserverCallKind, UsageError};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::mem;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const MAX_OBSERVER_ATTEMPTS: usize = 3;
// 観察 prompt は毎回現在の比較データを再構成するため、長期 session の履歴だけが判断へ残り続けないようにする。
const OBSERVER_SESSION_MAX_CALLS: usize = 60;

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SpeakerAliasIndex {
    schema_version: u8,
    registry_id: String,
    aliases: BTreeMap<String, String>,
}

fn load_speaker_aliases(paths: &ConfigPaths) -> Result<SpeakerAliasIndex, ObserverError> {
    let path = paths.speakers.join("aliases.json");
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SpeakerAliasIndex {
                schema_version: 1,
                registry_id: String::new(),
                aliases: BTreeMap::new(),
            });
        }
        Err(_) => return Err(ObserverError::Output),
    };
    let index: SpeakerAliasIndex =
        serde_json::from_slice(&data).map_err(|_| ObserverError::Output)?;
    if index.schema_version != 1 || uuid::Uuid::parse_str(&index.registry_id).is_err() {
        return Err(ObserverError::Output);
    }
    if index.aliases.iter().any(|(source, target)| {
        !valid_prompt_speaker_id(source) || !valid_prompt_speaker_id(target) || source == target
    }) {
        return Err(ObserverError::Output);
    }
    for source in index.aliases.keys() {
        let mut current = source.as_str();
        let mut visited = HashSet::new();
        while let Some(next) = index.aliases.get(current) {
            if !visited.insert(current) {
                return Err(ObserverError::Output);
            }
            current = next;
        }
    }
    Ok(index)
}

fn valid_prompt_speaker_id(value: &str) -> bool {
    value
        .strip_prefix("speaker-")
        .and_then(|number| number.parse::<u64>().ok())
        .is_some_and(|number| number > 0)
}

fn canonical_prompt_speaker_id(id: &str, aliases: &BTreeMap<String, String>) -> String {
    let mut current = id.to_owned();
    let mut visited = HashSet::new();
    while let Some(next) = aliases.get(&current) {
        if !visited.insert(current.clone()) {
            break;
        }
        current = next.clone();
    }
    current
}

fn namespaced_prompt_speaker_id(registry_id: &str, id: &str) -> Option<String> {
    let namespace = speaker_registry_namespace(registry_id)?;
    let prefix = format!("{namespace}/");
    if let Some(raw_id) = id.strip_prefix(&prefix) {
        return valid_prompt_speaker_id(raw_id).then(|| id.to_owned());
    }
    valid_prompt_speaker_id(id).then(|| format!("{namespace}/{id}"))
}

fn speaker_registry_namespace(registry_id: &str) -> Option<String> {
    let uuid = Uuid::parse_str(registry_id).ok()?;
    let digest = Sha256::digest(uuid.as_bytes());
    Some(format!(
        "r-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7]
    ))
}

fn prompt_speaker_id(record: &AudioObservation, aliases: &SpeakerAliasIndex) -> Option<String> {
    if record.source != crate::state::AudioObservationSource::Speaker
        || record.speaker_status != Some(crate::state::SpeakerIdentificationStatus::Identified)
    {
        return None;
    }
    let id = record.speaker_id.as_deref()?;
    if record.speaker_registry_id.as_deref() == Some(aliases.registry_id.as_str()) {
        valid_prompt_speaker_id(id).then(|| canonical_prompt_speaker_id(id, &aliases.aliases))
    } else {
        namespaced_prompt_speaker_id(record.speaker_registry_id.as_deref()?, id)
    }
}

fn sanitize_previous_audio(value: &Value, aliases: &SpeakerAliasIndex) -> Value {
    let mut sanitized = value.clone();
    let Some(segments) = sanitized
        .get_mut("audioSegments")
        .and_then(Value::as_array_mut)
    else {
        return sanitized;
    };
    for segment in segments {
        let Some(object) = segment.as_object_mut() else {
            continue;
        };
        let registry_id = object
            .get("speakerRegistryId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        object.remove("speakerRegistryId");
        if object.get("speakerStatus").and_then(Value::as_str) == Some("identified") {
            let mut replacements = Vec::new();
            let mut invalid = false;
            for speaker_key in ["speakerTag", "speakerId"] {
                if let Some(value) = object.get(speaker_key) {
                    let replacement = value.as_str().and_then(|id| {
                        if registry_id.as_deref() == Some(aliases.registry_id.as_str()) {
                            valid_prompt_speaker_id(id)
                                .then(|| canonical_prompt_speaker_id(id, &aliases.aliases))
                        } else {
                            registry_id
                                .as_deref()
                                .and_then(|registry| namespaced_prompt_speaker_id(registry, id))
                        }
                    });
                    if let Some(replacement) = replacement {
                        replacements.push((speaker_key, replacement));
                    } else {
                        invalid = true;
                    }
                }
            }
            if invalid || replacements.is_empty() {
                object.remove("speakerTag");
                object.remove("speakerId");
                object.insert(
                    "speakerStatus".to_owned(),
                    Value::String("unknown".to_owned()),
                );
            } else {
                for (speaker_key, replacement) in replacements {
                    object.insert(speaker_key.to_owned(), Value::String(replacement));
                }
            }
        } else {
            object.remove("speakerTag");
            object.remove("speakerId");
        }
    }
    sanitized
}

fn require_active_observation(cancellation: &CancellationToken) -> Result<(), ObserverError> {
    if cancellation.is_cancelled() {
        return Err(ProviderError {
            kind: ProviderErrorKind::Retryable,
            message: "observer をキャンセルしました".to_owned(),
        }
        .into());
    }
    Ok(())
}

fn session_mode(session: &SessionRequest) -> &'static str {
    match session {
        SessionRequest::New => "new",
        SessionRequest::Resume(_) => "resume",
        SessionRequest::Ephemeral | SessionRequest::Isolated => "ephemeral",
    }
}

#[path = "observer_storage.rs"]
mod storage;
pub use storage::{
    append_observation, excluded_bounds_for_self, mark_audio_consumed, migrate_legacy_audio,
    observation_store, read_audio_by_ids, read_observations_by_ids, read_unobserved_audio,
    record_audio_observation,
};
use storage::{
    append_observation_record, read_latest_observation, reconcile_transcripts, stagnation_identity,
    timestamp,
};

#[derive(Debug, Clone)]
pub struct ObservationFrameInput {
    pub display: Option<crate::ports::ScreenDisplay>,
    pub window_id: Option<u32>,
    pub window_bounds: Option<crate::ports::WindowBounds>,
    pub scope_generation: u64,
    pub context_id: String,
    pub captured_at: DateTime<Utc>,
    pub debug_id: Option<String>,
    pub relative_seconds: f64,
    pub trigger: ActivityTriggerKind,
    pub front_app: Option<String>,
    pub app: Option<String>,
    pub target: String,
    pub ocr_text: Option<String>,
    pub focus: Option<crate::ports::FocusElement>,
    pub image_path: PathBuf,
}

pub(crate) struct ScopedObservationOptions {
    pub(crate) expected_generation: u64,
    pub(crate) scope_generation: Arc<std::sync::atomic::AtomicU64>,
    pub(crate) scope_commit_lock: Arc<std::sync::Mutex<()>>,
    pub(crate) allow_companion_delivery: bool,
}

#[derive(Debug, Error)]
pub enum ObserverError {
    #[error("見守り対象が変更されたため観察を保存しませんでした")]
    StaleScope,
    #[error("observer の provider 呼び出しに失敗しました")]
    Provider(#[from] crate::provider::ProviderError),
    #[error("observer の構造化出力が不正です")]
    Output,
    #[error("observer の当日呼び出し上限に達しました")]
    LimitReached,
    #[error("observer の使用量を保存できませんでした: {0}")]
    Usage(#[from] UsageError),
    #[error("observer の永続化に失敗しました: {0}")]
    Persistence(#[from] PersistenceError),
    #[error("observer mailbox への配信に失敗しました: {0}")]
    Mailbox(#[from] MailboxError),
    #[error("observer outbox への保存に失敗しました: {0}")]
    Outbox(#[from] OutboxError),
    #[error("observer の観察を未配信として保持しました")]
    OutboxPending { record: Box<ObservationRecord> },
    #[error("observer のログ出力に失敗しました: {0}")]
    Log(#[from] io::Error),
    #[error("observer のフレーム保持に失敗しました: {0}")]
    FrameBuffer(#[source] io::Error),
    #[error("observer の JSON 化に失敗しました: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct ObserverAgent {
    provider: Arc<dyn ProviderClient>,
    vision_provider: Arc<dyn ProviderClient>,
    hearing_provider: Option<Arc<dyn ProviderClient>>,
    config: AgentConfig,
    vision_config: AgentConfig,
    hearing_config: Option<AgentConfig>,
    active_role: ObserverRole,
    session: Option<ProviderSession>,
    session_calls: usize,
    previous: Option<Value>,
    limits: ObservationLimits,
    usage_path: Option<PathBuf>,
    observation_enabled: bool,
    observation_directory: Option<PathBuf>,
    transcript_directory: Option<PathBuf>,
    observation_retention_days: Option<u64>,
    observation_paths: Option<ConfigPaths>,
    frame_buffer: Option<FrameBuffer>,
    previous_loaded: bool,
    observation_outbox_recovered: bool,
    outbox: Option<DurableOutbox>,
    mailbox: Option<Mailbox>,
    logger: Option<Arc<dyn RuntimeLogger>>,
    clock: Arc<dyn Clock>,
    ai_calls_today: u32,
    pending_outbox: Vec<ObservationRecord>,
    outbox_retry_at: Option<Instant>,
    outbox_retry_delay: Duration,
    last_outbox_warning_at: Option<Instant>,
    debug_store: Option<DebugStore>,
    transcript_reconciliation_pending: bool,
    #[cfg(test)]
    fail_retention_after_append: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObserverRole {
    Vision,
    Hearing,
}

impl ObserverAgent {
    pub fn new<C>(provider: Arc<dyn ProviderClient>, config: C) -> Self
    where
        C: Into<AgentConfig>,
    {
        let config = config.into();
        let vision_config = config.clone();
        let limits = ObservationLimits {
            text_total_max_chars: config.text_total_max_chars,
            changes_max_count: config.changes_max_count,
        };
        Self {
            vision_provider: provider.clone(),
            provider,
            hearing_provider: None,
            config,
            vision_config,
            hearing_config: None,
            active_role: ObserverRole::Vision,
            session: None,
            session_calls: 0,
            previous: None,
            limits,
            usage_path: None,
            observation_enabled: false,
            observation_directory: None,
            transcript_directory: None,
            observation_retention_days: None,
            observation_paths: None,
            frame_buffer: None,
            previous_loaded: false,
            observation_outbox_recovered: false,
            outbox: None,
            mailbox: None,
            logger: None,
            clock: Arc::new(SystemClock),
            ai_calls_today: 0,
            pending_outbox: Vec::new(),
            outbox_retry_at: None,
            outbox_retry_delay: Duration::from_secs(1),
            last_outbox_warning_at: None,
            debug_store: None,
            transcript_reconciliation_pending: false,
            #[cfg(test)]
            fail_retention_after_append: false,
        }
    }

    pub fn with_usage_path(mut self, path: PathBuf) -> Self {
        self.usage_path = Some(path);
        self
    }

    pub fn with_hearing_provider<C>(mut self, provider: Arc<dyn ProviderClient>, config: C) -> Self
    where
        C: Into<AgentConfig>,
    {
        self.hearing_provider = Some(provider);
        self.hearing_config = Some(config.into());
        self
    }

    pub fn with_observation_store(mut self, paths: &ConfigPaths, retention_days: u64) -> Self {
        self.configure_observation_store(paths, retention_days);
        self.load_previous_observation(paths);
        self
    }

    pub fn with_observation_store_without_read(
        mut self,
        paths: &ConfigPaths,
        retention_days: u64,
    ) -> Self {
        self.configure_observation_store(paths, retention_days);
        self
    }

    pub(crate) fn mark_audio_consumed(&self, ids: &[String]) -> Result<(), ObserverError> {
        if let Some(paths) = &self.observation_paths {
            mark_audio_consumed(paths, ids)?;
        }
        Ok(())
    }

    fn configure_observation_store(&mut self, paths: &ConfigPaths, retention_days: u64) {
        self.observation_enabled = true;
        self.observation_directory = Some(paths.observations.clone());
        self.transcript_directory = Some(paths.transcripts.clone());
        self.observation_retention_days = Some(retention_days);
        self.observation_paths = Some(paths.clone());
        let frame_buffer = FrameBuffer::new(paths.frame_buffer.clone());
        let _ = frame_buffer.cleanup_expired(self.clock.now());
        self.frame_buffer = Some(frame_buffer);
        self.previous_loaded = false;
        self.transcript_reconciliation_pending = true;
        self.observation_outbox_recovered = false;
        self.outbox =
            Some(DurableOutbox::new(paths.outbox.clone()).with_log_path(paths.log.clone()));
        self.reconcile_transcripts_if_needed();
    }

    fn load_previous_observation(&mut self, paths: &ConfigPaths) {
        self.previous = read_latest_observation(paths, self.limits);
        self.previous_loaded = true;
    }

    pub fn with_mailbox(mut self, mailbox: Mailbox) -> Self {
        self.mailbox = Some(mailbox);
        self
    }

    pub fn with_logger(mut self, logger: Arc<dyn RuntimeLogger>) -> Self {
        self.logger = Some(logger);
        self
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        if let Some(frame_buffer) = &self.frame_buffer {
            let _ = frame_buffer.cleanup_expired(self.clock.now());
        }
        self
    }

    pub fn with_debug_store(mut self, store: DebugStore) -> Self {
        self.debug_store = Some(store);
        self
    }

    pub async fn observe(
        &mut self,
        frames: Vec<ObservationFrameInput>,
        cancellation: CancellationToken,
    ) -> Result<VisualObservation, ObserverError> {
        self.observe_inner(frames, Vec::new(), cancellation, None)
            .await
    }

    pub async fn observe_scoped(
        &mut self,
        frames: Vec<ObservationFrameInput>,
        audio: Vec<AudioObservation>,
        cancellation: CancellationToken,
        expected_generation: u64,
        scope_generation: Arc<std::sync::atomic::AtomicU64>,
        scope_commit_lock: Arc<std::sync::Mutex<()>>,
    ) -> Result<VisualObservation, ObserverError> {
        self.observe_scoped_with_companion_delivery(
            frames,
            audio,
            cancellation,
            ScopedObservationOptions {
                expected_generation,
                scope_generation,
                scope_commit_lock,
                allow_companion_delivery: true,
            },
        )
        .await
    }

    pub(crate) async fn observe_scoped_with_companion_delivery(
        &mut self,
        frames: Vec<ObservationFrameInput>,
        audio: Vec<AudioObservation>,
        cancellation: CancellationToken,
        options: ScopedObservationOptions,
    ) -> Result<VisualObservation, ObserverError> {
        self.observe_inner_with_companion_delivery(
            frames,
            audio,
            cancellation,
            Some((
                options.expected_generation,
                options.scope_generation,
                options.scope_commit_lock,
            )),
            options.allow_companion_delivery,
        )
        .await
    }

    async fn observe_inner(
        &mut self,
        frames: Vec<ObservationFrameInput>,
        audio: Vec<AudioObservation>,
        cancellation: CancellationToken,
        scope_guard: Option<(
            u64,
            Arc<std::sync::atomic::AtomicU64>,
            Arc<std::sync::Mutex<()>>,
        )>,
    ) -> Result<VisualObservation, ObserverError> {
        self.observe_inner_with_companion_delivery(frames, audio, cancellation, scope_guard, true)
            .await
    }

    async fn observe_inner_with_companion_delivery(
        &mut self,
        frames: Vec<ObservationFrameInput>,
        audio: Vec<AudioObservation>,
        cancellation: CancellationToken,
        scope_guard: Option<(
            u64,
            Arc<std::sync::atomic::AtomicU64>,
            Arc<std::sync::Mutex<()>>,
        )>,
        allow_companion_delivery: bool,
    ) -> Result<VisualObservation, ObserverError> {
        require_active_observation(&cancellation)?;
        self.select_role(if frames.is_empty() && !audio.is_empty() {
            ObserverRole::Hearing
        } else {
            ObserverRole::Vision
        });
        self.load_previous_if_needed();
        self.retry_pending_outbox();
        self.reconcile_transcripts_if_needed();
        if let Some(paths) = &self.observation_paths {
            let retention_days = self
                .observation_retention_days
                .ok_or(ObserverError::Output)?;
            for record in &audio {
                record_audio_observation(paths, retention_days, record, self.clock.now())?;
            }
        }
        if frames.is_empty() && audio.is_empty() {
            return Err(ObserverError::Output);
        }
        let speaker_aliases = match &self.observation_paths {
            Some(paths) if !audio.is_empty() || self.previous.is_some() => {
                load_speaker_aliases(paths)?
            }
            _ => SpeakerAliasIndex {
                schema_version: 1,
                registry_id: String::new(),
                aliases: BTreeMap::new(),
            },
        };
        let previous_for_prompt = self
            .previous
            .as_ref()
            .map(|value| sanitize_previous_audio(value, &speaker_aliases));
        let mut source_frame_paths = BTreeMap::new();
        if let Some(frame_buffer) = &self.frame_buffer {
            frame_buffer
                .cleanup_expired(self.clock.now())
                .map_err(ObserverError::FrameBuffer)?;
            for frame in &frames {
                let path = frame_buffer
                    .save_frame(&frame.context_id, &frame.image_path, frame.captured_at)
                    .map_err(ObserverError::FrameBuffer)?;
                source_frame_paths.insert(
                    frame.context_id.clone(),
                    path.to_string_lossy().into_owned(),
                );
            }
        }
        let prompt_frames = frames
            .iter()
            .enumerate()
            .map(|(index, frame)| ObserverPromptFrame {
                index: index + 1,
                display: frame.display,
                window_id: frame.window_id,
                window_bounds: frame.window_bounds,
                relative_seconds: frame.relative_seconds,
                trigger: Some(frame.trigger),
                front_app: frame.front_app.clone(),
                app: frame.app.clone(),
                target: frame.target.clone(),
                ocr_text: frame.ocr_text.clone(),
                focus: frame.focus.clone(),
            })
            .collect::<Vec<_>>();
        let mut prompt = build_observer_prompt(
            &prompt_frames,
            previous_for_prompt.as_ref(),
            self.limits.outline_max_bytes(),
            self.limits.changes_max_count,
        );
        let audio_segments = audio
            .iter()
            .map(|record| {
                let time = DateTime::parse_from_rfc3339(&record.created_at)
                    .map_err(|_| ObserverError::Output)?
                    .with_timezone(&Utc);
                Ok(crate::state::AudioSegmentReference {
                    id: record.id.clone(),
                    time: record.created_at.clone(),
                    source: record.source,
                    transcript_path: self.transcript_directory.as_ref().map(|directory| {
                        directory
                            .join(format!("{}.jsonl", local_date_at(time)))
                            .to_string_lossy()
                            .into_owned()
                    }),
                    speaker_tag: prompt_speaker_id(record, &speaker_aliases),
                    speaker_registry_id: record.speaker_registry_id.clone(),
                    speaker_status: record.speaker_status,
                })
            })
            .collect::<Result<Vec<_>, ObserverError>>()?;
        if !audio.is_empty() {
            let prompt_audio = audio
                .iter()
                .map(|record| PromptAudioSegment {
                    kind: Some(record.kind.clone()),
                    schema_version: Some(record.schema_version),
                    id: record.id.clone(),
                    created_at: record.created_at.clone(),
                    window_start: Some(record.window_start.clone()),
                    window_end: Some(record.window_end.clone()),
                    source: record.source,
                    text: record.text.clone(),
                    speaker_id: prompt_speaker_id(record, &speaker_aliases),
                    speaker_status: record.speaker_status,
                })
                .collect::<Vec<_>>();
            prompt.push_str(&crate::prompts::observer_audio_context(&prompt_audio)?);
        }
        let image_paths: Vec<PathBuf> = frames
            .iter()
            .map(|frame| frame.image_path.clone())
            .collect();
        let mut system_prompt = observer_system_prompt();
        if !audio.is_empty() {
            system_prompt.push_str("\n\n");
            system_prompt.push_str(
                crate::prompts::BUILTIN_OBSERVER_AUDIO_INSTRUCTIONS.trim_end_matches('\n'),
            );
        }
        let debug_call_id = DebugStore::new_id();
        if let Some(store) = &self.debug_store {
            if store
                .record_prompt(
                    "observer",
                    &debug_call_id,
                    &system_prompt,
                    &prompt,
                    self.clock.now(),
                )
                .is_err()
            {
                self.log_debug_failure("observer-prompt");
            }
        }
        let result = self
            .call_provider(
                &system_prompt,
                &prompt,
                &image_paths,
                &debug_call_id,
                cancellation.clone(),
            )
            .await?;
        require_active_observation(&cancellation)?;
        let value = result.value.ok_or(ObserverError::Output)?;
        if let Some(store) = &self.debug_store {
            if store
                .record_response("observer", &debug_call_id, &value, self.clock.now())
                .is_err()
            {
                self.log_debug_failure("observer-response");
            }
        }
        let data = parse_visual_observation(value.clone(), self.limits)
            .map_err(|_| ObserverError::Output)?;
        let created_at = self.clock.now();
        let now = timestamp(created_at);
        let debug_frame_ids = frames
            .iter()
            .filter_map(|frame| frame.debug_id.clone())
            .collect::<Vec<_>>();
        let source_frame_ids = frames
            .iter()
            .map(|frame| frame.context_id.clone())
            .collect::<Vec<_>>();
        let debug_ocr = ocr_preview(Some(
            &frames
                .iter()
                .filter_map(|frame| frame.ocr_text.as_deref())
                .collect::<Vec<_>>()
                .join("\n"),
        ));
        let record = VisualObservation {
            kind: "visual".to_owned(),
            schema_version: 1,
            id: Uuid::new_v4().to_string(),
            created_at: now.clone(),
            window_start: audio
                .first()
                .map_or_else(|| now.clone(), |v| v.created_at.clone()),
            window_end: audio.last().map_or(now, |v| v.created_at.clone()),
            frame_count: frames.len(),
            source_frame_ids,
            audio_segments,
            frames: frames
                .into_iter()
                .map(|frame| ObservationFrame {
                    trigger: frame.trigger,
                    front_app: frame
                        .front_app
                        .map(|value| crate::state::truncate(&value, 300)),
                    app: frame.app.map(|value| crate::state::truncate(&value, 300)),
                    target: crate::state::truncate(&frame.target, 500),
                    ocr_text: frame
                        .ocr_text
                        .map(|value| crate::state::truncate(&value, 2_000)),
                    focus: frame.focus.map(crate::ports::FocusElement::bounded),
                })
                .collect(),
            source_frame_paths,
            data,
        };
        if let Some(store) = &self.debug_store {
            if store
                .record_observer_call(ObserverDebugCall {
                    call_id: &debug_call_id,
                    observation_id: &record.id,
                    frame_ids: debug_frame_ids.clone(),
                    image_files: debug_frame_ids
                        .iter()
                        .map(|id| format!("frame-{id}.png"))
                        .collect(),
                    ocr_preview: debug_ocr,
                    prompt: &prompt,
                    response: &value,
                    created_at,
                })
                .is_err()
            {
                self.log_debug_failure("observer");
            }
        }
        require_active_observation(&cancellation)?;
        let _scope_lock = scope_guard
            .as_ref()
            .map(|(_, _, lock)| lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        if scope_guard.as_ref().is_some_and(|(expected, current, _)| {
            current.load(std::sync::atomic::Ordering::Acquire) != *expected
        }) {
            return Err(ObserverError::StaleScope);
        }
        require_active_observation(&cancellation)?;
        if let Err(error) = self.persist_record_with_companion_delivery(
            &ObservationRecord::Visual(record.clone()),
            allow_companion_delivery,
        ) {
            if record.frame_count > 0 {
                self.previous = serde_json::to_value(&record).ok();
            }
            return Err(error);
        }
        if record.frame_count > 0 {
            self.previous = serde_json::to_value(&record).ok();
        }
        Ok(record)
    }

    pub fn update_config<C>(&mut self, config: C)
    where
        C: Into<AgentConfig>,
    {
        let config = config.into();
        if self.vision_config.provider != config.provider
            || self.vision_config.model != config.model
            || self.vision_config.executable != config.executable
        {
            self.reset_session();
        }
        self.limits = ObservationLimits {
            text_total_max_chars: config.text_total_max_chars,
            changes_max_count: config.changes_max_count,
        };
        self.vision_config = config.clone();
        self.config = config;
        self.active_role = ObserverRole::Vision;
        self.provider = self.vision_provider.clone();
    }

    pub fn update_observer_config(&mut self, config: ObserverConfig) {
        let hearing_config = config.hearing;
        self.update_config(config.vision);
        let hearing_changed = self.hearing_config.as_ref().is_some_and(|current| {
            current.provider != hearing_config.provider
                || current.model != hearing_config.model
                || current.executable != hearing_config.executable
        });
        if hearing_changed {
            self.reset_session();
        }
        if self.hearing_config.is_some() {
            self.hearing_config = Some(hearing_config.into());
        }
    }

    pub fn no_change(&mut self) -> Result<ObservationRecord, ObserverError> {
        self.no_change_with_stagnation(None)
    }

    fn select_role(&mut self, role: ObserverRole) {
        if self.active_role == role {
            return;
        }
        self.reset_session();
        match role {
            ObserverRole::Vision => {
                self.provider = self.vision_provider.clone();
                self.config = self.vision_config.clone();
            }
            ObserverRole::Hearing => {
                self.provider = self
                    .hearing_provider
                    .clone()
                    .unwrap_or_else(|| self.vision_provider.clone());
                self.config = self
                    .hearing_config
                    .clone()
                    .unwrap_or_else(|| self.vision_config.clone());
            }
        }
        self.limits = ObservationLimits {
            text_total_max_chars: self.config.text_total_max_chars,
            changes_max_count: self.config.changes_max_count,
        };
        self.active_role = role;
    }

    fn reconcile_transcripts_if_needed(&mut self) {
        if !self.transcript_reconciliation_pending {
            return;
        }
        let (Some(paths), Some(retention_days)) =
            (&self.observation_paths, self.observation_retention_days)
        else {
            self.transcript_reconciliation_pending = false;
            return;
        };
        match reconcile_transcripts(
            &paths.observations,
            &paths.transcripts,
            self.limits,
            retention_days,
            self.clock.now(),
        ) {
            Ok(()) => self.transcript_reconciliation_pending = false,
            Err(error) => {
                if let Some(logger) = &self.logger {
                    let _ = logger.write(
                        "WARN",
                        &format!(
                            "transcript の再整合を次回へ延期しました: error-type=persistence ({error})"
                        ),
                    );
                }
            }
        }
    }

    pub fn no_change_with_stagnation(
        &mut self,
        stagnation: Option<crate::state::StagnationObservation>,
    ) -> Result<ObservationRecord, ObserverError> {
        self.select_role(ObserverRole::Vision);
        self.load_previous_if_needed();
        self.retry_pending_outbox();
        let (id, now) = stagnation_identity(stagnation.as_ref(), self.clock.now())?;
        let record = ObservationRecord::NoChange(crate::state::NoChangeObservation {
            kind: "no-change".to_owned(),
            schema_version: 1,
            id,
            created_at: now.clone(),
            window_start: now.clone(),
            window_end: now,
            stagnation,
        });
        if let Err(error) = self.persist_record(&record) {
            self.previous = serde_json::to_value(&record).ok();
            return Err(error);
        }
        self.previous = serde_json::to_value(&record).ok();
        Ok(record)
    }

    pub fn ai_calls_today(&self) -> u32 {
        self.ai_calls_today
    }

    fn provider_label(&self) -> &str {
        self.provider
            .provider_name()
            .map_or("unknown", |provider| provider.as_str())
    }

    async fn call_provider(
        &mut self,
        system_prompt: &str,
        prompt: &str,
        image_paths: &[PathBuf],
        debug_call_id: &str,
        cancellation: CancellationToken,
    ) -> Result<ProviderResult, ObserverError> {
        let mut session = self.next_session_request();
        let mut last_error = None;
        for attempt in 0..MAX_OBSERVER_ATTEMPTS {
            require_active_observation(&cancellation)?;
            if let Some(path) = &self.usage_path {
                let date = local_date_at(self.clock.now());
                let role = match self.active_role {
                    ObserverRole::Vision => ObserverCallKind::Vision,
                    ObserverRole::Hearing => ObserverCallKind::Hearing,
                };
                let reservation =
                    try_reserve_observer_role(path, &date, role, self.config.daily_call_limit)?
                        .ok_or(ObserverError::LimitReached)?;
                self.ai_calls_today = reservation.ai_calls;
            }
            self.session_calls = self.session_calls.saturating_add(1);
            let mode = session_mode(&session);
            let started = Instant::now();
            self.log_call_start(mode)?;
            let provider = self.provider.clone();
            let cancellation_must_complete = provider.cancellation_must_complete();
            let mut provider_call = Box::pin(provider.call(
                ProviderCall {
                    system_prompt: system_prompt.to_owned(),
                    prompt: prompt.to_owned(),
                    images: image_paths.iter().cloned().map(Into::into).collect(),
                    tools_disabled: true,
                    output_schema: Some(observer_schema()),
                    output_validation_schema: None,
                    session: session.clone(),
                    model: Some(self.config.model.clone()),
                    effort: Some(self.config.effort.clone()),
                    allow_session_model_change: false,
                    stall_timeout: Duration::from_millis(self.config.stall_timeout_ms),
                    timeout: Duration::from_millis(self.config.timeout_ms),
                    tutorial_response_key: None,
                },
                cancellation.clone(),
            ));
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    if cancellation_must_complete {
                        provider_call.await
                    } else {
                        tokio::time::timeout(Duration::from_millis(100), &mut provider_call)
                            .await
                            .unwrap_or_else(|_| Err(ProviderError {
                                kind: ProviderErrorKind::Retryable,
                                message: "observer provider をキャンセルしました".to_owned(),
                            }))
                    }
                }
                result = &mut provider_call => result,
            };
            if let Err(error) = require_active_observation(&cancellation) {
                self.log_call_cancelled();
                return Err(error);
            }
            match result {
                Ok(result) => {
                    self.log_call_end(mode, started.elapsed().as_millis())?;
                    if let Err(error) = self.accept_session(&session, result.session.clone()) {
                        let detail = error.to_string();
                        self.log_call_failure(
                            mode,
                            ProviderErrorKind::InvalidOutput,
                            Some(&detail),
                        );
                        if matches!(session, SessionRequest::Resume(_))
                            && !cancellation.is_cancelled()
                        {
                            self.reset_session();
                            session = SessionRequest::New;
                            continue;
                        }
                        return Err(error);
                    }
                    return Ok(result);
                }
                Err(error) => {
                    if cancellation.is_cancelled() {
                        self.log_call_cancelled();
                        return Err(error.into());
                    }
                    self.log_call_failure(mode, error.kind, Some(&error.message));
                    if let Some(store) = &self.debug_store {
                        if store
                            .record_provider_error(
                                "observer",
                                debug_call_id,
                                &error.message,
                                self.clock.now(),
                            )
                            .is_err()
                        {
                            self.log_debug_failure("observer-error");
                        }
                    }
                    if matches!(session, SessionRequest::Resume(_)) {
                        self.reset_session();
                        session = SessionRequest::New;
                        continue;
                    }
                    if error.kind.is_retryable() && attempt + 1 < MAX_OBSERVER_ATTEMPTS {
                        last_error = Some(error);
                        tokio::select! {
                            _ = cancellation.cancelled() => return Err(ProviderError {
                                kind: ProviderErrorKind::Retryable,
                                message: "observer がキャンセルされました".to_owned(),
                            }.into()),
                            _ = tokio::time::sleep(Duration::from_millis(200 * (attempt as u64 + 1))) => {}
                        }
                    } else {
                        return Err(error.into());
                    }
                }
            }
        }
        Err(last_error
            .unwrap_or(ProviderError {
                kind: ProviderErrorKind::Retryable,
                message: "observer の呼び出しに失敗しました".to_owned(),
            })
            .into())
    }

    fn next_session_request(&mut self) -> SessionRequest {
        if self.session.is_some() && self.session_calls >= OBSERVER_SESSION_MAX_CALLS {
            self.reset_session();
        }
        self.session
            .clone()
            .map_or(SessionRequest::New, SessionRequest::Resume)
    }

    fn reset_session(&mut self) {
        self.session = None;
        self.session_calls = 0;
    }

    fn accept_session(
        &mut self,
        request: &SessionRequest,
        returned: Option<ProviderSession>,
    ) -> Result<(), ObserverError> {
        match (request, returned) {
            (SessionRequest::New, Some(session)) => {
                self.validate_returned_session(&session)?;
                self.session = Some(session);
            }
            (SessionRequest::New, None) => self.session = None,
            (SessionRequest::Resume(expected), Some(session)) => {
                self.validate_returned_session(&session)?;
                if session.provider != expected.provider {
                    return Err(ObserverError::Provider(ProviderError {
                        kind: ProviderErrorKind::InvalidOutput,
                        message: "observer provider の session provider が一致しません".to_owned(),
                    }));
                }
                self.session = Some(session);
            }
            (SessionRequest::Resume(_), None) => {}
            (SessionRequest::Ephemeral | SessionRequest::Isolated, _) => {}
        }
        Ok(())
    }

    fn validate_returned_session(&self, session: &ProviderSession) -> Result<(), ObserverError> {
        if session.id.trim().is_empty() {
            return Err(ObserverError::Provider(ProviderError {
                kind: ProviderErrorKind::InvalidOutput,
                message: "observer provider の session id が空です".to_owned(),
            }));
        }
        if self
            .provider
            .provider_name()
            .is_some_and(|provider| provider != session.provider)
        {
            return Err(ObserverError::Provider(ProviderError {
                kind: ProviderErrorKind::InvalidOutput,
                message: "observer provider の session provider が一致しません".to_owned(),
            }));
        }
        let expected_model = (self.config.model != "default").then_some(self.config.model.as_str());
        if session.model.as_deref() != expected_model {
            return Err(ObserverError::Provider(ProviderError {
                kind: ProviderErrorKind::InvalidOutput,
                message: "observer provider の session model が一致しません".to_owned(),
            }));
        }
        Ok(())
    }

    fn log_call_start(&self, mode: &str) -> Result<(), ObserverError> {
        if let Some(logger) = &self.logger {
            logger.write(
                "INFO",
                &format!(
                    "観察AI呼び出し開始: provider={} mode={mode}",
                    self.provider_label()
                ),
            )?;
        }
        Ok(())
    }

    fn log_call_end(&self, mode: &str, elapsed_ms: u128) -> Result<(), ObserverError> {
        if let Some(logger) = &self.logger {
            logger.write(
                "INFO",
                &format!(
                    "観察AI呼び出し終了: provider={} mode={mode} elapsed-ms={elapsed_ms}",
                    self.provider_label()
                ),
            )?;
        }
        Ok(())
    }

    fn log_call_cancelled(&self) {
        if let Some(logger) = &self.logger {
            let _ = logger.write(
                "INFO",
                "観察AI呼び出し中断: reason=runtime-operation-cancel",
            );
        }
    }

    fn log_call_failure(&self, mode: &str, kind: ProviderErrorKind, detail: Option<&str>) {
        if let Some(logger) = &self.logger {
            let detail = detail.map_or(String::new(), |value| format!(" detail={value}"));
            let _ = logger.write(
                "WARN",
                &format!(
                    "観察AI呼び出し失敗: provider={} mode={mode} error-type={}{}",
                    self.provider_label(),
                    kind.as_str(),
                    detail,
                ),
            );
        }
    }

    fn log_debug_failure(&self, stage: &str) {
        if let Some(logger) = &self.logger {
            let _ = logger.write(
                "WARN",
                &format!("デバッグ記録に失敗しました: stage={stage} error-type=debug-persistence"),
            );
        }
    }

    pub fn retry_pending_outbox(&mut self) {
        if self
            .outbox_retry_at
            .is_some_and(|deadline| deadline > Instant::now())
        {
            return;
        }
        let pending = mem::take(&mut self.pending_outbox);
        let mut failed = false;
        for record in pending {
            match self.frame_context_delivery_decision(&record) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(_) => {
                    failed = true;
                    self.queue_pending_outbox(record);
                    continue;
                }
            }
            if let Err(_error) = self.persist_mailbox(&record) {
                failed = true;
                self.queue_pending_outbox(record);
            }
        }
        if failed {
            self.log_outbox_failure("retry");
            self.outbox_retry_at = Some(Instant::now() + self.outbox_retry_delay);
            self.outbox_retry_delay = (self.outbox_retry_delay * 2).min(Duration::from_secs(30));
        } else {
            self.outbox_retry_at = None;
            self.outbox_retry_delay = Duration::from_secs(1);
        }
    }

    fn load_previous_if_needed(&mut self) {
        if !self.observation_enabled {
            return;
        }
        if let Some(paths) = self.observation_paths.clone() {
            if !self.previous_loaded {
                self.load_previous_observation(&paths);
            }
            if self.mailbox.is_some() && !self.observation_outbox_recovered {
                self.observation_outbox_recovered = self.recover_observation_outbox(&paths);
            }
        }
    }

    fn recover_observation_outbox(&mut self, paths: &ConfigPaths) -> bool {
        let Some(outbox) = self.outbox.clone() else {
            return true;
        };
        let Some(mailbox) = self.mailbox.clone() else {
            return true;
        };
        let Ok(entries) = fs::read_dir(&paths.observations) else {
            return true;
        };
        let mut records = Vec::new();
        let mut ids = HashSet::new();
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                continue;
            }
            let values = match JsonlStore::new(path).read::<Value>() {
                Ok(values) => values,
                Err(_) => continue,
            };
            for value in values {
                let Ok(record) = parse_observation(value, self.limits) else {
                    continue;
                };
                if matches!(record, ObservationRecord::Audio(_)) {
                    continue;
                }
                if ids.insert(record.id().to_owned()) {
                    records.push(record);
                }
            }
        }
        for record in records {
            match self.frame_context_delivery_decision(&record) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(_) => {
                    self.log_outbox_failure("recovery-frame-context");
                    return false;
                }
            }
            match outbox.contains("observation", record.id()) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(_) => {
                    self.log_outbox_failure("recovery-contains");
                    return false;
                }
            }
            let payload = match serde_json::to_value(&record) {
                Ok(payload) => payload,
                Err(_) => continue,
            };
            let recipients = vec![mailbox.recipient_name().to_owned()];
            match outbox.enqueue(
                record.id(),
                record.created_at(),
                "observation",
                payload,
                &recipients,
            ) {
                Ok(_) => {}
                Err(_) => {
                    self.log_outbox_failure("recovery-enqueue");
                    return false;
                }
            }
        }
        if outbox.deliver_pending(mailbox.root_path()).is_err() {
            self.log_outbox_failure("recovery-delivery");
            return false;
        }
        true
    }

    fn persist_record(&mut self, record: &ObservationRecord) -> Result<(), ObserverError> {
        self.persist_record_with_companion_delivery(record, true)
    }

    fn persist_record_with_companion_delivery(
        &mut self,
        record: &ObservationRecord,
        allow_companion_delivery: bool,
    ) -> Result<(), ObserverError> {
        if self.observation_enabled {
            let directory = self.observation_directory.clone().ok_or_else(|| {
                ObserverError::Persistence(PersistenceError::Invalid(
                    "observations の保存先がありません".to_owned(),
                ))
            })?;
            append_observation_record(&directory, record)?;
            if let Some(retention_days) = self.observation_retention_days {
                self.maintain_observation_retention(&directory, retention_days);
            }
        }
        if !allow_companion_delivery {
            return Ok(());
        }
        match self.frame_context_delivery_decision(record) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) => {
                self.log_frame_context_failure(&error);
                self.queue_pending_outbox(record.clone());
                return Ok(());
            }
        }
        if self.mailbox.is_some() {
            if let Err(_error) = self.persist_mailbox(record) {
                self.log_outbox_failure("enqueue");
                self.queue_pending_outbox(record.clone());
                return Err(ObserverError::OutboxPending {
                    record: Box::new(record.clone()),
                });
            }
        }
        Ok(())
    }

    fn frame_context_delivery_decision(
        &self,
        record: &ObservationRecord,
    ) -> Result<bool, PersistenceError> {
        if record.source_frame_ids().is_empty() {
            return Ok(false);
        }
        let Some(paths) = &self.observation_paths else {
            return Ok(false);
        };
        crate::companion_storage::CompanionStorage::from_paths(paths, 1)
            .observation_claimed_by_user(record)
    }

    fn log_frame_context_failure(&self, error: &PersistenceError) {
        if let Some(logger) = &self.logger {
            let _ = logger.write(
                "WARN",
                &format!(
                    "処理待ち画面の配達状態を確定できませんでした: error-type=persistence ({error})"
                ),
            );
        }
    }

    fn maintain_observation_retention(&mut self, directory: &std::path::Path, retention_days: u64) {
        if self
            .prune_observation_retention(directory, retention_days)
            .is_err()
        {
            if let Some(logger) = &self.logger {
                let _ = logger.write(
                    "WARN",
                    "観察 journal の保守を次回へ延期しました: error-type=retention",
                );
            }
        }
    }

    fn prune_observation_retention(
        &mut self,
        directory: &std::path::Path,
        retention_days: u64,
    ) -> Result<(), PersistenceError> {
        #[cfg(test)]
        if mem::take(&mut self.fail_retention_after_append) {
            return Err(PersistenceError::Invalid(
                "post-append retention failpoint".to_owned(),
            ));
        }
        prune_daily_jsonl_at(
            directory,
            retention_days,
            50 * 1024 * 1024,
            self.clock.now(),
        )
    }

    fn persist_mailbox(&mut self, record: &ObservationRecord) -> Result<(), ObserverError> {
        let Some(mailbox) = &self.mailbox else {
            return Ok(());
        };
        let payload = serde_json::to_value(record)?;
        if let Some(outbox) = &self.outbox {
            let recipients = vec![mailbox.recipient_name().to_owned()];
            outbox.enqueue(
                record.id(),
                record.created_at(),
                "observation",
                payload,
                &recipients,
            )?;
            if outbox.deliver_pending(mailbox.root_path()).is_err() {
                self.log_outbox_failure("delivery");
            }
        } else {
            mailbox.publish_with_identity(
                "observation".to_owned(),
                record.id().to_owned(),
                record.created_at().to_owned(),
                payload,
            )?;
        }
        Ok(())
    }

    fn queue_pending_outbox(&mut self, record: ObservationRecord) {
        if !self
            .pending_outbox
            .iter()
            .any(|pending| pending.id() == record.id())
        {
            self.pending_outbox.push(record);
        }
    }

    fn log_outbox_failure(&mut self, stage: &str) {
        let now = Instant::now();
        if self
            .last_outbox_warning_at
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(60))
        {
            return;
        }
        self.last_outbox_warning_at = Some(now);
        if let Some(logger) = &self.logger {
            let _ = logger.write(
                "WARN",
                &format!("observer mailbox配信を保留しました: error-type=mailbox stage={stage}"),
            );
        }
    }
}

