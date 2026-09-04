use crate::onboarding_notice::TutorialNoticeState;
use crate::persistence::{atomic_write_json, PersistenceError, SiblingLock};
use crate::provider::{
    ProviderCallOptions, ProviderCapabilities, ProviderClient, ProviderError, ProviderErrorKind,
    ProviderName, ProviderResult, ProviderSession,
};
use crate::timing::tutorial_response_delay;
use async_trait::async_trait;
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

pub const TUTORIAL_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TutorialStep {
    Chat,
    Text,
    Image,
    Voice,
    Persona,
    Watch,
}

impl TutorialStep {
    pub const ALL: [Self; 6] = [
        Self::Chat,
        Self::Text,
        Self::Image,
        Self::Voice,
        Self::Persona,
        Self::Watch,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Text => "text",
            Self::Image => "image",
            Self::Voice => "voice",
            Self::Persona => "persona",
            Self::Watch => "watch",
        }
    }

    pub fn response_key(self) -> Option<&'static str> {
        match self {
            Self::Chat => Some("after-chat"),
            Self::Text => Some("after-text"),
            Self::Image => Some("after-image"),
            Self::Voice => Some("after-voice"),
            Self::Persona | Self::Watch => None,
        }
    }

    fn introduced_in_version(self) -> u8 {
        match self {
            Self::Chat | Self::Text | Self::Image | Self::Voice | Self::Persona => 0,
            Self::Watch => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TutorialStepStatus {
    Done,
    Skipped,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TutorialStepState {
    pub status: TutorialStepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetupState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TutorialState {
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_requested_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    pub steps: BTreeMap<String, TutorialStepState>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub notices: BTreeMap<String, TutorialNoticeState>,
}

impl Default for TutorialState {
    fn default() -> Self {
        Self {
            version: TUTORIAL_VERSION,
            run_id: None,
            started_at: None,
            finish_requested_at: None,
            completed_at: None,
            steps: default_steps(),
            notices: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OnboardingState {
    pub schema_version: u8,
    pub setup: SetupState,
    pub tutorial: TutorialState,
}

impl Default for OnboardingState {
    fn default() -> Self {
        Self {
            schema_version: 1,
            setup: SetupState::default(),
            tutorial: TutorialState::default(),
        }
    }
}

impl OnboardingState {
    pub fn needs_setup(&self) -> bool {
        self.setup.completed_at.is_none()
    }

    pub fn tutorial_active(&self) -> bool {
        self.tutorial.started_at.is_some() && self.tutorial.completed_at.is_none()
    }

    pub fn current_step(&self) -> Option<TutorialStep> {
        TutorialStep::ALL.into_iter().find(|step| {
            self.tutorial
                .steps
                .get(step.id())
                .is_none_or(|state| state.status == TutorialStepStatus::Pending)
        })
    }

    pub fn tutorial_finish_pending(&self) -> bool {
        self.tutorial_active() && self.tutorial.finish_requested_at.is_some()
    }

    pub fn complete_setup(&mut self, now: &str) {
        self.setup.completed_at = Some(now.to_owned());
    }

    pub fn start_tutorial(&mut self, now: &str) {
        self.tutorial.version = TUTORIAL_VERSION;
        self.tutorial.run_id = Some(uuid::Uuid::new_v4().to_string());
        self.tutorial.started_at = Some(now.to_owned());
        self.tutorial.finish_requested_at = None;
        self.tutorial.completed_at = None;
        self.tutorial.notices.clear();
        for step in TutorialStep::ALL {
            self.tutorial
                .steps
                .entry(step.id().to_owned())
                .or_insert(TutorialStepState {
                    status: TutorialStepStatus::Pending,
                    at: None,
                });
        }
    }

    pub fn finish_step(&mut self, step: TutorialStep, skipped: bool, now: &str) {
        self.tutorial.steps.insert(
            step.id().to_owned(),
            TutorialStepState {
                status: if skipped {
                    TutorialStepStatus::Skipped
                } else {
                    TutorialStepStatus::Done
                },
                at: Some(now.to_owned()),
            },
        );
    }

    pub fn finish_tutorial(&mut self, now: &str) {
        self.tutorial.finish_requested_at = None;
        self.tutorial.completed_at = Some(now.to_owned());
    }

    pub fn request_tutorial_finish(&mut self, now: &str) {
        if self.tutorial_active() {
            self.tutorial.finish_requested_at = Some(now.to_owned());
        }
    }
}

#[derive(Debug, Clone)]
pub struct OnboardingStore {
    path: PathBuf,
}

impl OnboardingStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<OnboardingState, OnboardingError> {
        let lock = sibling_lock(&self.path);
        let _guard = SiblingLock::acquire(&lock)?;
        load_locked(&self.path)
    }

    pub fn update<R>(
        &self,
        update: impl FnOnce(&mut OnboardingState) -> R,
    ) -> Result<R, OnboardingError> {
        let lock = sibling_lock(&self.path);
        let _guard = SiblingLock::acquire(&lock)?;
        let mut state = load_locked(&self.path)?;
        let result = update(&mut state);
        atomic_write_json(&self.path, &state)?;
        Ok(result)
    }
}

fn load_locked(path: &Path) -> Result<OnboardingState, OnboardingError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OnboardingState::default())
        }
        Err(error) => return Err(error.into()),
    };
    let mut state: OnboardingState = serde_json::from_slice(&bytes)?;
    let changed = validate_and_evolve_tutorial(&mut state)?;
    if changed {
        atomic_write_json(path, &state)?;
    }
    Ok(state)
}

fn validate_and_evolve_tutorial(state: &mut OnboardingState) -> Result<bool, OnboardingError> {
    let expected_steps = TutorialStep::ALL
        .into_iter()
        .map(|step| step.id())
        .collect::<std::collections::BTreeSet<_>>();
    let actual_steps = state
        .tutorial
        .steps
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    if state.schema_version != 1
        || state.tutorial.version > TUTORIAL_VERSION
        || !actual_steps.is_subset(&expected_steps)
        || (state.tutorial.started_at.is_some() && state.tutorial.run_id.is_none())
        || TutorialStep::ALL.into_iter().any(|step| {
            step.introduced_in_version() <= state.tutorial.version
                && !actual_steps.contains(step.id())
        })
    {
        return Err(OnboardingError::Invalid(
            "onboarding.json が現行の canonical 形式ではありません".to_owned(),
        ));
    }
    if state.tutorial.version == TUTORIAL_VERSION {
        return Ok(false);
    }
    let previous_version = state.tutorial.version;
    let mut added = false;
    for step in TutorialStep::ALL {
        if step.introduced_in_version() > previous_version
            && !state.tutorial.steps.contains_key(step.id())
        {
            state.tutorial.steps.insert(
                step.id().to_owned(),
                TutorialStepState {
                    status: TutorialStepStatus::Pending,
                    at: None,
                },
            );
            added = true;
        }
    }
    state.tutorial.version = TUTORIAL_VERSION;
    if added {
        state.tutorial.completed_at = None;
    }
    Ok(true)
}

fn sibling_lock(path: &Path) -> PathBuf {
    path.with_file_name(".onboarding.lock")
}

fn default_steps() -> BTreeMap<String, TutorialStepState> {
    TutorialStep::ALL
        .into_iter()
        .map(|step| {
            (
                step.id().to_owned(),
                TutorialStepState {
                    status: TutorialStepStatus::Pending,
                    at: None,
                },
            )
        })
        .collect()
}

pub fn timestamp_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TutorialPlaceholders {
    pub display_name: String,
    pub send_text: String,
    pub capture_region: String,
    pub microphone: String,
    pub toggle_watch: String,
}

#[derive(Debug, Clone)]
pub struct TutorialScript {
    sections: BTreeMap<String, String>,
}

impl TutorialScript {
    pub fn parse(document: &str) -> Result<Self, OnboardingError> {
        let mut sections = BTreeMap::new();
        let mut current: Option<(String, usize)> = None;
        let mut offset = 0;
        for line in document.split_inclusive('\n') {
            let heading = line
                .strip_suffix('\n')
                .unwrap_or(line)
                .strip_suffix('\r')
                .unwrap_or_else(|| line.strip_suffix('\n').unwrap_or(line));
            if let Some(next) = heading.strip_prefix("## ") {
                if let Some((previous, body_start)) = current.take() {
                    insert_section(&mut sections, previous, &document[body_start..offset])?;
                }
                if next.is_empty() {
                    return Err(OnboardingError::Invalid("空の見出しがあります".to_owned()));
                }
                current = Some((next.to_owned(), offset + line.len()));
            }
            offset += line.len();
        }
        if let Some((previous, body_start)) = current {
            insert_section(&mut sections, previous, &document[body_start..])?;
        }
        for required in [
            "setup-intro",
            "setup-connecting",
            "setup-ok",
            "setup-fail",
            "setup-none",
            "intro",
            "intro-click",
            "after-open",
            "after-chat",
            "text-intro",
            "after-text",
            "image-intro",
            "after-image",
            "voice-intro",
            "after-voice",
            "persona-intro",
            "after-persona",
            "watch-intro",
            "after-watch",
            "finish",
            "forced-finish",
            "later",
            "skip-hint",
        ] {
            if !sections.contains_key(required) {
                return Err(OnboardingError::MissingSection(required.to_owned()));
            }
        }
        Ok(Self { sections })
    }

    pub fn load(path: &Path) -> Result<Self, OnboardingError> {
        Self::parse(&fs::read_to_string(path)?)
    }

    pub fn render(
        &self,
        key: &str,
        values: &TutorialPlaceholders,
    ) -> Result<String, OnboardingError> {
        let text = self
            .sections
            .get(key)
            .ok_or_else(|| OnboardingError::MissingSection(key.to_owned()))?;
        Ok(text
            .replace("{displayName}", &values.display_name)
            .replace("{sendText}", &values.send_text)
            .replace("{captureRegion}", &values.capture_region)
            .replace("{microphone}", &values.microphone)
            .replace("{toggleWatch}", &values.toggle_watch))
    }
}

fn insert_section(
    sections: &mut BTreeMap<String, String>,
    key: String,
    body: &str,
) -> Result<(), OnboardingError> {
    if body.is_empty() || sections.insert(key.clone(), body.to_owned()).is_some() {
        return Err(OnboardingError::Invalid(format!(
            "チュートリアルの見出しが不正です: {key}"
        )));
    }
    Ok(())
}

#[derive(Clone)]
pub struct TutorialProvider {
    script: Arc<TutorialScript>,
    placeholders: TutorialPlaceholders,
}
impl TutorialProvider {
    pub fn new(script: TutorialScript, placeholders: TutorialPlaceholders) -> Self {
        Self {
            script: Arc::new(script),
            placeholders,
        }
    }

    pub fn render(&self, key: &str) -> Result<String, ProviderError> {
        self.script
            .render(key, &self.placeholders)
            .map_err(provider_error)
    }
}
#[async_trait]
impl ProviderClient for TutorialProvider {
    fn capabilities(&self) -> Option<ProviderCapabilities> {
        Some(ProviderCapabilities {
            default_model: "tutorial".to_owned(),
            model_candidates: vec!["tutorial".to_owned()],
            image_input: true,
            native_structured_output: true,
            effective_structured_output: true,
            streaming: true,
            cancellation: true,
            session_resume: true,
            session_compact: false,
            effort: false,
            mid_turn_input: false,
        })
    }

    async fn call(
        &self,
        input: ProviderCallOptions,
        cancellation: CancellationToken,
    ) -> Result<ProviderResult, ProviderError> {
        let session_model = input.model.clone().filter(|model| model != "default");
        if cancellation.is_cancelled() {
            return Err(ProviderError {
                kind: ProviderErrorKind::Retryable,
                message: "チュートリアルを取り消しました".to_owned(),
            });
        }
        let observer_call = input.output_schema.as_ref().is_some_and(|schema| {
            schema
                .get("properties")
                .and_then(|value| value.get("activity"))
                .is_some()
        });
        let (message, value) = if observer_call {
            let value = json!({
                "activity": "チュートリアル中",
                "outline": "", "changes": [], "events": [],
                "guess": null, "confidence": null, "wakeCompanion": false
            });
            (String::new(), Some(value))
        } else {
            let key = input
                .tutorial_response_key
                .as_deref()
                .ok_or_else(|| ProviderError {
                    kind: ProviderErrorKind::InvalidOutput,
                    message: "チュートリアル入力に応答 key がありません".to_owned(),
                })?;
            let message = self
                .script
                .render(key, &self.placeholders)
                .map_err(provider_error)?;
            let value = input.output_schema.as_ref().map(|_| {
                json!({
                    "emit": true, "message": message, "messageKind": "chat",
                    "notificationPriority": "none", "factCandidates": [], "factUpdates": []
                })
            });
            (message, value)
        };
        if !observer_call {
            tokio::select! {
                () = tokio::time::sleep(tutorial_response_delay(&message)) => {}
                () = cancellation.cancelled() => return Err(ProviderError {
                    kind: ProviderErrorKind::Retryable,
                    message: "チュートリアルを取り消しました".to_owned(),
                }),
            }
        }
        Ok(ProviderResult {
            text: value.as_ref().map_or(message, Value::to_string),
            value,
            session: Some(ProviderSession {
                provider: ProviderName::Codex,
                model: session_model,
                id: "tutorial-session".to_owned(),
            }),
        })
    }
}
fn provider_error(error: OnboardingError) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::InvalidOutput,
        message: error.to_string(),
    }
}
#[derive(Debug, Error)]
pub enum OnboardingError {
    #[error("onboarding の永続化に失敗しました: {0}")]
    Persistence(#[from] PersistenceError),
    #[error("onboarding のファイル操作に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("onboarding の JSON が不正です: {0}")]
    Json(#[from] serde_json::Error),
    #[error("チュートリアルに見出しがありません: {0}")]
    MissingSection(String),
    #[error("{0}")]
    Invalid(String),
}

