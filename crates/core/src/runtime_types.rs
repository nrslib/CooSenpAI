use crate::companion::{AttachmentOcrFailureKind, CompanionAgent, CompanionError};
use crate::config::{Config, ConfigError, ConfigValidationIssue};
use crate::memory::{MemoryService, MemoryStatus};
use crate::observer::{ObserverAgent, ObserverError};
use crate::provider::ProviderUsage;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    pub revision: u64,
    pub phase: RuntimePhase,
    pub pending_observations: usize,
    pub last_error: Option<RuntimeLastError>,
    pub companion_retry_in_seconds: Option<u64>,
    pub pending_deliveries: usize,
    pub delivery_outbox_blocked: bool,
    pub memory_status: MemoryStatus,
    pub companion_display_name: String,
    #[serde(default)]
    pub proactive_limit_reached: bool,
    pub active_user_message_id: Option<String>,
    pub cancelled_user_message_ids: Vec<String>,
    pub companion_draft: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_companion_thought: Option<String>,
    pub provider_usage: ProviderUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLastError {
    pub kind: RuntimeErrorKind,
    pub occurred_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<ConfigValidationIssue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachment_ocr: Option<RuntimeAttachmentOcrFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAttachmentOcrFailure {
    pub input_id: String,
    pub reason: AttachmentOcrFailureKind,
    pub attempts: u8,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeErrorKind {
    Config,
    Provider,
    Persistence,
    Mailbox,
    Outbox,
    Logging,
    Serialization,
}

impl RuntimeErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Provider => "provider",
            Self::Persistence => "persistence",
            Self::Mailbox => "mailbox",
            Self::Outbox => "outbox",
            Self::Logging => "logging",
            Self::Serialization => "serialization",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimePhase {
    Idle,
    Observing,
    Companion,
    Stopping,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("runtime が停止しています")]
    Closed,
    #[error("設定反映に伴い provider の呼び出しをキャンセルしました")]
    ConfigUpdateCancelled,
    #[error("observer が設定されていません")]
    ObserverUnavailable,
    #[error("companion が設定されていません")]
    CompanionUnavailable,
    #[error("observer エラー: {0}")]
    Observer(#[from] ObserverError),
    #[error("companion エラー: {0}")]
    Companion(#[from] CompanionError),
    #[error("runtime 応答を受け取れませんでした")]
    ResponseDropped,
    #[error("設定更新中のため provider 操作を開始できません")]
    ProviderStartsBlocked,
    #[error("見守り対象の変更前に取得した frame です")]
    StaleWatchScope,
    #[error("runtime の設定が不正です: {0}")]
    Config(#[from] ConfigError),
    #[error("runtime の構成を作成できません: {0}")]
    Factory(String),
}

pub struct RuntimeAgents {
    pub observer: Option<ObserverAgent>,
    pub companion: Option<CompanionAgent>,
    pub memory: Option<MemoryService>,
}

#[async_trait]
pub trait RuntimeFactory: Send + Sync {
    async fn build(&self, config: &Config) -> Result<RuntimeAgents, String>;

    async fn shutdown(&self) {}
}
