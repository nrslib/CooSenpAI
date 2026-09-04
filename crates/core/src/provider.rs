use crate::process::{ProcessRequest, ProcessRunner, TokioProcessRunner};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

mod bridge;
mod bridge_io;
mod bridge_provider;
mod bridge_validation;
#[path = "provider_output.rs"]
mod provider_output;
pub use bridge::{BridgeLaunch, ProviderBridge};
pub use bridge_provider::BridgeProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderName {
    Codex,
    Claude,
    Opencode,
}

impl ProviderName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Opencode => "opencode",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSession {
    pub provider: ProviderName,
    pub model: Option<String>,
    pub id: String,
}

#[derive(Debug, Clone)]
pub enum SessionRequest {
    New,
    Ephemeral,
    Resume(ProviderSession),
}

#[derive(Debug, Clone)]
pub struct ProviderImageAttachment {
    pub path: PathBuf,
}

impl From<PathBuf> for ProviderImageAttachment {
    fn from(path: PathBuf) -> Self {
        Self { path }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderMidTurnInput {
    pub source_id: String,
    pub message: String,
    pub images: Vec<ProviderImageAttachment>,
}

#[derive(Debug, Clone)]
pub struct ProviderCall {
    pub system_prompt: String,
    pub prompt: String,
    pub images: Vec<ProviderImageAttachment>,
    pub tools_disabled: bool,
    pub output_schema: Option<Value>,
    pub session: SessionRequest,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub timeout: Duration,
    pub tutorial_response_key: Option<String>,
}

pub type ProviderCallOptions = ProviderCall;

pub(crate) fn bridge_send_request_fits(input: &ProviderCall) -> bool {
    bridge::send_request_fits(input)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub call_id: Option<String>,
    pub provider: Option<ProviderName>,
    pub model: Option<String>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapabilities {
    pub default_model: String,
    pub model_candidates: Vec<String>,
    pub image_input: bool,
    pub native_structured_output: bool,
    pub effective_structured_output: bool,
    pub streaming: bool,
    pub cancellation: bool,
    pub session_resume: bool,
    pub session_compact: bool,
    pub effort: bool,
    pub mid_turn_input: bool,
}

#[derive(Debug, Clone)]
pub struct ProviderResult {
    pub text: String,
    pub value: Option<Value>,
    pub session: Option<ProviderSession>,
}

#[derive(Debug, Clone)]
pub struct ProviderCompactSessionOptions {
    pub session: ProviderSession,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    Retryable,
    Auth,
    Unsupported,
    InvalidModel,
    InvalidOutput,
}

impl ProviderErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Retryable => "retryable",
            Self::Auth => "auth",
            Self::Unsupported => "unsupported",
            Self::InvalidModel => "invalid-model",
            Self::InvalidOutput => "invalid-output",
        }
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
}

pub trait ProviderEventSink: Send + Sync {
    fn delta(&self, _text: &str) {}
    fn usage(&self, _usage: &ProviderUsage) {}
    fn reset(&self) {}
    fn mid_turn_accepted(&self, _source_id: &str) {}
}

#[derive(Debug, Default)]
pub struct IgnoreProviderEvents;

impl ProviderEventSink for IgnoreProviderEvents {}

#[async_trait]
pub trait ProviderClient: Send + Sync {
    fn cancellation_must_complete(&self) -> bool {
        false
    }

    fn provider_name(&self) -> Option<ProviderName> {
        None
    }

    fn capabilities(&self) -> Option<ProviderCapabilities> {
        None
    }

    async fn resolve_capabilities(
        &self,
        _cancellation: CancellationToken,
        _timeout: Duration,
    ) -> Result<Option<ProviderCapabilities>, ProviderError> {
        Ok(self.capabilities())
    }

