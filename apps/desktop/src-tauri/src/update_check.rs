use crate::bubbles::{self, BubbleRecord};
use crate::state::DesktopState;
use async_trait::async_trait;
use coosenpai_core::persistence::{atomic_write_json, SiblingLock};
use coosenpai_core::ports::RuntimeLogger;
use reqwest::{Client, StatusCode};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub(crate) const RELEASES_URL: &str = "https://github.com/nrslib/CooSenpAI/releases";

const LATEST_RELEASE_API_URL: &str =
    "https://api.github.com/repos/nrslib/CooSenpAI/releases/latest";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const STATE_SCHEMA_VERSION: u8 = 1;

#[async_trait]
trait LatestReleaseClient: Send + Sync {
    async fn latest_tag(&self) -> Result<String, ReleaseFetchError>;
}

struct GitHubReleaseClient {
    client: Client,
}

impl GitHubReleaseClient {
    fn new() -> Result<Self, reqwest::Error> {
        Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("CooSenpAI/", env!("CARGO_PKG_VERSION")))
            .build()
            .map(|client| Self { client })
    }
}

#[derive(Debug, Clone, Copy)]
enum ReleaseFetchError {
    Timeout,
    Transport,
    RateLimited,
    HttpStatus,
    InvalidResponse,
}

impl ReleaseFetchError {
    fn reason(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Transport => "network",
            Self::RateLimited => "rate-limit",
            Self::HttpStatus => "http-status",
            Self::InvalidResponse => "invalid-response",
        }
    }
}

#[derive(Debug, Deserialize)]
struct LatestReleasePayload {
    tag_name: String,
}

#[async_trait]
impl LatestReleaseClient for GitHubReleaseClient {
    async fn latest_tag(&self) -> Result<String, ReleaseFetchError> {
        let response = self
            .client
            .get(LATEST_RELEASE_API_URL)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    ReleaseFetchError::Timeout
                } else {
                    ReleaseFetchError::Transport
                }
            })?;
        let status = response.status();
        if matches!(
            status,
            StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS
        ) {
            return Err(ReleaseFetchError::RateLimited);
        }
        if !status.is_success() {
            return Err(ReleaseFetchError::HttpStatus);
        }
        response
            .json::<LatestReleasePayload>()
            .await
            .map(|release| release.tag_name)
            .map_err(|error| {
                if error.is_timeout() {
                    ReleaseFetchError::Timeout
                } else {
                    ReleaseFetchError::InvalidResponse
                }
            })
    }
}

#[async_trait]
trait UpdateNoticeSink: Send + Sync {
    async fn show_update(&self, version: &Version) -> bool;
}

struct DesktopUpdateNoticeSink {
    state: Arc<DesktopState>,
}

#[async_trait]
impl UpdateNoticeSink for DesktopUpdateNoticeSink {
    async fn show_update(&self, version: &Version) -> bool {
        let config = self.state.runtime_config();
        let conversation_generation = self.state.bubbles.lock().await.conversation_generation();
        let record = BubbleRecord {
            id: format!("update-available-{version}"),
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message: format!(
                "新しいバージョン v{version} があります。GitHub Releases からダウンロードできます"
            ),
            message_kind: "notice".to_owned(),
            notification_priority: "info".to_owned(),
            caused_by: None,
            display_name: self.state.runtime_snapshot().companion_display_name,
            persona: config.companion.persona,
            avatar_color: config.ui.avatar_color,
            conversation_generation,
            persistent: false,
            open_url: Some(RELEASES_URL.to_owned()),
            interaction: None,
        };
        bubbles::show_best_effort(
            self.state.clone(),
            record,
            config.notification.bubble_duration_ms,
        )
        .await
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateCheckState {
    schema_version: u8,
    #[serde(default)]
    last_notified_version: Option<String>,
}

impl Default for UpdateCheckState {
    fn default() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            last_notified_version: None,
        }
    }
}

impl UpdateCheckState {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != STATE_SCHEMA_VERSION {
            return Err(format!(
                "更新確認 state schemaVersion が不正です: {}",
                self.schema_version
            ));
        }
        if let Some(version) = &self.last_notified_version {
            let parsed =
                Version::parse(version).map_err(|_| "更新確認 state の版が不正です".to_owned())?;
            if !parsed.pre.is_empty() || parsed.to_string() != version.as_str() {
                return Err("更新確認 state の版が stable semver ではありません".to_owned());
            }
        }
        Ok(())
    }
}

struct UpdateCheckStateStore {
    path: PathBuf,
    lock_path: PathBuf,
}

impl UpdateCheckStateStore {
    fn new(path: PathBuf) -> Self {
        let lock_path = path.with_file_name(".update-check.lock");
        Self { path, lock_path }
    }

    fn load(&self) -> Result<UpdateCheckState, String> {
        let _lock = SiblingLock::acquire(&self.lock_path).map_err(|error| error.to_string())?;
        self.read_unlocked()
    }

