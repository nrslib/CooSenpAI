use crate::state::DesktopState;
use async_trait::async_trait;
use coosenpai_core::config::{Config, ConfigPaths};
use coosenpai_core::persistence::{atomic_write_json, SiblingLock};
use coosenpai_core::ports::RuntimeLogger;
use coosenpai_core::process::{ProcessRequest, ProcessRunner, TokioProcessRunner};
use coosenpai_core::provider::resolve_executable;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

pub(crate) const REMOTE_MODELS_URL: &str = "https://coosenp.ai/models.json";

const STATE_SCHEMA_VERSION: u8 = 1;
const REMOTE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const REMOTE_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REMOTE_BODY_BYTES: usize = 64 * 1024;
const OPENCODE_MODELS_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_CANDIDATES: usize = 256;
const MAX_HISTORY: usize = 50;
const CODEX_BUILTIN_CANDIDATES: &[&str] =
    &["default", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna"];
const CLAUDE_BUILTIN_CANDIDATES: &[&str] = &["default", "opus", "sonnet", "haiku"];
const BUILTIN_EFFORT_CANDIDATES: &[&str] = &["default", "low", "medium", "high", "xhigh"];

static REMOTE_REFRESH_LOCK: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelCatalogView {
    pub providers: Vec<ProviderModelCatalog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opencode_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderModelCatalog {
    pub provider: String,
    pub default_model: String,
    pub candidates: Vec<String>,
    pub history: Vec<String>,
    pub efforts: Vec<String>,
    pub model_efforts: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelCatalogState {
    schema_version: u8,
    #[serde(default)]
    last_remote_fetch_at: Option<u64>,
    #[serde(default = "default_remote_available")]
    remote_available: bool,
    #[serde(default)]
    codex: Option<Vec<String>>,
    #[serde(default)]
    claude: Option<Vec<String>>,
    #[serde(default)]
    opencode: Option<Vec<String>>,
    #[serde(default)]
    codex_efforts: Option<Vec<String>>,
    #[serde(default)]
    claude_efforts: Option<Vec<String>>,
    #[serde(default)]
    opencode_efforts: Option<Vec<String>>,
    #[serde(default)]
    codex_model_efforts: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    claude_model_efforts: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    opencode_model_efforts: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    history: ModelHistory,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelHistory {
    #[serde(default)]
    codex: Vec<String>,
    #[serde(default)]
    claude: Vec<String>,
    #[serde(default)]
    opencode: Vec<String>,
}

impl Default for ModelCatalogState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            last_remote_fetch_at: None,
            remote_available: true,
            codex: None,
            claude: None,
            opencode: None,
            codex_efforts: None,
            claude_efforts: None,
            opencode_efforts: None,
            codex_model_efforts: BTreeMap::new(),
            claude_model_efforts: BTreeMap::new(),
            opencode_model_efforts: BTreeMap::new(),
            history: ModelHistory::default(),
        }
    }
}

impl ModelCatalogState {
    fn validate(&self) -> Result<(), ModelCatalogError> {
        if self.schema_version != STATE_SCHEMA_VERSION {
            return Err(ModelCatalogError::InvalidState);
        }
        validate_optional_model_list(self.codex.as_deref())?;
        validate_optional_model_list(self.claude.as_deref())?;
        validate_optional_model_list(self.opencode.as_deref())?;
        validate_optional_model_list(self.codex_efforts.as_deref())?;
        validate_optional_model_list(self.claude_efforts.as_deref())?;
        validate_optional_model_list(self.opencode_efforts.as_deref())?;
        validate_model_efforts(&self.codex_model_efforts)?;
        validate_model_efforts(&self.claude_model_efforts)?;
        validate_model_efforts(&self.opencode_model_efforts)?;
        validate_value_list(&self.history.codex, false)?;
        validate_value_list(&self.history.claude, false)?;
        validate_value_list(&self.history.opencode, false)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteModelCatalog {
    codex: RemoteModelProvider,
    claude: RemoteModelProvider,
    opencode: RemoteEffortProvider,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteModelCatalogPayload {
    schema_version: u8,
    codex: RemoteModelProviderPayload,
    claude: RemoteModelProviderPayload,
    opencode: RemoteEffortProviderPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteModelProvider {
    models: Vec<String>,
    efforts: Vec<String>,
    model_efforts: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteEffortProvider {
    efforts: Vec<String>,
    model_efforts: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteModelProviderPayload {
    models: Vec<String>,
    efforts: Vec<String>,
    model_efforts: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoteEffortProviderPayload {
    efforts: Vec<String>,
    model_efforts: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelCatalogError {
    InvalidState,
    InvalidResponse,
    Network,
    Process,
    EmptyModels,
}

impl ModelCatalogError {
    fn reason(self) -> &'static str {
        match self {
            Self::InvalidState => "invalid-state",
            Self::InvalidResponse => "invalid-response",
            Self::Network => "network",
            Self::Process => "process",
            Self::EmptyModels => "empty-models",
        }
    }
}

struct ModelCatalogStore {
    path: PathBuf,
    lock_path: PathBuf,
}

impl ModelCatalogStore {
    fn new(paths: &ConfigPaths) -> Self {
        Self {
            path: paths.model_catalog.clone(),
            lock_path: paths.model_catalog.with_file_name(".model-catalog.lock"),
        }
    }

    fn load(&self) -> Result<ModelCatalogState, ModelCatalogError> {
        let _lock =
            SiblingLock::acquire(&self.lock_path).map_err(|_| ModelCatalogError::InvalidState)?;
        self.read_unlocked()
    }

    fn read_unlocked(&self) -> Result<ModelCatalogState, ModelCatalogError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ModelCatalogState::default())
            }
            Err(_) => return Err(ModelCatalogError::InvalidState),
        };
        let state = serde_json::from_slice::<ModelCatalogState>(&bytes)
            .map_err(|_| ModelCatalogError::InvalidState)?;
        state.validate()?;
        Ok(state)
    }

    fn update_remote(
        &self,
        remote: RemoteModelCatalog,
        fetched_at: u64,
    ) -> Result<(), ModelCatalogError> {
        let _lock =
            SiblingLock::acquire(&self.lock_path).map_err(|_| ModelCatalogError::InvalidState)?;
        let mut state = self.read_unlocked().unwrap_or_default();
        state.codex = Some(remote.codex.models);
        state.claude = Some(remote.claude.models);
        state.opencode_efforts = Some(remote.opencode.efforts);
        state.codex_efforts = Some(remote.codex.efforts);
        state.claude_efforts = Some(remote.claude.efforts);
        state.codex_model_efforts = remote.codex.model_efforts;
        state.claude_model_efforts = remote.claude.model_efforts;
        state.opencode_model_efforts = remote.opencode.model_efforts;
        state.last_remote_fetch_at = Some(fetched_at);
        state.remote_available = true;
        atomic_write_json(&self.path, &state).map_err(|_| ModelCatalogError::InvalidState)
    }

    fn mark_remote_failed(&self, attempted_at: u64) -> Result<(), ModelCatalogError> {
        let _lock =
            SiblingLock::acquire(&self.lock_path).map_err(|_| ModelCatalogError::InvalidState)?;
        let mut state = self.read_unlocked().unwrap_or_default();
        state.last_remote_fetch_at = Some(attempted_at);
        state.remote_available = false;
        atomic_write_json(&self.path, &state).map_err(|_| ModelCatalogError::InvalidState)
    }

    fn update_opencode(&self, models: Vec<String>) -> Result<(), ModelCatalogError> {
        let _lock =
            SiblingLock::acquire(&self.lock_path).map_err(|_| ModelCatalogError::InvalidState)?;
        let mut state = self.read_unlocked().unwrap_or_default();
        state.opencode = Some(models);
        atomic_write_json(&self.path, &state).map_err(|_| ModelCatalogError::InvalidState)
    }

    fn record_history(&self, provider: &str, model: &str) -> Result<(), ModelCatalogError> {
        let model = model.trim();
        if model.is_empty() {
            return Ok(());
        }
        let _lock =
            SiblingLock::acquire(&self.lock_path).map_err(|_| ModelCatalogError::InvalidState)?;
        let mut state = self.read_unlocked().unwrap_or_default();
        let history = match provider {
            "codex" => &mut state.history.codex,
            "claude" => &mut state.history.claude,
            "opencode" => &mut state.history.opencode,
            _ => return Ok(()),
        };
        history.retain(|value| value != model);
        history.push(model.to_owned());
        if history.len() > MAX_HISTORY {
            let excess = history.len() - MAX_HISTORY;
            history.drain(..excess);
        }
        atomic_write_json(&self.path, &state).map_err(|_| ModelCatalogError::InvalidState)
    }

}

async fn load_state_async(store: Arc<ModelCatalogStore>) -> ModelCatalogState {
    tokio::task::spawn_blocking(move || store.load())
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default()
}

async fn view_async(
    store: Arc<ModelCatalogStore>,
    config: Config,
    opencode_error: Option<String>,
) -> ModelCatalogView {
    let state = load_state_async(store).await;
    view_from_state(&state, &config, opencode_error.as_deref())
}

fn validate_optional_model_list(values: Option<&[String]>) -> Result<(), ModelCatalogError> {
    if let Some(values) = values {
        validate_value_list(values, true)?;
    }
    Ok(())
}

fn default_remote_available() -> bool {
    true
}

fn validate_value_list(
    values: &[String],
    require_non_empty: bool,
) -> Result<(), ModelCatalogError> {
    if require_non_empty && values.is_empty() {
        return Err(ModelCatalogError::EmptyModels);
    }
    if values.len() > MAX_CANDIDATES {
        return Err(ModelCatalogError::InvalidResponse);
    }
    if values.iter().any(|value| {
        let trimmed = value.trim();
        trimmed.is_empty() || trimmed.chars().any(char::is_control) || trimmed.len() > 256
    }) {
        return Err(ModelCatalogError::InvalidResponse);
    }
    Ok(())
}

fn normalize_value_list(values: Vec<String>) -> Result<Vec<String>, ModelCatalogError> {
    validate_value_list(&values, true)?;
    let mut normalized = Vec::with_capacity(values.len());
    for value in values {
        let value = value.trim().to_owned();
        if !normalized.iter().any(|current| current == &value) {
            normalized.push(value);
        }
    }
    if normalized.is_empty() {
        return Err(ModelCatalogError::EmptyModels);
    }
    Ok(normalized)
}

fn validate_model_efforts(
    model_efforts: &BTreeMap<String, Vec<String>>,
) -> Result<(), ModelCatalogError> {
    if model_efforts.len() > MAX_CANDIDATES {
        return Err(ModelCatalogError::InvalidResponse);
    }
    for (model, efforts) in model_efforts {
        if model.trim().is_empty() || model.chars().any(char::is_control) || model.len() > 256 {
            return Err(ModelCatalogError::InvalidResponse);
        }
        validate_value_list(efforts, true)?;
    }
    Ok(())
}

fn normalize_model_efforts(
    model_efforts: BTreeMap<String, Vec<String>>,
) -> Result<BTreeMap<String, Vec<String>>, ModelCatalogError> {
    validate_model_efforts(&model_efforts)?;
    let mut normalized = BTreeMap::new();
    for (model, efforts) in model_efforts {
        normalized.insert(model.trim().to_owned(), normalize_value_list(efforts)?);
    }
    Ok(normalized)
}

fn parse_remote_model_catalog(
    value: serde_json::Value,
) -> Result<RemoteModelCatalog, ModelCatalogError> {
    let payload = serde_json::from_value::<RemoteModelCatalogPayload>(value)
        .map_err(|_| ModelCatalogError::InvalidResponse)?;
    if payload.schema_version != STATE_SCHEMA_VERSION {
        return Err(ModelCatalogError::InvalidResponse);
    }
    Ok(RemoteModelCatalog {
        codex: RemoteModelProvider {
            models: normalize_value_list(payload.codex.models)?,
            efforts: normalize_value_list(payload.codex.efforts)?,
            model_efforts: normalize_model_efforts(payload.codex.model_efforts)?,
        },
        claude: RemoteModelProvider {
            models: normalize_value_list(payload.claude.models)?,
            efforts: normalize_value_list(payload.claude.efforts)?,
            model_efforts: normalize_model_efforts(payload.claude.model_efforts)?,
        },
        opencode: RemoteEffortProvider {
            efforts: normalize_value_list(payload.opencode.efforts)?,
            model_efforts: normalize_model_efforts(payload.opencode.model_efforts)?,
        },
    })
}

fn builtin_candidates(provider: &str) -> Vec<String> {
    match provider {
        "codex" => CODEX_BUILTIN_CANDIDATES
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        "claude" => CLAUDE_BUILTIN_CANDIDATES
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        _ => Vec::new(),
    }
}

fn builtin_efforts() -> Vec<String> {
    BUILTIN_EFFORT_CANDIDATES
        .iter()
        .map(|value| (*value).to_owned())
        .collect()
}

fn unique_values(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut unique = Vec::new();
    for value in values {
        let value = value.trim().to_owned();
        if !value.is_empty() && !unique.iter().any(|current| current == &value) {
            unique.push(value);
        }
    }
    unique
}

fn candidates_for(
    provider: &str,
    cached: Option<&[String]>,
    remote_available: bool,
) -> Vec<String> {
    remote_available
        .then_some(cached)
        .flatten()
        .map(|values| values.to_vec())
        .unwrap_or_else(|| builtin_candidates(provider))
}

fn efforts_for(cached: Option<&[String]>, remote_available: bool) -> Vec<String> {
    remote_available
        .then_some(cached)
        .flatten()
        .map(|values| values.to_vec())
        .unwrap_or_else(builtin_efforts)
}

fn model_efforts_for(
    model_efforts: &BTreeMap<String, Vec<String>>,
    remote_available: bool,
) -> BTreeMap<String, Vec<String>> {
    if remote_available {
        model_efforts.clone()
    } else {
        BTreeMap::new()
    }
}

fn history_for<'a>(history: &'a ModelHistory, provider: &str) -> &'a [String] {
    match provider {
        "codex" => &history.codex,
        "claude" => &history.claude,
        "opencode" => &history.opencode,
        _ => &[],
    }
}

fn provider_view(
    provider: &str,
    candidates: Vec<String>,
    history: &[String],
    efforts: Vec<String>,
    model_efforts: BTreeMap<String, Vec<String>>,
    config: &Config,
) -> ProviderModelCatalog {
    let candidates = unique_values(candidates);
    let history = unique_values(history.iter().cloned());
    let configured_model = config.companion.model.trim();
    let default_model = if config.companion.provider == provider && !configured_model.is_empty() {
        config.companion.model.clone()
    } else if provider == "claude" {
        "sonnet".to_owned()
    } else {
        candidates.first().cloned().unwrap_or_default()
    };
    ProviderModelCatalog {
        provider: provider.to_owned(),
        default_model,
        candidates,
        history,
        efforts: unique_values(efforts),
        model_efforts: model_efforts
            .into_iter()
            .map(|(model, efforts)| (model, unique_values(efforts)))
            .collect(),
    }
}

fn view_from_state(
    state: &ModelCatalogState,
    config: &Config,
    opencode_error: Option<&str>,
) -> ModelCatalogView {
    let codex_candidates = candidates_for("codex", state.codex.as_deref(), state.remote_available);
    let claude_candidates =
        candidates_for("claude", state.claude.as_deref(), state.remote_available);
    let codex_efforts = efforts_for(state.codex_efforts.as_deref(), state.remote_available);
    let claude_efforts = efforts_for(state.claude_efforts.as_deref(), state.remote_available);
    let opencode_efforts = efforts_for(state.opencode_efforts.as_deref(), state.remote_available);
    let opencode_candidates = match opencode_error {
        Some(_) => unique_values(
            std::iter::once(config.companion.model.clone())
                .chain(state.history.opencode.iter().cloned()),
        ),
        None => state.opencode.clone().unwrap_or_default(),
    };
    ModelCatalogView {
        providers: vec![
            provider_view(
                "codex",
                codex_candidates,
                history_for(&state.history, "codex"),
                codex_efforts,
                model_efforts_for(&state.codex_model_efforts, state.remote_available),
                config,
            ),
            provider_view(
                "claude",
                claude_candidates,
                history_for(&state.history, "claude"),
                claude_efforts,
                model_efforts_for(&state.claude_model_efforts, state.remote_available),
                config,
            ),
            provider_view(
                "opencode",
                opencode_candidates,
                history_for(&state.history, "opencode"),
                opencode_efforts,
                model_efforts_for(&state.opencode_model_efforts, state.remote_available),
                config,
            ),
        ],
        opencode_error: opencode_error.map(str::to_owned),
    }
}

#[async_trait]
trait RemoteModelsClient: Send + Sync {
    async fn fetch(&self) -> Result<RemoteModelCatalog, ModelCatalogError>;
}

struct HttpRemoteModelsClient {
    client: Client,
}

impl HttpRemoteModelsClient {
    fn new() -> Result<Self, reqwest::Error> {
        Client::builder()
            .timeout(REMOTE_REQUEST_TIMEOUT)
            .user_agent(concat!("CooSenpAI/", env!("CARGO_PKG_VERSION")))
            .build()
            .map(|client| Self { client })
    }
}

#[async_trait]
impl RemoteModelsClient for HttpRemoteModelsClient {
    async fn fetch(&self) -> Result<RemoteModelCatalog, ModelCatalogError> {
        let response = self
            .client
            .get(REMOTE_MODELS_URL)
            .send()
            .await
            .map_err(|_| ModelCatalogError::Network)?;
        if matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
        ) {
            return Err(ModelCatalogError::Network);
        }
        if !response.status().is_success() {
            return Err(ModelCatalogError::InvalidResponse);
        }
        let body = read_remote_body(response).await?;
        let value = serde_json::from_slice::<serde_json::Value>(&body)
            .map_err(|_| ModelCatalogError::InvalidResponse)?;
        parse_remote_model_catalog(value)
    }
}

async fn read_remote_body(mut response: reqwest::Response) -> Result<Vec<u8>, ModelCatalogError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_REMOTE_BODY_BYTES as u64)
    {
        return Err(ModelCatalogError::InvalidResponse);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ModelCatalogError::Network)?
    {
        append_remote_chunk(&mut body, &chunk)?;
    }
    Ok(body)
}

