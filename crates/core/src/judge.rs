use crate::config::{JudgeComposition, JudgeConfig};
use crate::interactive_process::{
    InteractiveProcess, InteractiveProcessError, InteractiveProcessEvent,
    InteractiveProcessRequest, ProcessCompletion,
};
use crate::observer::ObservationFrameInput;
use crate::persistence::atomic_write_json;
use crate::ports::RuntimeLogger;
use crate::process::{ProcessError, ProcessRequest, ProcessRunner};
use crate::state::{AudioObservation, ObservationRecord};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const JUDGE_PROTOCOL_VERSION: u8 = 1;
pub const JUDGE_FEATURE_SCHEMA: &str = "coosenpai-observation-v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum JudgeAction {
    Notify,
    Hold,
    Silence,
}

impl JudgeAction {
    fn allows_companion(self) -> bool {
        matches!(self, Self::Notify)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum JudgeMode {
    Shadow,
    Follow,
    PassThrough,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum JudgeFeedSign {
    Positive,
    Negative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JudgeFeedApplyResult {
    Applied,
    NotApplied,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum JudgeFeedSource {
    Automatic,
    Explicit,
}

pub(crate) struct JudgeFeedEvent<'a> {
    pub(crate) event_id: &'a str,
    pub(crate) input_id: &'a str,
    pub(crate) event_time: Option<&'a str>,
    pub(crate) sign: JudgeFeedSign,
    pub(crate) strength: f64,
    pub(crate) source: JudgeFeedSource,
    pub(crate) cancelled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FeedbackState {
    input_id: String,
    event_time: Option<String>,
    sign: JudgeFeedSign,
    strength: f64,
    source: JudgeFeedSource,
    cancelled: bool,
    revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "status")]
enum FeedbackDeliveryStatus {
    Pending,
    Sent { applied: bool },
    Failed { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedbackDeliveryRecord {
    event_id: String,
    input_id: String,
    event_time: String,
    sign: JudgeFeedSign,
    strength: f64,
    source: JudgeFeedSource,
    cancelled: bool,
    revision: u64,
    attempt: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retryable: Option<bool>,
    #[serde(default, skip_serializing_if = "is_false")]
    uncertain: bool,
    #[serde(flatten)]
    status: FeedbackDeliveryStatus,
}

#[derive(Debug, Clone, Default)]
struct FeedbackLedger {
    events: HashMap<String, FeedbackState>,
    history: Vec<FeedbackDeliveryRecord>,
}

const FEEDBACK_LEDGER_SCHEMA_VERSION: u8 = 2;
const MAX_FEEDBACK_EVENTS: usize = 4_096;
const MAX_FEEDBACK_HISTORY: usize = 16_384;
const INITIAL_RESTART_BACKOFF: Duration = Duration::from_millis(100);
const MAX_RESTART_BACKOFF: Duration = Duration::from_secs(5);
const JUDGE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(4);

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedFeedbackLedger {
    schema_version: u8,
    events: HashMap<String, FeedbackState>,
    #[serde(default)]
    history: Vec<FeedbackDeliveryRecord>,
}

#[derive(Debug, Clone, Copy)]
struct FeedbackReservation {
    revision: u64,
    attempt: u64,
}

impl FeedbackState {
    fn from_event(event: &JudgeFeedEvent<'_>, revision: u64) -> Self {
        Self {
            input_id: event.input_id.to_owned(),
            event_time: event.event_time.map(str::to_owned),
            sign: event.sign,
            strength: event.strength,
            source: event.source,
            cancelled: event.cancelled,
            revision,
        }
    }

    fn matches(&self, event: &JudgeFeedEvent<'_>) -> bool {
        self.input_id == event.input_id
            && self.event_time.as_deref() == event.event_time
            && self.sign == event.sign
            && self.strength.to_bits() == event.strength.to_bits()
            && self.source == event.source
            && self.cancelled == event.cancelled
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JudgeDecision {
    pub sequence: u64,
    pub occurred_at: String,
    pub input_id: String,
    pub novelty: Option<f64>,
    pub relevance: Option<f64>,
    pub action: JudgeAction,
    pub readiness: String,
    pub mode: JudgeMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_reason: Option<String>,
}

impl PartialEq for JudgeDecision {
    fn eq(&self, other: &Self) -> bool {
        self.sequence == other.sequence
            && self.occurred_at == other.occurred_at
            && self.input_id == other.input_id
            && equal_score(self.novelty, other.novelty)
            && equal_score(self.relevance, other.relevance)
            && self.action == other.action
            && self.readiness == other.readiness
            && self.mode == other.mode
            && self.hold_reason == other.hold_reason
            && self.fallback_reason == other.fallback_reason
    }
}

impl Eq for JudgeDecision {}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JudgeTrace {
    pub input_id: String,
    pub request: Value,
    pub responses: Vec<JudgeModuleTrace>,
    pub decision: Option<JudgeDecision>,
    #[serde(default)]
    pub feedable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JudgeModuleTrace {
    pub module_index: usize,
    pub response: Option<Value>,
    pub evaluation: Option<JudgeModuleEvaluation>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JudgeModuleEvaluation {
    pub novelty: Option<f64>,
    pub relevance: Option<f64>,
    pub action: JudgeAction,
    pub readiness: String,
}

const MAX_JUDGE_TRACES: usize = 1_024;
const MAX_JUDGE_OBSERVATION_LINKS: usize = 4_096;
const MAX_PENDING_JUDGE_OBSERVATIONS: usize = 4_096;

#[derive(Debug, Clone, Default)]
pub struct JudgeTraceStore {
    entries: Arc<Mutex<HashMap<String, JudgeTrace>>>,
    order: Arc<Mutex<VecDeque<String>>>,
    observation_inputs: Arc<Mutex<HashMap<String, String>>>,
    observation_order: Arc<Mutex<VecDeque<String>>>,
    pending_observations: Arc<Mutex<VecDeque<(String, String)>>>,
}

impl JudgeTraceStore {
    pub fn get(&self, input_id: &str) -> Option<JudgeTrace> {
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(input_id).cloned())
    }

    pub fn get_for_observation(&self, observation_id: &str) -> Option<JudgeTrace> {
        let input_id = self
            .observation_inputs
            .lock()
            .ok()
            .and_then(|inputs| inputs.get(observation_id).cloned())?;
        self.get(&input_id)
    }

    pub fn associate_observation(&self, observation: &ObservationRecord) {
        let observation_id = observation.id().to_owned();
        let input_ids = observation_input_ids(observation);
        if input_ids.is_empty() {
            return;
        }
        let input_id = self.entries.lock().ok().and_then(|entries| {
            input_ids
                .iter()
                .find(|input_id| entries.contains_key(*input_id))
                .cloned()
        });
        if let Some(input_id) = input_id {
            self.remember_observation(&observation_id, &input_id);
            return;
        }
        let Ok(mut pending) = self.pending_observations.lock() else {
            return;
        };
        pending.retain(|(_, pending_id)| pending_id != &observation_id);
        for input_id in input_ids {
            while pending.len() >= MAX_PENDING_JUDGE_OBSERVATIONS {
                pending.pop_front();
            }
            pending.push_back((input_id, observation_id.clone()));
        }
    }

    fn insert(&self, trace: JudgeTrace) {
        let input_id = trace.input_id.clone();
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        let Ok(mut order) = self.order.lock() else {
            return;
        };
        let is_new = !entries.contains_key(&input_id);
        order.retain(|value| value != &input_id);
        while is_new && entries.len() >= MAX_JUDGE_TRACES {
            let Some(oldest) = order.pop_front() else {
                break;
            };
            entries.remove(&oldest);
        }
        order.push_back(input_id.clone());
        entries.insert(input_id.clone(), trace);
        drop(order);
        drop(entries);

        let Ok(mut pending) = self.pending_observations.lock() else {
            return;
        };
        let mut resolved = Vec::new();
        pending.retain(|(pending_input_id, observation_id)| {
            if pending_input_id == &input_id {
                resolved.push(observation_id.clone());
                false
            } else {
                true
            }
        });
        pending.retain(|(_, observation_id)| {
            !resolved
                .iter()
                .any(|resolved_id| resolved_id == observation_id)
        });
        drop(pending);
        for observation_id in resolved {
            self.remember_observation(&observation_id, &input_id);
        }
    }

    pub(crate) fn insert_for_fixture(&self, trace: JudgeTrace) {
        self.insert(trace);
    }

    fn remember_observation(&self, observation_id: &str, input_id: &str) {
        let Ok(mut inputs) = self.observation_inputs.lock() else {
            return;
        };
        let Ok(mut order) = self.observation_order.lock() else {
            return;
        };
        let is_new = !inputs.contains_key(observation_id);
        order.retain(|value| value != observation_id);
        while is_new && inputs.len() >= MAX_JUDGE_OBSERVATION_LINKS {
            let Some(oldest) = order.pop_front() else {
                break;
            };
            inputs.remove(&oldest);
        }
        order.push_back(observation_id.to_owned());
        inputs.insert(observation_id.to_owned(), input_id.to_owned());
    }
}

fn observation_input_ids(observation: &ObservationRecord) -> Vec<String> {
    let mut input_ids = Vec::new();
    let mut push = |value: &str| {
        if !input_ids.iter().any(|existing| existing == value) {
            input_ids.push(value.to_owned());
        }
    };
    match observation {
        ObservationRecord::Visual(value) => {
            for input_id in &value.source_frame_ids {
                push(input_id);
            }
            for input_id in value.source_frame_paths.keys() {
                push(input_id);
            }
            for segment in &value.audio_segments {
                push(&segment.id);
            }
        }
        ObservationRecord::Audio(value) => push(&value.id),
        ObservationRecord::NoChange(_) => {}
    }
    input_ids
}

#[derive(Debug, Clone)]
pub struct JudgeEvaluation {
    pub decision: Option<JudgeDecision>,
    pub companion_delivery: bool,
}

impl JudgeEvaluation {
    fn pass_through() -> Self {
        Self {
            decision: None,
            companion_delivery: true,
        }
    }
}

#[derive(Debug, Error)]
pub enum JudgeError {
    #[error("判断役の応答を JSON として読めません: {0}")]
    Json(#[from] serde_json::Error),
    #[error("判断役の通信契約が不正です: {0}")]
    Protocol(String),
    #[error("判断役が要求を拒否しました: code={code} message={message}")]
    PluginRejected { code: String, message: String },
    #[error("判断役 process に失敗しました: {0}")]
    Process(String),
    #[error("判断役への入力が不正です: {0}")]
    Input(String),
    #[error("判断役への通信がキャンセルされました")]
    Cancelled,
}

impl JudgeError {
    fn should_reset_transport(&self) -> bool {
        match self {
            Self::Json(_) | Self::Protocol(_) | Self::Process(_) => true,
            Self::PluginRejected { code, .. } => !matches!(
                plugin_failure_disposition(code),
                PluginFailureDisposition::NonRetryable
            ),
            Self::Input(_) | Self::Cancelled => false,
        }
    }

    fn failure_metadata(&self) -> (Option<String>, Option<String>, Option<bool>, bool) {
        match self {
            Self::PluginRejected { code, message } => {
                let disposition = plugin_failure_disposition(code);
                let retryable = match disposition {
                    PluginFailureDisposition::Retryable => Some(true),
                    PluginFailureDisposition::NonRetryable => Some(false),
                    PluginFailureDisposition::PersistenceUncertain => None,
                };
                (
                    Some(code.clone()),
                    Some(message.clone()),
                    retryable,
                    disposition == PluginFailureDisposition::PersistenceUncertain,
                )
            }
            Self::Json(error) => (None, Some(error.to_string()), Some(true), false),
            Self::Protocol(message) | Self::Process(message) => {
                (None, Some(message.clone()), Some(true), false)
            }
            Self::Input(message) => (None, Some(message.clone()), Some(false), false),
            Self::Cancelled => (
                None,
                Some("キャンセルされました".to_owned()),
                Some(false),
                false,
            ),
        }
    }

    fn is_persistence_uncertain(&self) -> bool {
        matches!(
            self,
            Self::PluginRejected { code, .. }
                if plugin_failure_disposition(code)
                    == PluginFailureDisposition::PersistenceUncertain
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PluginFailureDisposition {
    Retryable,
    NonRetryable,
    PersistenceUncertain,
}

fn plugin_failure_disposition(code: &str) -> PluginFailureDisposition {
    match code {
        "observation_ledger_capacity_exceeded"
        | "observation_body_capacity_exceeded"
        | "feedback_capacity_exceeded"
        | "history_stream_capacity_exceeded"
        | "state_size_exceeded"
        | "observation_expired"
        | "observation_not_feedable"
        | "observation_replay_mismatch"
        | "observation_replay_unverifiable" => PluginFailureDisposition::NonRetryable,
        "persistence_uncertain" => PluginFailureDisposition::PersistenceUncertain,
        "timeout" | "request_error" => PluginFailureDisposition::Retryable,
        _ => PluginFailureDisposition::Retryable,
    }
}

#[async_trait]
pub(crate) trait JudgeTransport: Send + Sync {
    async fn request(
        &self,
        request: Vec<u8>,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> Result<Vec<u8>, JudgeError>;

    async fn reset(&self) {}

    async fn wait_for_shutdown(&self) {}
}

pub(crate) struct JudgeStartGate {
    open: AtomicBool,
    notify: Notify,
}

impl JudgeStartGate {
    fn open() -> Arc<Self> {
        Arc::new(Self {
            open: AtomicBool::new(true),
            notify: Notify::new(),
        })
    }

    fn closed() -> Arc<Self> {
        Arc::new(Self {
            open: AtomicBool::new(false),
            notify: Notify::new(),
        })
    }

    fn release(&self) {
        self.open.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn wait(
        &self,
        deadline: tokio::time::Instant,
        cancellation: &CancellationToken,
    ) -> Result<(), JudgeError> {
        if self.open.load(Ordering::Acquire) {
            return Ok(());
        }
        tokio::time::timeout_at(deadline, async {
            loop {
                if self.open.load(Ordering::Acquire) {
                    return Ok(());
                }
                tokio::select! {
                    _ = cancellation.cancelled() => return Err(JudgeError::Cancelled),
                    _ = self.notify.notified() => {}
                }
            }
        })
        .await
        .map_err(|_| {
            JudgeError::Process("前の判断役の停止完了待ちが timeout しました".to_owned())
        })?
    }

    async fn wait_until_open(&self) {
        while !self.open.load(Ordering::Acquire) {
            self.notify.notified().await;
        }
    }
}

struct ResidentJudgeTransport {
    request: InteractiveProcessRequest,
    state: Arc<tokio::sync::Mutex<ResidentJudgeState>>,
    reset_requested: Arc<AtomicBool>,
    start_gate: Arc<JudgeStartGate>,
}

struct StoppingProcess {
    completion: ProcessCompletion,
    task: tokio::task::JoinHandle<()>,
}

struct ResidentJudgeState {
    process: Option<InteractiveProcess>,
    stopping: Option<StoppingProcess>,
    restart_at: Option<tokio::time::Instant>,
    restart_backoff: Duration,
}

impl ResidentJudgeTransport {
    fn new(module: &crate::config::JudgeModuleConfig, start_gate: Arc<JudgeStartGate>) -> Self {
        Self {
            request: InteractiveProcessRequest {
                executable: PathBuf::from(&module.executable),
                args: module.arguments.clone(),
                env: module
                    .environment
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
                cwd: None,
            },
            state: Arc::new(tokio::sync::Mutex::new(ResidentJudgeState {
                process: None,
                stopping: None,
                restart_at: None,
                restart_backoff: INITIAL_RESTART_BACKOFF,
            })),
            reset_requested: Arc::new(AtomicBool::new(false)),
            start_gate,
        }
    }

    fn start_stopping(&self, state: &mut ResidentJudgeState) {
        if !self.reset_requested.load(Ordering::Acquire) || state.stopping.is_some() {
            return;
        }
        if let Some(process) = state.process.take() {
            let completion = process.completion();
            let task = tokio::spawn(process.shutdown());
            state.stopping = Some(StoppingProcess { completion, task });
            schedule_restart(state);
        }
    }

    async fn wait_for_stopping(
        &self,
        state: &mut ResidentJudgeState,
        deadline: tokio::time::Instant,
        cancellation: &CancellationToken,
    ) -> Result<(), JudgeError> {
        self.start_stopping(state);
        let Some(stopping) = state.stopping.take() else {
            self.reset_requested.store(false, Ordering::Release);
            return Ok(());
        };
        let completion = stopping.completion.clone();
        let result = tokio::time::timeout_at(deadline, async {
            tokio::select! {
                _ = cancellation.cancelled() => Err(JudgeError::Cancelled),
                () = completion.wait() => Ok(()),
            }
        })
        .await;
        match result {
            Ok(Ok(())) => {
                self.reset_requested.store(false, Ordering::Release);
                if stopping.task.is_finished() {
                    let _ = stopping.task.await;
                }
                Ok(())
            }
            Ok(Err(error)) => {
                state.stopping = Some(stopping);
                Err(error)
            }
            Err(_) => {
                state.stopping = Some(stopping);
                Err(JudgeError::Process(
                    "判断役の停止完了待ちが timeout しました".to_owned(),
                ))
            }
        }
    }
}

#[async_trait]
impl JudgeTransport for ResidentJudgeTransport {
    async fn request(
        &self,
        request: Vec<u8>,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> Result<Vec<u8>, JudgeError> {
        let deadline = tokio::time::Instant::now() + timeout;
        self.start_gate.wait(deadline, &cancellation).await?;
        let mut state = self.state.lock().await;
        if self.reset_requested.load(Ordering::Acquire) {
            self.wait_for_stopping(&mut state, deadline, &cancellation)
                .await?;
        }
        if state.process.is_none() {
            if let Some(restart_at) = state.restart_at {
                let restart_wait = async {
                    tokio::select! {
                        _ = cancellation.cancelled() => Err(JudgeError::Cancelled),
                        () = tokio::time::sleep_until(restart_at) => Ok(()),
                    }
                };
                tokio::time::timeout_at(deadline, restart_wait)
                    .await
                    .map_err(|_| {
                        JudgeError::Process("判断役の再起動待ちが timeout しました".to_owned())
                    })??;
            }
            let spawn = InteractiveProcess::spawn(self.request.clone(), CancellationToken::new());
            let child = match tokio::time::timeout_at(deadline, async {
                tokio::select! {
                    _ = cancellation.cancelled() => Err(JudgeError::Cancelled),
                    result = spawn => result.map_err(interactive_error),
                }
            })
            .await
            {
                Ok(Ok(child)) => child,
                Ok(Err(error)) => {
                    schedule_restart(&mut state);
                    return Err(error);
                }
                Err(_) => {
                    schedule_restart(&mut state);
                    return Err(JudgeError::Process(
                        "判断役の起動が timeout しました".to_owned(),
                    ));
                }
            };
            state.process = Some(child);
        }
        let result = {
            let child = state
                .process
                .as_mut()
                .expect("resident judge process was initialized");
            request_resident_process(child, request, deadline, cancellation).await
        };
        if result.is_ok() {
            state.restart_at = None;
            state.restart_backoff = INITIAL_RESTART_BACKOFF;
        }
        result
    }

    async fn reset(&self) {
        if self.reset_requested.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut state = self.state.lock().await;
        self.start_stopping(&mut state);
        if state.process.is_none() && state.stopping.is_none() {
            self.reset_requested.store(false, Ordering::Release);
        }
    }

    async fn wait_for_shutdown(&self) {
        self.reset().await;
        let mut state = self.state.lock().await;
        self.start_stopping(&mut state);
        let Some(completion) = state
            .stopping
            .as_ref()
            .map(|stopping| stopping.completion.clone())
        else {
            self.reset_requested.store(false, Ordering::Release);
            return;
        };
        completion.wait().await;
        let stopping = state
            .stopping
            .take()
            .expect("stopping process disappeared after completion");
        self.reset_requested.store(false, Ordering::Release);
        drop(state);
        let _ = stopping.task.await;
    }
}

fn schedule_restart(state: &mut ResidentJudgeState) {
    state.restart_at = Some(tokio::time::Instant::now() + state.restart_backoff);
    state.restart_backoff = (state.restart_backoff * 2).min(MAX_RESTART_BACKOFF);
}

async fn request_resident_process(
    process: &mut InteractiveProcess,
    request: Vec<u8>,
    deadline: tokio::time::Instant,
    cancellation: CancellationToken,
) -> Result<Vec<u8>, JudgeError> {
    let control = process.control();
    let write = control.write_line(request);
    match tokio::time::timeout_at(deadline, async {
        tokio::select! {
            _ = cancellation.cancelled() => Err(JudgeError::Cancelled),
            result = write => result.map_err(interactive_error),
        }
    })
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            return Err(JudgeError::Process(
                "要求の送信が timeout しました".to_owned(),
            ))
        }
    }

    let read = async {
        loop {
            let event = tokio::select! {
                _ = cancellation.cancelled() => return Err(JudgeError::Cancelled),
                event = process.next_event() => event,
            };
            match event {
                Some(Ok(InteractiveProcessEvent::StdoutLine(line))) => return Ok(line),
                Some(Ok(InteractiveProcessEvent::StderrLine(_))) => {}
                Some(Ok(InteractiveProcessEvent::Exited { status, .. })) => {
                    return Err(JudgeError::Process(format!(
                        "判断役が終了しました status={status:?}"
                    )))
                }
                Some(Err(error)) => return Err(interactive_error(error)),
                None => return Err(JudgeError::Process("判断役の通信が閉じました".to_owned())),
            }
        }
    };
    tokio::time::timeout_at(deadline, read)
        .await
        .map_err(|_| JudgeError::Process("応答待ちが timeout しました".to_owned()))?
}

fn interactive_error(error: InteractiveProcessError) -> JudgeError {
    match error {
        InteractiveProcessError::Cancelled => JudgeError::Cancelled,
        other => JudgeError::Process(other.to_string()),
    }
}

struct RunnerJudgeTransport {
    request: InteractiveProcessRequest,
    runner: Arc<dyn ProcessRunner>,
}

#[async_trait]
impl JudgeTransport for RunnerJudgeTransport {
    async fn request(
        &self,
        request: Vec<u8>,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> Result<Vec<u8>, JudgeError> {
        let output = self
            .runner
            .run(
                ProcessRequest {
                    executable: self.request.executable.clone(),
                    args: self.request.args.clone(),
                    env: self.request.env.clone(),
                    cwd: self.request.cwd.clone(),
                    stdin: request,
                    timeout,
                },
                cancellation,
            )
            .await
            .map_err(process_error)?;
        Ok(output.stdout)
    }
}

fn process_error(error: ProcessError) -> JudgeError {
    match error {
        ProcessError::Cancelled => JudgeError::Cancelled,
        other => JudgeError::Process(other.to_string()),
    }
}

async fn request_with_deadline(
    transport: &Arc<dyn JudgeTransport>,
    request: Vec<u8>,
    deadline: tokio::time::Instant,
    cancellation: CancellationToken,
    operation: &str,
) -> Result<Vec<u8>, JudgeError> {
    let timeout = deadline.saturating_duration_since(tokio::time::Instant::now());
    if timeout.is_zero() {
        return Err(JudgeError::Process(format!(
            "判断役の{operation}が timeout しました"
        )));
    }
    tokio::time::timeout_at(deadline, transport.request(request, timeout, cancellation))
        .await
        .map_err(|_| JudgeError::Process(format!("判断役の{operation}が timeout しました")))?
}

#[derive(Clone)]
pub struct JudgeAgent {
    config: JudgeConfig,
    transports: Arc<Vec<Arc<dyn JudgeTransport>>>,
    logger: Option<Arc<dyn RuntimeLogger>>,
    session: String,
    sequence: Arc<AtomicU64>,
    feedback_ledger: Arc<Mutex<FeedbackLedger>>,
    feedback_send_lock: Arc<tokio::sync::Mutex<()>>,
    feedback_store: Option<PathBuf>,
    feedback_store_load_error: Option<String>,
    trace_store: JudgeTraceStore,
}

impl JudgeAgent {
    pub fn new(config: JudgeConfig) -> Self {
        Self::new_with_start_gate(config, JudgeStartGate::open())
    }

    pub(crate) fn new_with_start_gate(
        config: JudgeConfig,
        start_gate: Arc<JudgeStartGate>,
    ) -> Self {
        let transports = config
            .modules
            .iter()
            .map(|module| {
                Arc::new(ResidentJudgeTransport::new(module, start_gate.clone()))
                    as Arc<dyn JudgeTransport>
            })
            .collect();
        Self {
            config,
            transports: Arc::new(transports),
            logger: None,
            session: Uuid::new_v4().to_string(),
            sequence: Arc::new(AtomicU64::new(0)),
            feedback_ledger: Arc::new(Mutex::new(FeedbackLedger::default())),
            feedback_send_lock: Arc::new(tokio::sync::Mutex::new(())),
            feedback_store: None,
            feedback_store_load_error: None,
            trace_store: JudgeTraceStore::default(),
        }
    }

    pub fn with_process_runner(mut self, runner: Arc<dyn ProcessRunner>) -> Self {
        let transports = self
            .config
            .modules
            .iter()
            .map(|module| {
                Arc::new(RunnerJudgeTransport {
                    request: InteractiveProcessRequest {
                        executable: PathBuf::from(&module.executable),
                        args: module.arguments.clone(),
                        env: module
                            .environment
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect(),
                        cwd: None,
                    },
                    runner: runner.clone(),
                }) as Arc<dyn JudgeTransport>
            })
            .collect();
        self.transports = Arc::new(transports);
        self
    }

    pub fn with_logger(mut self, logger: Option<Arc<dyn RuntimeLogger>>) -> Self {
        self.logger = logger;
        if let Some(error) = self.feedback_store_load_error.as_deref() {
            if let Some(logger) = &self.logger {
                let _ = logger.write(
                    "WARN",
                    &format!("判断役の餌履歴を読み込めません。餌を停止します: error={error}"),
                );
            }
        }
        self
    }

    pub fn with_feedback_store(mut self, path: PathBuf) -> Self {
        let (ledger, migrated, mut load_error) = match load_feedback_ledger(&path) {
            Ok(loaded) => (loaded.ledger, loaded.migrated, None),
            Err(error) => (FeedbackLedger::default(), false, Some(error)),
        };
        self.feedback_ledger = Arc::new(Mutex::new(ledger));
        self.feedback_store = Some(path.clone());
        self.feedback_store_load_error = load_error;
        if migrated && self.feedback_store_load_error.is_none() {
            let ledger = self
                .feedback_ledger
                .lock()
                .map(|ledger| ledger.clone())
                .map_err(|_| "餌履歴の移行用ロックが壊れています".to_owned());
            if let Ok(ledger) = ledger {
                if let Err(error) = self.persist_feedback_ledger(&ledger) {
                    load_error = Some(error.to_string());
                    self.feedback_store_load_error = load_error;
                }
            } else {
                self.feedback_store_load_error = ledger.err();
            }
        }
        self
    }

    pub fn with_feedback_store_if_present(self, path: Option<PathBuf>) -> Self {
        match path {
            Some(path) => self.with_feedback_store(path),
            None => self,
        }
    }

    pub fn feedback_store_path(&self) -> Option<&Path> {
        self.feedback_store.as_deref()
    }

    pub fn config(&self) -> &JudgeConfig {
        &self.config
    }

    pub fn trace_store(&self) -> JudgeTraceStore {
        self.trace_store.clone()
    }

    pub(crate) fn with_trace_store(mut self, trace_store: JudgeTraceStore) -> Self {
        self.trace_store = trace_store;
        self
    }

    pub fn enabled(&self) -> bool {
        !self.transports.is_empty()
    }

    pub async fn shutdown(&self) {
        let gate = self.begin_shutdown();
        let _ = tokio::time::timeout(JUDGE_SHUTDOWN_TIMEOUT, gate.wait_until_open()).await;
    }

    pub(crate) fn begin_shutdown(&self) -> Arc<JudgeStartGate> {
        let gate = JudgeStartGate::closed();
        let transports = self.transports.clone();
        let gate_for_task = gate.clone();
        tokio::spawn(async move {
            for transport in transports.iter() {
                transport.reset().await;
            }
            join_all(
                transports
                    .iter()
                    .map(|transport| async { transport.wait_for_shutdown().await }),
            )
            .await;
            gate_for_task.release();
        });
        gate
    }

    fn remember_trace(
        &self,
        input_id: &str,
        request: &Value,
        responses: Vec<JudgeModuleTrace>,
        decision: Option<JudgeDecision>,
    ) {
        self.trace_store.insert(JudgeTrace {
            input_id: input_id.to_owned(),
            request: request.clone(),
            feedable: !responses.is_empty() && responses.iter().all(module_trace_feedable),
            responses,
            decision,
        });
    }

    fn unavailable_request(&self, input_id: &str, reason: &str) -> Value {
        json!({
            "v": JUDGE_PROTOCOL_VERSION,
            "op": "evaluate",
            "params": {
                "input_id": input_id,
                "feature_schema": JUDGE_FEATURE_SCHEMA,
                "unavailable_reason": reason,
            },
        })
    }

    pub async fn evaluate(
        &self,
        frames: &[ObservationFrameInput],
        audio: &[AudioObservation],
        cancellation: CancellationToken,
    ) -> JudgeEvaluation {
        let Some(input_id) = primary_input_id(frames, audio) else {
            return JudgeEvaluation::pass_through();
        };
        if self.transports.is_empty() {
            return JudgeEvaluation::pass_through();
        }
        if let Some(reason) = input_insufficiency(frames, audio) {
            let evaluation = self.input_hold(&input_id, reason);
            let request = self.unavailable_request(&input_id, reason);
            self.remember_trace(&input_id, &request, Vec::new(), evaluation.decision.clone());
            return evaluation;
        }
        let request_id = Uuid::new_v4().to_string();
        let request = match self.request_bytes(&request_id, &input_id, frames, audio) {
            Ok(request) => request,
            Err(error) => {
                let evaluation = self.fallback(&input_id, error.to_string());
                let request = self.unavailable_request(&input_id, "request-build-failed");
                self.remember_trace(&input_id, &request, Vec::new(), evaluation.decision.clone());
                return evaluation;
            }
        };
        let request_value = match serde_json::from_slice::<Value>(&request) {
            Ok(value) => value,
            Err(error) => {
                let evaluation = self.fallback(&input_id, error.to_string());
                self.remember_trace(
                    &input_id,
                    &json!({
                        "v": JUDGE_PROTOCOL_VERSION,
                        "op": "evaluate",
                        "params": {
                            "input_id": input_id,
                            "feature_schema": JUDGE_FEATURE_SCHEMA,
                            "request_parse_error": error.to_string(),
                        },
                    }),
                    Vec::new(),
                    evaluation.decision.clone(),
                );
                return evaluation;
            }
        };
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let deadline = tokio::time::Instant::now() + timeout;
        let responses = join_all(
            self.transports
                .iter()
                .enumerate()
                .map(|(index, transport)| {
                    let transport = transport.clone();
                    let request = request.clone();
                    let cancellation = cancellation.clone();
                    async move {
                        let result = request_with_deadline(
                            &transport,
                            request,
                            deadline,
                            cancellation,
                            "評価",
                        )
                        .await;
                        (index, transport, result)
                    }
                }),
        )
        .await;
        let all_transports = responses
            .iter()
            .map(|(_, transport, _)| transport.clone())
            .collect::<Vec<_>>();
        if cancellation.is_cancelled()
            && responses
                .iter()
                .all(|(_, _, result)| matches!(result, Err(JudgeError::Cancelled)))
        {
            for (_, transport, _) in &responses {
                transport.reset().await;
            }
            self.remember_trace(&input_id, &request_value, Vec::new(), None);
            return JudgeEvaluation::pass_through();
        }
        let mut evaluations = Vec::new();
        let mut module_errors = Vec::new();
        let mut traces = Vec::with_capacity(responses.len());
        for (index, transport, result) in responses {
            let (response, output) = match result {
                Ok(stdout) => {
                    let response = first_json_value(&stdout);
                    let output = self.parse_evaluation(&stdout, &request_id, &input_id);
                    (response, output)
                }
                Err(error) => (None, Err(error)),
            };
            match output {
                Ok(output) => {
                    let mut trace = JudgeModuleTrace {
                        module_index: index,
                        response,
                        evaluation: Some(output.trace()),
                        error: None,
                    };
                    if let Some(update_token) = output.update_token.clone() {
                        if let Err(error) = self
                            .commit_update(
                                &transport,
                                &update_token,
                                deadline,
                                cancellation.clone(),
                            )
                            .await
                        {
                            if error.should_reset_transport() {
                                transport.reset().await;
                            }
                            module_errors.push(error.to_string());
                            self.log_module_error(index, &error);
                            trace.error = Some(error.to_string());
                        }
                    }
                    traces.push(trace);
                    evaluations.push((index, output));
                }
                Err(JudgeError::Cancelled) if cancellation.is_cancelled() => {
                    for transport in &all_transports {
                        transport.reset().await;
                    }
                    self.remember_trace(&input_id, &request_value, traces, None);
                    return JudgeEvaluation::pass_through();
                }
                Err(error) => {
                    if error.should_reset_transport() {
                        transport.reset().await;
                    }
                    module_errors.push(error.to_string());
                    self.log_module_error(index, &error);
                    traces.push(JudgeModuleTrace {
                        module_index: index,
                        response,
                        evaluation: None,
                        error: Some(error.to_string()),
                    });
                }
            }
        }
        if evaluations.is_empty() || !module_errors.is_empty() {
            let reason = if module_errors.is_empty() {
                "すべての判断役モジュールが利用できません".to_owned()
            } else {
                format!(
                    "判断役モジュールの一部または全部が利用できません。素通しに戻します: {}",
                    module_errors.join("; ")
                )
            };
            self.remember_trace(&input_id, &request_value, traces, None);
            return self.fallback(&input_id, reason);
        }
        self.log_feed_metadata(&input_id, &evaluations);
        let evaluation = match self.compose(&input_id, evaluations, module_errors.is_empty()) {
            Ok(mut decision) => {
                decision.sequence = self.next_sequence();
                decision.occurred_at = now_string();
                self.log_decision(&decision, "INFO");
                let companion_delivery = if self.config.follow {
                    decision.action.allows_companion()
                } else {
                    true
                };
                JudgeEvaluation {
                    decision: Some(decision),
                    companion_delivery,
                }
            }
            Err(error) => self.fallback(&input_id, error),
        };
        self.remember_trace(
            &input_id,
            &request_value,
            traces,
            evaluation.decision.clone(),
        );
        evaluation
    }

    async fn commit_update(
        &self,
        transport: &Arc<dyn JudgeTransport>,
        update_token: &str,
        deadline: tokio::time::Instant,
        cancellation: CancellationToken,
    ) -> Result<(), JudgeError> {
        let request_id = Uuid::new_v4().to_string();
        let request = self.request_bytes_for_operation(
            &request_id,
            "commit",
            json!({"update_token": update_token, "weight": 1.0}),
        )?;
        let stdout =
            request_with_deadline(transport, request, deadline, cancellation, "学習更新の確定")
                .await?;
        let response = self.parse_envelope(&stdout, &request_id)?;
        if !response.ok {
            return Err(response.plugin_error("commit が拒否されました")?);
        }
        if response
            .result
            .as_ref()
            .and_then(|result| result.get("committed"))
            .and_then(Value::as_bool)
            != Some(true)
        {
            return Err(JudgeError::Protocol(
                "commit の committed がありません".to_owned(),
            ));
        }
        Ok(())
    }

    pub async fn feed(
        &self,
        input_id: &str,
        sign: JudgeFeedSign,
        strength: f64,
        source: JudgeFeedSource,
        cancellation: CancellationToken,
    ) -> Result<(), JudgeError> {
        let event_id = format!("coosenpai:explicit:{}", Uuid::new_v4());
        self.feed_event(
            JudgeFeedEvent {
                event_id: &event_id,
                input_id,
                event_time: None,
                sign,
                strength,
                source,
                cancelled: false,
            },
            cancellation,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn feed_event_with_id(
        &self,
        event_id: String,
        input_id: String,
        sign: JudgeFeedSign,
        strength: f64,
        source: JudgeFeedSource,
        cancelled: bool,
        cancellation: CancellationToken,
    ) -> Result<(), JudgeError> {
        self.feed_event_with_result(
            JudgeFeedEvent {
                event_id: &event_id,
                input_id: &input_id,
                event_time: None,
                sign,
                strength,
                source,
                cancelled,
            },
            cancellation,
        )
        .await
        .map(|_| ())
    }

    pub(crate) async fn feed_event(
        &self,
        event: JudgeFeedEvent<'_>,
        cancellation: CancellationToken,
    ) -> Result<(), JudgeError> {
        self.feed_event_with_result(event, cancellation)
            .await
            .map(|_| ())
    }

    pub(crate) async fn feed_event_with_result(
        &self,
        event: JudgeFeedEvent<'_>,
        cancellation: CancellationToken,
    ) -> Result<JudgeFeedApplyResult, JudgeError> {
        if event.input_id.trim().is_empty() {
            return Err(JudgeError::Input("input_id が空です".to_owned()));
        }
        if event.event_id.trim().is_empty() {
            return Err(JudgeError::Input("event_id が空です".to_owned()));
        }
        if !event.strength.is_finite() || !(0.0..=1.0).contains(&event.strength) {
            return Err(JudgeError::Input(
                "strength は 0 以上 1 以下の有限値で指定してください".to_owned(),
            ));
        }
        if event
            .event_time
            .is_some_and(|value| timestamp_seconds(value).is_none())
        {
            return Err(JudgeError::Input(
                "event_time は RFC3339 形式で指定してください".to_owned(),
            ));
        }
        if self.transports.is_empty() {
            return Ok(JudgeFeedApplyResult::NotApplied);
        }
        let _send_guard = self.feedback_send_lock.lock().await;
        let JudgeFeedEvent {
            event_id,
            input_id,
            event_time,
            sign,
            strength,
            source,
            cancelled,
        } = event;
        let event_time = match event_time {
            Some(value) => value.to_owned(),
            None => self
                .feedback_ledger
                .lock()
                .map_err(|_| JudgeError::Input("feed revision のロックが壊れています".to_owned()))?
                .events
                .get(event_id)
                .and_then(|previous| previous.event_time.clone())
                .unwrap_or_else(now_string),
        };
        let event = JudgeFeedEvent {
            event_id,
            input_id,
            event_time: Some(&event_time),
            sign,
            strength,
            source,
            cancelled,
        };
        let reservation = self.reserve_feedback_revision(&event)?;
        self.send_feed_event(event, reservation, cancellation).await
    }

    fn reserve_feedback_revision(
        &self,
        event: &JudgeFeedEvent<'_>,
    ) -> Result<FeedbackReservation, JudgeError> {
        let mut ledger = self
            .feedback_ledger
            .lock()
            .map_err(|_| JudgeError::Input("feed revision のロックが壊れています".to_owned()))?;
        if !ledger.events.contains_key(event.event_id) && ledger.events.len() >= MAX_FEEDBACK_EVENTS
        {
            return Err(JudgeError::Input("餌履歴が上限に達しました".to_owned()));
        }
        if ledger.history.len() >= MAX_FEEDBACK_HISTORY {
            return Err(JudgeError::Input(
                "餌の発行履歴が上限に達しました".to_owned(),
            ));
        }
        if ledger
            .history
            .iter()
            .any(|record| record.event_id == event.event_id && record.uncertain)
        {
            return Err(JudgeError::Input(
                "永続化が不確実な餌は検証完了まで再送できません".to_owned(),
            ));
        }
        let revision = match ledger.events.get(event.event_id) {
            Some(previous) if previous.input_id != event.input_id => {
                return Err(JudgeError::Input(
                    "同じ event_id を別の input_id に付け替えられません".to_owned(),
                ));
            }
            Some(previous)
                if previous.event_time != event.event_time.map(str::to_owned)
                    || previous.source != event.source =>
            {
                return Err(JudgeError::Input(
                    "同じ event_id の時刻と source は変更できません".to_owned(),
                ));
            }
            Some(previous) if previous.matches(event) => previous.revision,
            Some(previous) => previous
                .revision
                .checked_add(1)
                .ok_or_else(|| JudgeError::Input("feed revision が上限に達しました".to_owned()))?,
            None => 1,
        };
        let attempt = ledger
            .history
            .iter()
            .filter(|record| record.event_id == event.event_id)
            .map(|record| record.attempt)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| JudgeError::Input("餌の発行試行番号が上限に達しました".to_owned()))?;
        let previous_events = ledger.events.clone();
        let previous_history_len = ledger.history.len();
        ledger.events.insert(
            event.event_id.to_owned(),
            FeedbackState::from_event(event, revision),
        );
        ledger.history.push(FeedbackDeliveryRecord {
            event_id: event.event_id.to_owned(),
            input_id: event.input_id.to_owned(),
            event_time: event.event_time.unwrap_or_default().to_owned(),
            sign: event.sign,
            strength: event.strength,
            source: event.source,
            cancelled: event.cancelled,
            revision,
            attempt,
            error_code: None,
            error_message: None,
            retryable: None,
            uncertain: false,
            status: FeedbackDeliveryStatus::Pending,
        });
        if let Err(error) = self.persist_feedback_ledger(&ledger) {
            ledger.events = previous_events;
            ledger.history.truncate(previous_history_len);
            return Err(error);
        }
        Ok(FeedbackReservation { revision, attempt })
    }

    fn persist_feedback_ledger(&self, ledger: &FeedbackLedger) -> Result<(), JudgeError> {
        if let Some(error) = self.feedback_store_load_error.as_deref() {
            return Err(JudgeError::Process(format!(
                "判断役の餌履歴を読み込めないため保存できません: {error}"
            )));
        }
        let Some(path) = self.feedback_store.as_deref() else {
            return Ok(());
        };
        atomic_write_json(
            path,
            &PersistedFeedbackLedger {
                schema_version: FEEDBACK_LEDGER_SCHEMA_VERSION,
                events: ledger.events.clone(),
                history: ledger.history.clone(),
            },
        )
        .map_err(|error| JudgeError::Process(format!("判断役の餌履歴を保存できません: {error}")))
    }

    async fn send_feed_event(
        &self,
        event: JudgeFeedEvent<'_>,
        reservation: FeedbackReservation,
        cancellation: CancellationToken,
    ) -> Result<JudgeFeedApplyResult, JudgeError> {
        let event_time_s = event
            .event_time
            .and_then(timestamp_seconds)
            .unwrap_or_else(|| Utc::now().timestamp_millis() as f64 / 1_000.0);
        let received_at_s = Utc::now().timestamp_millis() as f64 / 1_000.0;
        let params = json!({
            "event_id": event.event_id,
            "input_id": event.input_id,
            "sign": feed_sign_wire(event.sign),
            "strength": event.strength,
            "event_time_s": event_time_s,
            "received_at_s": received_at_s,
            "revision": reservation.revision,
            "cancelled": event.cancelled,
            "source": event.source,
        });
        let request_id = Uuid::new_v4().to_string();
        let request = match self.request_bytes_for_operation(&request_id, "feed", params) {
            Ok(request) => request,
            Err(error) => {
                self.record_feedback_delivery(
                    event.event_id,
                    reservation,
                    FeedbackDeliveryStatus::Failed {
                        reason: error.to_string(),
                    },
                    Some(&error),
                )?;
                return Err(error);
            }
        };
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let deadline = tokio::time::Instant::now() + timeout;
        let responses = join_all(
            self.transports
                .iter()
                .enumerate()
                .map(|(index, transport)| {
                    let transport = transport.clone();
                    let request = request.clone();
                    let cancellation = cancellation.clone();
                    async move {
                        let result = request_with_deadline(
                            &transport,
                            request,
                            deadline,
                            cancellation,
                            "餌",
                        )
                        .await;
                        (index, transport, result)
                    }
                }),
        )
        .await;
        let mut first_error = None;
        let mut uncertain_error = None;
        let mut applied_results = Vec::new();
        for (index, transport, result) in responses {
            let result = result.and_then(|stdout| self.parse_feed_response(&stdout, &request_id));
            match result {
                Ok(applied) => {
                    applied_results.push(applied);
                    self.log_feed(&event, reservation.revision, applied);
                }
                Err(error) => {
                    if error.should_reset_transport() {
                        transport.reset().await;
                    }
                    self.log_module_error(index, &error);
                    if error.is_persistence_uncertain() && uncertain_error.is_none() {
                        uncertain_error = Some(error);
                        continue;
                    }
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        let first_error = uncertain_error.or(first_error);
        let apply_result = if first_error.is_some() {
            None
        } else if applied_results.is_empty() {
            Some(JudgeFeedApplyResult::Unknown)
        } else if applied_results.iter().all(|applied| *applied) {
            Some(JudgeFeedApplyResult::Applied)
        } else {
            Some(JudgeFeedApplyResult::NotApplied)
        };
        let status = match &first_error {
            Some(error) => FeedbackDeliveryStatus::Failed {
                reason: error.to_string(),
            },
            None => FeedbackDeliveryStatus::Sent {
                applied: applied_results.iter().all(|applied| *applied),
            },
        };
        self.record_feedback_delivery(event.event_id, reservation, status, first_error.as_ref())?;
        match first_error {
            Some(error) => Err(error),
            None => Ok(apply_result.unwrap_or(JudgeFeedApplyResult::Unknown)),
        }
    }

    fn record_feedback_delivery(
        &self,
        event_id: &str,
        reservation: FeedbackReservation,
        status: FeedbackDeliveryStatus,
        error: Option<&JudgeError>,
    ) -> Result<(), JudgeError> {
        let mut ledger = self
            .feedback_ledger
            .lock()
            .map_err(|_| JudgeError::Input("feed 履歴のロックが壊れています".to_owned()))?;
        let record_index = ledger
            .history
            .iter()
            .rev()
            .position(|record| {
                record.event_id == event_id
                    && record.revision == reservation.revision
                    && record.attempt == reservation.attempt
            })
            .ok_or_else(|| JudgeError::Input("餌の発行履歴が見つかりません".to_owned()))?;
        let record_index = ledger.history.len() - 1 - record_index;
        let previous_status = ledger.history[record_index].status.clone();
        let previous_error_code = ledger.history[record_index].error_code.clone();
        let previous_error_message = ledger.history[record_index].error_message.clone();
        let previous_retryable = ledger.history[record_index].retryable;
        let previous_uncertain = ledger.history[record_index].uncertain;
        ledger.history[record_index].status = status;
        let (error_code, error_message, retryable, uncertain) = error
            .map(JudgeError::failure_metadata)
            .unwrap_or((None, None, None, false));
        ledger.history[record_index].error_code = error_code;
        ledger.history[record_index].error_message = error_message;
        ledger.history[record_index].retryable = retryable;
        ledger.history[record_index].uncertain = uncertain;
        if let Err(error) = self.persist_feedback_ledger(&ledger) {
            ledger.history[record_index].status = previous_status;
            ledger.history[record_index].error_code = previous_error_code;
            ledger.history[record_index].error_message = previous_error_message;
            ledger.history[record_index].retryable = previous_retryable;
            ledger.history[record_index].uncertain = previous_uncertain;
            return Err(error);
        }
        Ok(())
    }

    fn request_bytes(
        &self,
        request_id: &str,
        input_id: &str,
        frames: &[ObservationFrameInput],
        audio: &[AudioObservation],
    ) -> Result<Vec<u8>, JudgeError> {
        let primary = frames
            .first()
            .map(frame_param)
            .or_else(|| audio.first().map(audio_param))
            .unwrap_or(Value::Null);
        let at_ms = primary["at_ms"].as_u64().unwrap_or_default();
        let image_path = frames
            .first()
            .filter(|frame| !frame.image_path.as_os_str().is_empty())
            .map(|frame| frame.image_path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stream_id = if frames.is_empty() {
            "audio/main"
        } else {
            "screen/main"
        };
        self.request_bytes_for_operation(
            request_id,
            "evaluate",
            json!({
                "input_id": input_id,
                "image_path": image_path,
                "stream_id": stream_id,
                "at_ms": at_ms,
                "timestamp_s": at_ms as f64 / 1_000.0,
                "feature_schema": JUDGE_FEATURE_SCHEMA,
                "features": [],
                "position": 0,
                "shadow": !self.config.follow,
                "observation": primary,
                "context": {
                    "frames": frames.iter().map(frame_param).collect::<Vec<_>>(),
                    "audio": audio.iter().map(audio_param).collect::<Vec<_>>(),
                },
            }),
        )
    }

    fn request_bytes_for_operation(
        &self,
        request_id: &str,
        operation: &str,
        params: Value,
    ) -> Result<Vec<u8>, JudgeError> {
        let request = json!({
            "v": JUDGE_PROTOCOL_VERSION,
            "id": request_id,
            "session": &self.session,
            "op": operation,
            "params": params,
        });
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn parse_evaluation(
        &self,
        stdout: &[u8],
        request_id: &str,
        input_id: &str,
    ) -> Result<ModuleEvaluation, JudgeError> {
        let response = self.parse_envelope(stdout, request_id)?;
        if !response.ok {
            return Err(response.plugin_error("evaluate が拒否されました")?);
        }
        let result = response
            .result
            .ok_or_else(|| JudgeError::Protocol("evaluate の result がありません".to_owned()))?;
        let output: PluginEvaluation = serde_json::from_value(result)?;
        if output.input_id != input_id {
            return Err(JudgeError::Protocol(
                "evaluate の input_id が要求と一致しません".to_owned(),
            ));
        }
        let action = parse_action(&output.action)?;
        let readiness = normalize_readiness(&output.readiness, action);
        if output.readiness.trim().is_empty() {
            return Err(JudgeError::Protocol("readiness が空です".to_owned()));
        }
        if readiness == "ready" && (output.novelty.is_none() || output.relevance.is_none()) {
            return Err(JudgeError::Protocol(
                "ready の応答には novelty と relevance が必要です".to_owned(),
            ));
        }
        for score in [output.novelty, output.relevance].into_iter().flatten() {
            if !score.is_finite() || !(0.0..=1.0).contains(&score) {
                return Err(JudgeError::Protocol(
                    "novelty と relevance は 0 以上 1 以下の有限値で指定してください".to_owned(),
                ));
            }
        }
        let action = if readiness != "ready" || action == JudgeAction::Hold {
            JudgeAction::Hold
        } else {
            action
        };
        let readiness = if action == JudgeAction::Hold && readiness == "ready" {
            "composition-pending".to_owned()
        } else {
            readiness
        };
        let (novelty, relevance) = if readiness == "ready" {
            (output.novelty, output.relevance)
        } else {
            (None, None)
        };
        let hold_reason = output
            .hold_reason
            .filter(|reason| !reason.trim().is_empty())
            .or_else(|| (action == JudgeAction::Hold).then(|| readiness.clone()));
        Ok(ModuleEvaluation {
            novelty,
            relevance,
            action,
            readiness,
            hold_reason,
            update_token: output.update_token,
            feed_target: output.feed_target,
            feed_rejection_code: output.feed_rejection_code,
        })
    }

    fn parse_feed_response(&self, stdout: &[u8], request_id: &str) -> Result<bool, JudgeError> {
        let response = self.parse_envelope(stdout, request_id)?;
        if !response.ok {
            return Err(response.plugin_error("feed が拒否されました")?);
        }
        let result = response
            .result
            .ok_or_else(|| JudgeError::Protocol("feed の result がありません".to_owned()))?;
        result
            .get("applied")
            .and_then(Value::as_bool)
            .ok_or_else(|| JudgeError::Protocol("feed の applied がありません".to_owned()))
    }

    fn parse_envelope(
        &self,
        stdout: &[u8],
        request_id: &str,
    ) -> Result<PluginResponse, JudgeError> {
        let line = stdout
            .split(|byte| *byte == b'\n')
            .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
            .find(|line| !line.is_empty())
            .ok_or_else(|| JudgeError::Protocol("標準出力に応答がありません".to_owned()))?;
        let response: PluginResponse = serde_json::from_slice(line)?;
        if response.v != JUDGE_PROTOCOL_VERSION {
            return Err(JudgeError::Protocol(
                "応答の v が 1 ではありません".to_owned(),
            ));
        }
        if response.id != request_id {
            return Err(JudgeError::Protocol(
                "応答の id が要求と一致しません".to_owned(),
            ));
        }
        if response
            .session
            .as_deref()
            .is_some_and(|session| session != self.session)
        {
            return Err(JudgeError::Protocol(
                "応答の session が要求と一致しません".to_owned(),
            ));
        }
        Ok(response)
    }

    fn compose(
        &self,
        input_id: &str,
        evaluations: Vec<(usize, ModuleEvaluation)>,
        scores_available: bool,
    ) -> Result<JudgeDecision, String> {
        let mode = if self.config.follow {
            JudgeMode::Follow
        } else {
            JudgeMode::Shadow
        };
        if self.config.composition == JudgeComposition::Single {
            let (_, evaluation) = evaluations
                .into_iter()
                .next()
                .ok_or_else(|| "single の判断役応答がありません".to_owned())?;
            return Ok(JudgeDecision {
                sequence: 0,
                occurred_at: String::new(),
                input_id: input_id.to_owned(),
                novelty: evaluation.novelty,
                relevance: evaluation.relevance,
                action: evaluation.action,
                readiness: evaluation.readiness,
                mode,
                hold_reason: evaluation.hold_reason,
                fallback_reason: None,
            });
        }
        let eligible = evaluations
            .iter()
            .filter(|(_, evaluation)| {
                evaluation.readiness == "ready" && evaluation.action != JudgeAction::Hold
            })
            .collect::<Vec<_>>();
        if eligible.is_empty() {
            let composition_pending = evaluations.iter().any(|(_, evaluation)| {
                evaluation.readiness == "composition-pending"
                    || (evaluation.readiness == "ready" && evaluation.action == JudgeAction::Hold)
            });
            let history_insufficient = evaluations
                .iter()
                .all(|(_, evaluation)| evaluation.readiness == "insufficient-history");
            let readiness = if composition_pending {
                "composition-pending"
            } else if history_insufficient {
                "insufficient-history"
            } else {
                "ensemble-pending"
            };
            return Ok(JudgeDecision {
                sequence: 0,
                occurred_at: String::new(),
                input_id: input_id.to_owned(),
                novelty: None,
                relevance: None,
                action: JudgeAction::Hold,
                readiness: readiness.to_owned(),
                mode,
                hold_reason: Some(readiness.to_owned()),
                fallback_reason: None,
            });
        }
        let veto_silence = self.config.veto
            && eligible
                .iter()
                .any(|(_, evaluation)| evaluation.action == JudgeAction::Silence);
        let (action, readiness) = if veto_silence {
            (JudgeAction::Silence, "ready".to_owned())
        } else {
            let mut votes = [(JudgeAction::Notify, 0.0), (JudgeAction::Silence, 0.0)];
            for (index, evaluation) in &eligible {
                let weight = if self.config.composition == JudgeComposition::Weighted {
                    self.config.modules[*index].weight
                } else {
                    1.0
                };
                if let Some((_, score)) = votes
                    .iter_mut()
                    .find(|(candidate, _)| *candidate == evaluation.action)
                {
                    *score += weight;
                }
            }
            if (votes[0].1 - votes[1].1).abs() < f64::EPSILON {
                return Ok(JudgeDecision {
                    sequence: 0,
                    occurred_at: String::new(),
                    input_id: input_id.to_owned(),
                    novelty: None,
                    relevance: None,
                    action: JudgeAction::Hold,
                    readiness: "composition-pending".to_owned(),
                    mode,
                    hold_reason: Some("composition-pending".to_owned()),
                    fallback_reason: None,
                });
            } else if votes[0].1 > votes[1].1 {
                (JudgeAction::Notify, "ready".to_owned())
            } else {
                (JudgeAction::Silence, "ready".to_owned())
            }
        };
        let weight_for = |index: usize| {
            if self.config.composition == JudgeComposition::Weighted {
                self.config.modules[index].weight
            } else {
                1.0
            }
        };
        let total_weight = eligible
            .iter()
            .map(|(index, _)| weight_for(*index))
            .sum::<f64>();
        let (novelty, relevance) = if scores_available {
            (
                Some(
                    eligible
                        .iter()
                        .filter_map(|(index, evaluation)| {
                            evaluation.novelty.map(|score| score * weight_for(*index))
                        })
                        .sum::<f64>()
                        / total_weight,
                ),
                Some(
                    eligible
                        .iter()
                        .filter_map(|(index, evaluation)| {
                            evaluation.relevance.map(|score| score * weight_for(*index))
                        })
                        .sum::<f64>()
                        / total_weight,
                ),
            )
        } else {
            (None, None)
        };
        Ok(JudgeDecision {
            sequence: 0,
            occurred_at: String::new(),
            input_id: input_id.to_owned(),
            novelty,
            relevance,
            action,
            readiness,
            mode,
            hold_reason: (action == JudgeAction::Hold).then_some("composition-pending".to_owned()),
            fallback_reason: None,
        })
    }

    fn input_hold(&self, input_id: &str, reason: &str) -> JudgeEvaluation {
        let decision = JudgeDecision {
            sequence: self.next_sequence(),
            occurred_at: now_string(),
            input_id: input_id.to_owned(),
            novelty: None,
            relevance: None,
            action: JudgeAction::Hold,
            readiness: "input-insufficient".to_owned(),
            mode: if self.config.follow {
                JudgeMode::Follow
            } else {
                JudgeMode::Shadow
            },
            hold_reason: Some(reason.to_owned()),
            fallback_reason: None,
        };
        self.log_decision(&decision, "WARN");
        JudgeEvaluation {
            decision: Some(decision),
            companion_delivery: !self.config.follow,
        }
    }

    fn fallback(&self, input_id: &str, reason: String) -> JudgeEvaluation {
        let decision = JudgeDecision {
            sequence: self.next_sequence(),
            occurred_at: now_string(),
            input_id: input_id.to_owned(),
            novelty: None,
            relevance: None,
            action: JudgeAction::Notify,
            readiness: "unavailable".to_owned(),
            mode: JudgeMode::PassThrough,
            hold_reason: None,
            fallback_reason: Some(reason),
        };
        self.log_decision(&decision, "WARN");
        JudgeEvaluation {
            decision: Some(decision),
            companion_delivery: true,
        }
    }

    fn next_sequence(&self) -> u64 {
        self.sequence
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
    }

    fn log_module_error(&self, index: usize, error: &JudgeError) {
        if let Some(logger) = &self.logger {
            let _ = logger.write(
                "WARN",
                &format!("判断役モジュールの処理に失敗しました: module={index} error={error}"),
            );
        }
    }

    fn log_feed_metadata(&self, input_id: &str, evaluations: &[(usize, ModuleEvaluation)]) {
        let Some(logger) = &self.logger else { return };
        for (index, evaluation) in evaluations {
            if evaluation.feed_target.is_none() && evaluation.feed_rejection_code.is_none() {
                continue;
            }
            let _ = logger.write(
                "INFO",
                &format!(
                    "判断役の餌対象: input-id={input_id} module={index} feed-target={:?} feed-rejection-code={:?}",
                    evaluation.feed_target, evaluation.feed_rejection_code
                ),
            );
        }
    }

    fn log_decision(&self, decision: &JudgeDecision, level: &str) {
        let Some(logger) = &self.logger else { return };
        let fallback = decision
            .fallback_reason
            .as_deref()
            .map_or(String::new(), |reason| format!(" fallback={reason}"));
        let hold_reason = decision
            .hold_reason
            .as_deref()
            .map_or(String::new(), |reason| format!(" hold-reason={reason}"));
        let _ = logger.write(
            level,
            &format!(
                "判断役の出力: input-id={} novelty={} relevance={} action={} readiness={} mode={:?}{}{}",
                decision.input_id,
                format_score(decision.novelty),
                format_score(decision.relevance),
                serde_json::to_string(&decision.action).unwrap_or_else(|_| "unknown".to_owned()),
                decision.readiness,
                decision.mode,
                hold_reason,
                fallback,
            ),
        );
    }

    fn log_feed(&self, event: &JudgeFeedEvent<'_>, revision: u64, applied: bool) {
        if let Some(logger) = &self.logger {
            let _ = logger.write(
                "INFO",
                &format!(
                    "判断役へ餌を渡しました: event-id={} input-id={} sign={:?} strength={:.3} source={:?} revision={revision} applied={applied}",
                    event.event_id,
                    event.input_id,
                    event.sign,
                    event.strength,
                    event.source,
                ),
            );
        }
    }
}

#[derive(Debug, Deserialize)]
struct PluginResponse {
    v: u8,
    id: String,
    #[serde(default)]
    session: Option<String>,
    ok: bool,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<PluginErrorEnvelope>,
}

#[derive(Debug, Deserialize)]
struct PluginErrorEnvelope {
    code: String,
    message: String,
}

impl PluginResponse {
    fn plugin_error(self, fallback: &str) -> Result<JudgeError, JudgeError> {
        let error = self
            .error
            .ok_or_else(|| JudgeError::Protocol(fallback.to_owned()))?;
        if error.code.trim().is_empty() || error.message.trim().is_empty() {
            return Err(JudgeError::Protocol(format!(
                "{fallback} の error.code または error.message が空です"
            )));
        }
        Ok(JudgeError::PluginRejected {
            code: error.code,
            message: error.message,
        })
    }
}

#[derive(Debug, Deserialize)]
struct PluginEvaluation {
    input_id: String,
    #[serde(default)]
    novelty: Option<f64>,
    #[serde(default)]
    relevance: Option<f64>,
    action: String,
    readiness: String,
    #[serde(default)]
    hold_reason: Option<String>,
    #[serde(default)]
    update_token: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    feedable: Option<bool>,
    #[serde(default)]
    feed_target: Option<String>,
    #[serde(default)]
    feed_rejection_code: Option<String>,
}

#[derive(Debug, Clone)]
struct ModuleEvaluation {
    novelty: Option<f64>,
    relevance: Option<f64>,
    action: JudgeAction,
    readiness: String,
    hold_reason: Option<String>,
    update_token: Option<String>,
    feed_target: Option<String>,
    feed_rejection_code: Option<String>,
}

impl ModuleEvaluation {
    fn trace(&self) -> JudgeModuleEvaluation {
        JudgeModuleEvaluation {
            novelty: self.novelty,
            relevance: self.relevance,
            action: self.action,
            readiness: self.readiness.clone(),
        }
    }
}

fn module_trace_feedable(trace: &JudgeModuleTrace) -> bool {
    trace.error.is_none()
        && trace
            .response
            .as_ref()
            .and_then(|response| response.get("result"))
            .and_then(|result| result.get("feedable"))
            .and_then(Value::as_bool)
            == Some(true)
}

fn first_json_value(stdout: &[u8]) -> Option<Value> {
    stdout
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .find(|line| !line.is_empty())
        .and_then(|line| serde_json::from_slice(line).ok())
}

fn parse_action(value: &str) -> Result<JudgeAction, JudgeError> {
    match value {
        "notify" => Ok(JudgeAction::Notify),
        "silence" => Ok(JudgeAction::Silence),
        "hold" => Ok(JudgeAction::Hold),
        _ => Err(JudgeError::Protocol(format!("action が不正です: {value}"))),
    }
}

fn normalize_readiness(value: &str, action: JudgeAction) -> String {
    match value {
        "evaluated" => "ready".to_owned(),
        "throttled" | "baseline_insufficient" | "insufficient-history" | "history-insufficient" => {
            "insufficient-history".to_owned()
        }
        "unsupported-input" | "input-insufficient" => "input-insufficient".to_owned(),
        "pending" | "ensemble-pending" => "ensemble-pending".to_owned(),
        "composition-pending" => "composition-pending".to_owned(),
        value if !value.trim().is_empty() => value.to_owned(),
        _ if action == JudgeAction::Hold => "composition-pending".to_owned(),
        _ => String::new(),
    }
}

fn input_insufficiency(
    frames: &[ObservationFrameInput],
    audio: &[AudioObservation],
) -> Option<&'static str> {
    if let Some(frame) = frames.first() {
        let app = frame.app.as_ref().or(frame.front_app.as_ref());
        if app.is_none_or(|value| value.trim().is_empty()) {
            return Some("visual-app-missing");
        }
        if frame.target.trim().is_empty() {
            return Some("visual-target-missing");
        }
    } else if let Some(record) = audio.first() {
        if record.text.trim().is_empty() {
            return Some("audio-text-missing");
        }
    }
    None
}

fn frame_param(frame: &ObservationFrameInput) -> Value {
    let at_ms = frame.captured_at.timestamp_millis().max(0) as u64;
    let mut value = json!({
        "frame_id": &frame.context_id,
        "at_ms": at_ms,
        "app": frame.app.as_ref().or(frame.front_app.as_ref()),
        "kind": "visual",
        "target": &frame.target,
    });
    if !frame.image_path.as_os_str().is_empty() {
        value["image_path"] = json!(frame.image_path.to_string_lossy());
    }
    if let Some(ocr_text) = &frame.ocr_text {
        value["ocr_text"] = json!(ocr_text);
    }
    value
}

fn audio_param(audio: &AudioObservation) -> Value {
    json!({
        "frame_id": &audio.id,
        "at_ms": timestamp_ms(&audio.created_at).unwrap_or_default(),
        "app": Value::Null,
        "kind": "audio",
        "source": audio.source,
        "text": &audio.text,
    })
}

fn primary_input_id(
    frames: &[ObservationFrameInput],
    audio: &[AudioObservation],
) -> Option<String> {
    frames
        .first()
        .map(|frame| frame.context_id.clone())
        .or_else(|| audio.first().map(|record| record.id.clone()))
}

fn timestamp_ms(value: &str) -> Option<u64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.timestamp_millis().max(0) as u64)
}

fn timestamp_seconds(value: &str) -> Option<f64> {
    timestamp_ms(value).map(|value| value as f64 / 1_000.0)
}

fn feed_sign_wire(sign: JudgeFeedSign) -> &'static str {
    match sign {
        JudgeFeedSign::Positive => "reward",
        JudgeFeedSign::Negative => "punish",
    }
}

fn now_string() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn equal_score(left: Option<f64>, right: Option<f64>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.to_bits() == right.to_bits(),
        (None, None) => true,
        _ => false,
    }
}

fn format_score(score: Option<f64>) -> String {
    score.map_or_else(|| "null".to_owned(), |score| format!("{score:.3}"))
}

struct LoadedFeedbackLedger {
    ledger: FeedbackLedger,
    migrated: bool,
}

fn load_feedback_ledger(path: &Path) -> Result<LoadedFeedbackLedger, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LoadedFeedbackLedger {
                ledger: FeedbackLedger::default(),
                migrated: false,
            });
        }
        Err(error) => return Err(format!("{error}")),
    };
    let ledger: PersistedFeedbackLedger =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if ledger.schema_version != 1 && ledger.schema_version != FEEDBACK_LEDGER_SCHEMA_VERSION {
        return Err(format!(
            "対応していない schema_version です: {}",
            ledger.schema_version
        ));
    }
    if ledger.events.len() > MAX_FEEDBACK_EVENTS {
        return Err(format!(
            "餌履歴が上限を超えています: {}",
            ledger.events.len()
        ));
    }
    for (event_id, event) in &ledger.events {
        if event_id.trim().is_empty()
            || event.input_id.trim().is_empty()
            || event.revision == 0
            || !event.strength.is_finite()
            || !(0.0..=1.0).contains(&event.strength)
            || event
                .event_time
                .as_deref()
                .is_none_or(|value| timestamp_seconds(value).is_none())
        {
            return Err(
                "餌履歴の event_id、input_id、event_time、revision または strength が不正です"
                    .to_owned(),
            );
        }
    }
    if ledger.history.len() > MAX_FEEDBACK_HISTORY {
        return Err(format!(
            "餌の発行履歴が上限を超えています: {}",
            ledger.history.len()
        ));
    }
    for record in &ledger.history {
        if record.event_id.trim().is_empty()
            || record.input_id.trim().is_empty()
            || record.event_time.trim().is_empty()
            || timestamp_seconds(&record.event_time).is_none()
            || record.revision == 0
            || record.attempt == 0
            || !record.strength.is_finite()
            || !(0.0..=1.0).contains(&record.strength)
            || matches!(
                &record.status,
                FeedbackDeliveryStatus::Failed { reason } if reason.trim().is_empty()
            )
        {
            return Err("餌の発行履歴の event_id、input_id、event_time、revision、attempt、strength または status が不正です".to_owned());
        }
    }
    Ok(LoadedFeedbackLedger {
        ledger: FeedbackLedger {
            events: ledger.events,
            history: ledger.history,
        },
        migrated: ledger.schema_version == 1,
    })
}