    fn mark_notified(&self, version: &Version) -> Result<bool, String> {
        let _lock = SiblingLock::acquire(&self.lock_path).map_err(|error| error.to_string())?;
        let current = self.read_unlocked()?;
        let normalized = version.to_string();
        if current.last_notified_version.as_deref() == Some(normalized.as_str()) {
            return Ok(false);
        }
        let next = UpdateCheckState {
            schema_version: STATE_SCHEMA_VERSION,
            last_notified_version: Some(normalized),
        };
        atomic_write_json(&self.path, &next).map_err(|error| error.to_string())?;
        Ok(true)
    }

    fn read_unlocked(&self) -> Result<UpdateCheckState, String> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(UpdateCheckState::default())
            }
            Err(error) => return Err(error.to_string()),
        };
        let state = serde_json::from_slice::<UpdateCheckState>(&bytes)
            .map_err(|error| error.to_string())?;
        state.validate()?;
        Ok(state)
    }
}

struct UpdateChecker {
    client: Arc<dyn LatestReleaseClient>,
    state: Arc<UpdateCheckStateStore>,
    logger: Arc<dyn RuntimeLogger>,
}

impl UpdateChecker {
    fn new(state_path: PathBuf, logger: Arc<dyn RuntimeLogger>) -> Result<Self, reqwest::Error> {
        let client = Arc::new(GitHubReleaseClient::new()?);
        Ok(Self::with_client(state_path, client, logger))
    }

    fn with_client(
        state_path: PathBuf,
        client: Arc<dyn LatestReleaseClient>,
        logger: Arc<dyn RuntimeLogger>,
    ) -> Self {
        Self {
            client,
            state: Arc::new(UpdateCheckStateStore::new(state_path)),
            logger,
        }
    }

    async fn check(&self, enabled: bool, current_version: &str, notice: &dyn UpdateNoticeSink) {
        if !enabled {
            return;
        }
        let Some(current) = Version::parse(current_version).ok() else {
            self.log_info("更新確認をスキップしました: reason=invalid-current-version");
            return;
        };
        let tag = match self.client.latest_tag().await {
            Ok(tag) => tag,
            Err(error) => {
                self.log_info(&format!(
                    "更新確認をスキップしました: reason={}",
                    error.reason()
                ));
                return;
            }
        };
        let Some(latest) = newer_stable_release(&current, &tag) else {
            self.log_info("更新確認をスキップしました: reason=not-newer-or-invalid-tag");
            return;
        };
        let state = match self.load_state().await {
            Ok(state) => state,
            Err(_) => {
                self.log_info("更新確認をスキップしました: reason=state");
                return;
            }
        };
        if state.last_notified_version.as_deref() == Some(latest.to_string().as_str()) {
            return;
        }
        if !notice.show_update(&latest).await {
            self.log_info("更新通知をスキップしました: reason=notice");
            return;
        }
        match self.mark_notified(&latest).await {
            Ok(_) => {}
            Err(_) => self.log_info("更新通知を表示済みとして保存できませんでした: reason=state"),
        }
    }

    async fn load_state(&self) -> Result<UpdateCheckState, String> {
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || state.load())
            .await
            .map_err(|error| error.to_string())?
    }

    async fn mark_notified(&self, version: &Version) -> Result<bool, String> {
        let state = self.state.clone();
        let version = version.clone();
        tokio::task::spawn_blocking(move || state.mark_notified(&version))
            .await
            .map_err(|error| error.to_string())?
    }

    fn log_info(&self, message: &str) {
        let _ = self.logger.write("INFO", message);
    }
}

pub(crate) fn start(state: Arc<DesktopState>) {
    tauri::async_runtime::spawn(async move {
        let logger: Arc<dyn RuntimeLogger> = state.logger.clone();
        let checker = match UpdateChecker::new(state.paths.update_check.clone(), logger) {
            Ok(checker) => checker,
            Err(_) => {
                let _ = state
                    .logger
                    .write("INFO", "更新確認をスキップしました: reason=client");
                return;
            }
        };
        loop {
            if state.cancellation.is_cancelled() {
                return;
            }
            if state.runtime_config().app.check_for_updates {
                let notice = DesktopUpdateNoticeSink {
                    state: state.clone(),
                };
                checker
                    .check(true, env!("CARGO_PKG_VERSION"), &notice)
                    .await;
            }
            tokio::select! {
                _ = state.cancellation.cancelled() => return,
                _ = tokio::time::sleep(CHECK_INTERVAL) => {}
            }
        }
    });
}

fn newer_stable_release(current: &Version, tag: &str) -> Option<Version> {
    let latest = stable_release_version(tag)?;
    (latest > *current).then_some(latest)
}

fn stable_release_version(tag: &str) -> Option<Version> {
    let tag = tag.trim();
    let version_text = tag
        .strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag);
    let version = Version::parse(version_text).ok()?;
    version.pre.is_empty().then_some(version)
}