fn append_remote_chunk(body: &mut Vec<u8>, chunk: &[u8]) -> Result<(), ModelCatalogError> {
    if body.len().saturating_add(chunk.len()) > MAX_REMOTE_BODY_BYTES {
        return Err(ModelCatalogError::InvalidResponse);
    }
    body.extend_from_slice(chunk);
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteRefreshOutcome {
    Skipped,
    NotDue,
    Updated,
    Failed(ModelCatalogError),
}

fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn remote_refresh_is_due(last_fetch_at: Option<u64>, now: u64) -> bool {
    last_fetch_at
        .map(|last| now.saturating_sub(last) >= REMOTE_CHECK_INTERVAL.as_secs())
        .unwrap_or(true)
}

async fn refresh_remote_models(
    enabled: bool,
    force: bool,
    client: &dyn RemoteModelsClient,
    store: Arc<ModelCatalogStore>,
) -> RemoteRefreshOutcome {
    if !enabled {
        return RemoteRefreshOutcome::Skipped;
    }
    let _refresh_guard = REMOTE_REFRESH_LOCK
        .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
        .lock()
        .await;
    let state = load_state_async(store.clone()).await;
    if !force && !remote_refresh_is_due(state.last_remote_fetch_at, now_unix_seconds()) {
        return RemoteRefreshOutcome::NotDue;
    }
    let remote = match client.fetch().await {
        Ok(remote) => remote,
        Err(error) => {
            let store_for_failure = store.clone();
            let _ = tokio::task::spawn_blocking(move || {
                store_for_failure.mark_remote_failed(now_unix_seconds())
            })
            .await;
            return RemoteRefreshOutcome::Failed(error);
        }
    };
    let fetched_at = now_unix_seconds();
    let store_for_update = store;
    match tokio::task::spawn_blocking(move || store_for_update.update_remote(remote, fetched_at))
        .await
    {
        Ok(Ok(())) => RemoteRefreshOutcome::Updated,
        Ok(Err(error)) => RemoteRefreshOutcome::Failed(error),
        Err(_) => RemoteRefreshOutcome::Failed(ModelCatalogError::InvalidState),
    }
}

async fn load_opencode_models(
    config: &Config,
    cancellation: CancellationToken,
) -> Result<Vec<String>, ModelCatalogError> {
    load_opencode_models_with_runner(config, cancellation, &TokioProcessRunner).await
}

async fn load_opencode_models_with_runner(
    config: &Config,
    cancellation: CancellationToken,
    runner: &dyn ProcessRunner,
) -> Result<Vec<String>, ModelCatalogError> {
    let path_value =
        coosenpai_core::provider::resolve_login_shell_path(cancellation.child_token()).await;
    let executable_name = if config.companion.provider == "opencode" {
        config.companion.executable.as_deref().unwrap_or("opencode")
    } else {
        "opencode"
    };
    let executable =
        resolve_executable(executable_name, &path_value).map_err(|_| ModelCatalogError::Process)?;
    let output = runner
        .run(
            ProcessRequest {
                executable,
                args: vec!["models".to_owned()],
                env: vec![("PATH".to_owned(), path_value)],
                cwd: None,
                stdin: Vec::new(),
                timeout: OPENCODE_MODELS_TIMEOUT,
            },
            cancellation,
        )
        .await
        .map_err(|_| ModelCatalogError::Process)?;
    if output.status != Some(0) {
        return Err(ModelCatalogError::Process);
    }
    let stdout =
        std::str::from_utf8(&output.stdout).map_err(|_| ModelCatalogError::InvalidResponse)?;
    parse_opencode_models(stdout)
}

fn parse_opencode_models(output: &str) -> Result<Vec<String>, ModelCatalogError> {
    let models = output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty()
                || line.chars().any(char::is_whitespace)
                || line.chars().any(char::is_control)
            {
                return None;
            }
            let (provider, model) = line.split_once('/')?;
            if provider.is_empty() || model.is_empty() || provider.contains('/') {
                return None;
            }
            Some(line.to_owned())
        })
        .collect::<Vec<_>>();
    normalize_value_list(models)
}