    async fn resolve_model_capabilities(
        &self,
        _model: Option<&str>,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Result<Option<ProviderCapabilities>, ProviderError> {
        self.resolve_capabilities(cancellation, timeout).await
    }

    async fn call(
        &self,
        input: ProviderCallOptions,
        cancellation: CancellationToken,
    ) -> Result<ProviderResult, ProviderError>;

    async fn call_streaming(
        &self,
        input: ProviderCallOptions,
        cancellation: CancellationToken,
        events: Arc<dyn ProviderEventSink>,
    ) -> Result<ProviderResult, ProviderError> {
        let result = self.call(input, cancellation).await?;
        events.delta(&result.text);
        Ok(result)
    }

    async fn call_streaming_with_mid_turn(
        &self,
        input: ProviderCallOptions,
        cancellation: CancellationToken,
        events: Arc<dyn ProviderEventSink>,
        _additional_inputs: tokio::sync::mpsc::UnboundedReceiver<ProviderMidTurnInput>,
    ) -> Result<ProviderResult, ProviderError> {
        self.call_streaming(input, cancellation, events).await
    }

    async fn compact_session(
        &self,
        _options: ProviderCompactSessionOptions,
        _cancellation: CancellationToken,
    ) -> Result<(), ProviderError> {
        Err(ProviderError {
            kind: ProviderErrorKind::Unsupported,
            message: "provider は明示的な session compact に対応していません。".to_owned(),
        })
    }
}

pub fn resolve_executable(name: &str, path_value: &str) -> Result<PathBuf, ProviderError> {
    let candidate = Path::new(name);
    if candidate.is_absolute() {
        return is_executable(candidate)
            .then(|| candidate.to_path_buf())
            .ok_or_else(executable_not_found);
    }
    std::env::split_paths(path_value)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(name))
        .find(|path| is_executable(path))
        .ok_or_else(executable_not_found)
}

pub async fn validate_node_version(
    node: &Path,
    path_value: &str,
    cancellation: CancellationToken,
) -> Result<(), ProviderError> {
    let output = TokioProcessRunner
        .run(
            ProcessRequest {
                executable: node.to_path_buf(),
                args: vec!["--version".to_owned()],
                env: vec![("PATH".to_owned(), path_value.to_owned())],
                cwd: None,
                stdin: Vec::new(),
                timeout: Duration::from_secs(3),
            },
            cancellation,
        )
        .await
        .map_err(|_| node_version_error())?;
    if output.status != Some(0) {
        return Err(node_version_error());
    }
    let version = std::str::from_utf8(&output.stdout).map_err(|_| node_version_error())?;
    let major = version
        .trim()
        .strip_prefix('v')
        .and_then(|value| value.split('.').next())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(node_version_error)?;
    if major < 18 {
        return Err(node_version_error());
    }
    Ok(())
}

fn node_version_error() -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::Unsupported,
        message: "Node.js 18 以上が必要です。".to_owned(),
    }
}

fn executable_not_found() -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::Retryable,
        message: "実行ファイルが見つかりません。".to_owned(),
    }
}

/// ログイン shell の PATH は provider 構成候補ごとに一度取得し、その候補内で固定する。
pub async fn resolve_login_shell_path(cancellation: CancellationToken) -> String {
    let fallback = std::env::var("PATH").unwrap_or_default();
    let shell = std::env::var_os("SHELL").map(PathBuf::from);
    let Some(shell) = shell.filter(|path| path.is_absolute()) else {
        return fallback;
    };
    let marker = format!("__COOSENPAI_PATH_{}__", uuid::Uuid::new_v4().simple());
    let command = format!("printf '{marker}%s{marker}' \"$(/usr/bin/printenv PATH)\"");
    let output = TokioProcessRunner
        .run(
            ProcessRequest {
                executable: shell,
                args: vec!["-ilc".to_owned(), command],
                env: Vec::new(),
                cwd: None,
                stdin: Vec::new(),
                timeout: Duration::from_secs(5),
            },
            cancellation,
        )
        .await;
    let Ok(output) = output else { return fallback };
    if output.status != Some(0) || output.stdout.len() > 64 * 1024 {
        return fallback;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(start) = text.find(&marker) else {
        return fallback;
    };
    let rest = &text[start + marker.len()..];
    let Some(end) = rest.find(&marker) else {
        return fallback;
    };
    let path = &rest[..end];
    if path.is_empty() || std::env::split_paths(path).any(|entry| !entry.is_absolute()) {
        fallback
    } else {
        path.to_owned()
    }
}

fn is_executable(path: &Path) -> bool {
    path.is_file() && {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            path.metadata()
                .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        }
        #[cfg(not(unix))]
        {
            true
        }
    }
}

