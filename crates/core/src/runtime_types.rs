use crate::companion::{AttachmentOcrFailureKind, CompanionAgent, CompanionError};
use crate::config::{Config, ConfigError, ConfigValidationIssue};
use crate::locale::{text, Locale, TextKey};
use crate::memory::{MemoryService, MemoryStatus};
use crate::observer::{ObserverAgent, ObserverError};
use crate::provider::ProviderUsage;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    #[serde(default)]
    pub companion_emotions: crate::emotion::EmotionState,
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
    pub user_work_pending: bool,
    pub cancelled_user_message_ids: Vec<String>,
    pub companion_draft: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_companion_thought: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_companion_decision: Option<CompanionDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_user_interruption: Option<UserInterruption>,
    pub latest_companion_thought_generation: Option<u64>,
    pub provider_usage: ProviderUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UserInterruption {
    pub sequence: u64,
    pub occurred_at: String,
    pub observer: bool,
    pub proactive: bool,
}

// companion の emit 判断を details のデータフロー表示へ流すための眺め。永続はしない。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionDecision {
    pub sequence: u64,
    pub occurred_at: String,
    pub emit: bool,
    pub message_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observation_ids: Vec<String>,
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
    #[error("利用者入力で観察を中断しました")]
    ObservationCancelled,
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

impl RuntimeError {
    pub fn format_for_user(&self) -> String {
        self.format_for_locale(Locale::Ja)
    }

    pub fn format_for_locale(&self, locale: Locale) -> String {
        if locale == Locale::Ja {
            return self.to_string();
        }
        match self {
            Self::Closed => text(TextKey::RuntimeClosed, locale).to_owned(),
            Self::ConfigUpdateCancelled => {
                text(TextKey::RuntimeConfigUpdateCancelled, locale).to_owned()
            }
            Self::ObservationCancelled => {
                text(TextKey::RuntimeObservationCancelled, locale).to_owned()
            }
            Self::ObserverUnavailable => {
                text(TextKey::RuntimeObserverUnavailable, locale).to_owned()
            }
            Self::CompanionUnavailable => {
                text(TextKey::RuntimeCompanionUnavailable, locale).to_owned()
            }
            Self::ResponseDropped => text(TextKey::RuntimeResponseDropped, locale).to_owned(),
            Self::ProviderStartsBlocked => {
                text(TextKey::RuntimeProviderStartsBlocked, locale).to_owned()
            }
            Self::StaleWatchScope => text(TextKey::RuntimeStaleWatchScope, locale).to_owned(),
            Self::Observer(error) => {
                text(TextKey::RuntimeObserverError, locale).replace("{error}", &error.to_string())
            }
            Self::Companion(error) => {
                text(TextKey::RuntimeCompanionError, locale).replace("{error}", &error.to_string())
            }
            Self::Config(error) => text(TextKey::RuntimeConfigInvalid, locale)
                .replace("{error}", &error.format_for_locale(locale)),
            Self::Factory(error) => {
                let factory_detail = crate::locale::localize_factory_message(error, locale);
                let detail = if factory_detail.as_str() == error.as_str() {
                    crate::locale::localize_onboarding_message(error, locale)
                } else {
                    factory_detail
                };
                text(TextKey::RuntimeFactoryError, locale).replace("{error}", &detail)
            }
        }
    }
}

pub struct RuntimeAgents {
    pub observer: Option<ObserverAgent>,
    pub companion: Option<CompanionAgent>,
    pub memory: Option<MemoryService>,
    pub observation_delivery: ObservationDelivery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationDelivery {
    Companion,
    CallerOnly,
}

#[async_trait]
pub trait RuntimeFactory: Send + Sync {
    async fn build(&self, config: &Config) -> Result<RuntimeAgents, String>;

    async fn shutdown(&self) {}
}