fn log_remote_refresh(logger: &dyn RuntimeLogger, outcome: RemoteRefreshOutcome) {
    if let RemoteRefreshOutcome::Failed(error) = outcome {
        let _ = logger.write(
            "DEBUG",
            &format!(
                "モデル候補のリモート更新をスキップしました: reason={}",
                error.reason()
            ),
        );
    }
}

pub(crate) async fn catalog_for_state(state: &DesktopState) -> ModelCatalogView {
    let config = state.runtime_config();
    let store = Arc::new(ModelCatalogStore::new(&state.paths));
    if config.app.check_for_updates {
        match HttpRemoteModelsClient::new() {
            Ok(client) => {
                let outcome = refresh_remote_models(true, false, &client, store.clone()).await;
                log_remote_refresh(state.logger.as_ref(), outcome);
            }
            Err(_) => {
                let _ = state.logger.write(
                    "DEBUG",
                    "モデル候補のリモート更新をスキップしました: reason=client",
                );
            }
        }
    }

    let cached = load_state_async(store.clone()).await;
    if cached.opencode.is_none() {
        match load_opencode_models(&config, state.cancellation.child_token()).await {
            Ok(models) => {
                let store_for_update = store.clone();
                if tokio::task::spawn_blocking(move || store_for_update.update_opencode(models))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .is_none()
                {
                    let _ = state.logger.write(
                        "DEBUG",
                        "opencode のモデル一覧を保存できませんでした: reason=state",
                    );
                }
                view_async(store.clone(), config.clone(), None).await
            }
            Err(error) => {
                let _ = state.logger.write(
                    "DEBUG",
                    &format!(
                        "opencode のモデル一覧を取得できませんでした: reason={}",
                        error.reason()
                    ),
                );
                view_async(
                    store.clone(),
                    config.clone(),
                    Some("一覧を取得できませんでした".to_owned()),
                )
                .await
            }
        }
    } else {
        view_async(store, config, None).await
    }
}

pub(crate) async fn reload_opencode_models(state: &DesktopState) -> ModelCatalogView {
    let config = state.runtime_config();
    let store = Arc::new(ModelCatalogStore::new(&state.paths));
    match load_opencode_models(&config, state.cancellation.child_token()).await {
        Ok(models) => {
            let store_for_update = store.clone();
            if tokio::task::spawn_blocking(move || store_for_update.update_opencode(models))
                .await
                .ok()
                .and_then(Result::ok)
                .is_some()
            {
                view_async(store.clone(), state.runtime_config(), None).await
            } else {
                let _ = state.logger.write(
                    "DEBUG",
                    "opencode のモデル一覧を保存できませんでした: reason=state",
                );
                view_async(
                    store.clone(),
                    config.clone(),
                    Some("一覧を保存できませんでした".to_owned()),
                )
                .await
            }
        }
        Err(error) => {
            let _ = state.logger.write(
                "DEBUG",
                &format!(
                    "opencode のモデル一覧を取得できませんでした: reason={}",
                    error.reason()
                ),
            );
            view_async(store, config, Some("一覧を取得できませんでした".to_owned())).await
        }
    }
}

pub(crate) async fn record_companion_selection(
    paths: &ConfigPaths,
    provider: &str,
    model: &str,
) -> Result<(), String> {
    let store = ModelCatalogStore::new(paths);
    let provider = provider.to_owned();
    let model = model.to_owned();
    tokio::task::spawn_blocking(move || store.record_history(&provider, &model))
        .await
        .map_err(|_| "worker".to_owned())?
        .map_err(|error| error.reason().to_owned())
}

pub(crate) fn start(state: Arc<DesktopState>) {
    tauri::async_runtime::spawn(async move {
        let store = Arc::new(ModelCatalogStore::new(&state.paths));
        let mut client: Option<HttpRemoteModelsClient> = None;
        loop {
            if state.cancellation.is_cancelled() {
                return;
            }
            if state.runtime_config().app.check_for_updates {
                if client.is_none() {
                    client = match HttpRemoteModelsClient::new() {
                        Ok(client) => Some(client),
                        Err(_) => {
                            let _ = state.logger.write(
                                "DEBUG",
                                "モデル候補のリモート更新をスキップしました: reason=client",
                            );
                            None
                        }
                    };
                }
                if let Some(client) = client.as_ref() {
                    let outcome = refresh_remote_models(true, false, client, store.clone()).await;
                    log_remote_refresh(state.logger.as_ref(), outcome);
                }
            }
            tokio::select! {
                _ = state.cancellation.cancelled() => return,
                _ = tokio::time::sleep(REMOTE_CHECK_INTERVAL) => {}
            }
        }
    });
}

