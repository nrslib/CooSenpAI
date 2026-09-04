use crate::persistence::PersistenceError;
#[path = "config_issue_paths.rs"]
mod config_issue_paths;
#[path = "config_parse.rs"]
mod config_parse;
#[path = "config_presence.rs"]
mod config_presence;
#[path = "config_shortcut.rs"]
mod config_shortcut;
#[path = "config_defaults.rs"]
mod defaults;
#[path = "config_paths.rs"]
mod paths;
#[path = "config_storage.rs"]
mod storage;
#[path = "config_validate.rs"]
mod validate;
use chrono::{DateTime, Local, TimeZone, Utc};
pub use config_issue_paths::config_issue_path_patterns;
pub use config_presence::{AppConfig, CompanionReminder};
pub use config_shortcut::shortcut_identity;
use defaults::*;
pub use paths::ConfigPaths;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;
pub use storage::{
    ensure_layout, load_config, patch_config, patch_config_before_save, save_config,
};
use thiserror::Error;
pub use validate::validate_config;
pub const CONFIG_VERSION: u8 = 3;
pub const PRODUCT_DIR: &str = ".coosenpai";
pub const PENDING_DELIVERY_ITEM_MAX_BYTES: usize = 1_052_672;
pub const POPUP_QUICK_ACTION_LIMIT: usize = 12;
pub const POPUP_QUICK_ACTION_LABEL_MAX_CHARS: usize = 40;
pub const POPUP_QUICK_ACTION_MESSAGE_MAX_BYTES: usize = 32 * 1_024;
pub const NUMERIC_CONFIG_PATHS: &[&str] = &[
    "watch.sendIntervalMs",
    "watch.sendDebounceMs",
    "watch.framesPerSend",
    "watch.downscaleWidth",
    "watch.triggers.typingPauseMs",
    "watch.triggers.activeThresholdMs",
    "watch.triggers.appSwitchSettleMs",
    "watch.triggers.maxIntervalMs",
    "watch.triggers.minSpacingMs",
    "watch.triggers.pollMs",
    "watch.battery.multiplier",
    "watch.ocrGate.timeoutMs",
    "observer.timeoutMs",
    "observer.dailyCallLimit",
    "observer.textExcerptMaxChars",
    "observer.textExcerptMaxCount",
    "observer.textTotalMaxChars",
    "observer.changesMaxCount",
    "companion.timeoutMs",
    "companion.dailyProactiveLimit",
    "companion.wakeCoalesceMax",
    "companion.sessionMaxCalls",
    "companion.stuckAfterMs",
    "companion.pendingDeliveryLimit",
    "companion.pendingDeliveryMaxBytes",
    "companion.contextRefreshCalls",
    "companion.proactiveQuietMinutes",
    "notification.bubbleDurationMs",
    "bubble.maxStack",
    "retention.observationDays",
    "retention.conversationDays",
    "memory.graceMinutes",
    "memory.dailyRetentionDays",
    "memory.weeklyRetentionWeeks",
    "memory.jobRetentionDays",
    "memory.sourceMaxBytes",
    "memory.promptMaxBytes",
    "memory.factLimit",
    "memory.factMaxBytes",
    "memory.candidateLimit",
    "memory.candidateMaxBytes",
    "memory.storageMaxBytes",
    "memory.factPromptDailyLimit",
];
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub config_version: u8,
    #[serde(default)]
    pub watch: WatchConfig,
    #[serde(default)]
    pub observer: AgentConfig,
    #[serde(default)]
    pub companion: CompanionConfig,
    #[serde(default)]
    pub chat: ChatConfig,
    #[serde(default)]
    pub notification: NotificationConfig,
    #[serde(default)]
    pub bubble: BubbleConfig,
    #[serde(default)]
    pub retention: RetentionConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub debug: DebugConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub speech: SpeechConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub keymap: KeymapConfig,
    #[serde(default)]
    pub popup: PopupConfig,
    #[serde(default)]
    pub app: AppConfig,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            watch: WatchConfig::default(),
            observer: AgentConfig::default_observer(),
            companion: CompanionConfig::default(),
            chat: ChatConfig::default(),
            notification: NotificationConfig::default(),
            bubble: BubbleConfig::default(),
            retention: RetentionConfig::default(),
            memory: MemoryConfig::default(),
            debug: DebugConfig::default(),
            audio: AudioConfig::default(),
            speech: SpeechConfig::default(),
            ui: UiConfig::default(),
            keymap: KeymapConfig::default(),
            popup: PopupConfig::default(),
            app: AppConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChatConfig {
    #[serde(default = "default_while_thinking")]
    pub while_thinking: String,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            while_thinking: default_while_thinking(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub fullscreen: bool,
    #[serde(default)]
    pub apps: Vec<WatchAppConfig>,
    #[serde(default = "default_send_interval")]
    pub send_interval_ms: u64,
    #[serde(default = "default_send_debounce")]
    pub send_debounce_ms: u64,
    #[serde(default = "default_frames_per_send")]
    pub frames_per_send: usize,
    #[serde(default = "default_downscale_width")]
    pub downscale_width: u32,
    #[serde(default)]
    pub triggers: TriggerConfig,
    #[serde(default)]
    pub battery: BatteryConfig,
    #[serde(default)]
    pub ocr_gate: OcrGateConfig,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            fullscreen: false,
            apps: Vec::new(),
            send_interval_ms: default_send_interval(),
            send_debounce_ms: default_send_debounce(),
            frames_per_send: default_frames_per_send(),
            downscale_width: default_downscale_width(),
            triggers: TriggerConfig::default(),
            battery: BatteryConfig::default(),
            ocr_gate: OcrGateConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchAppConfig {
    pub bundle_id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mic: bool,
    #[serde(default)]
    pub speaker: bool,
    #[serde(default)]
    pub debug_dump_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeechConfig {
    #[serde(default = "default_speech_locale")]
    pub locale: String,
    #[serde(default = "default_speech_mode")]
    pub mode: String,
    #[serde(default = "default_speech_confirm_before_send")]
    pub confirm_before_send: bool,
    #[serde(default = "default_speech_input_device")]
    pub input_device: String,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            locale: default_speech_locale(),
            mode: default_speech_mode(),
            confirm_before_send: default_speech_confirm_before_send(),
            input_device: default_speech_input_device(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiConfig {
    #[serde(default)]
    pub avatar_color: Option<String>,
    #[serde(default)]
    pub avatar_path: Option<String>,
    #[serde(default = "default_ui_theme")]
    pub theme: String,
    #[serde(default = "default_ui_font")]
    pub font: String,
    #[serde(default = "default_true")]
    pub thought_bubble: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeymapConfig {
    #[serde(default = "default_capture_shortcut")]
    pub capture_region: Option<String>,
    #[serde(default = "default_microphone_shortcut")]
    pub microphone: Option<String>,
    #[serde(default = "default_toggle_panel_shortcut")]
    pub toggle_panel: Option<String>,
    #[serde(default = "default_toggle_watch_shortcut")]
    pub toggle_watch: Option<String>,
    #[serde(default = "default_send_text_shortcut")]
    pub send_text: Option<String>,
    #[serde(default = "default_copy_last_reply_shortcut")]
    pub copy_last_reply: Option<String>,
    #[serde(default = "default_send_key")]
    pub send_key: String,
}

impl Default for KeymapConfig {
    fn default() -> Self {
        Self {
            capture_region: default_capture_shortcut(),
            microphone: default_microphone_shortcut(),
            toggle_panel: default_toggle_panel_shortcut(),
            toggle_watch: default_toggle_watch_shortcut(),
            send_text: default_send_text_shortcut(),
            copy_last_reply: default_copy_last_reply_shortcut(),
            send_key: default_send_key(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PopupQuickAction {
    pub label: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PopupQuickActionsConfig {
    #[serde(default = "default_text_quick_actions")]
    pub text: Vec<PopupQuickAction>,
    #[serde(default = "default_image_quick_actions")]
    pub image: Vec<PopupQuickAction>,
}

impl Default for PopupQuickActionsConfig {
    fn default() -> Self {
        Self {
            text: default_text_quick_actions(),
            image: default_image_quick_actions(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PopupConfig {
    #[serde(default)]
    pub quick_actions: PopupQuickActionsConfig,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            avatar_color: None,
            avatar_path: None,
            theme: default_ui_theme(),
            font: default_ui_font(),
            thought_bubble: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TriggerConfig {
    #[serde(default = "default_typing_pause")]
    pub typing_pause_ms: u64,
    #[serde(default = "default_active_threshold")]
    pub active_threshold_ms: u64,
    #[serde(default = "default_true")]
    pub app_switch: bool,
    #[serde(default = "default_app_switch_settle")]
    pub app_switch_settle_ms: u64,
    #[serde(default = "default_max_interval")]
    pub max_interval_ms: u64,
    #[serde(default = "default_min_spacing")]
    pub min_spacing_ms: u64,
    #[serde(default = "default_poll")]
    pub poll_ms: u64,
}

impl Default for TriggerConfig {
    fn default() -> Self {
        Self {
            typing_pause_ms: default_typing_pause(),
            active_threshold_ms: default_active_threshold(),
            app_switch: true,
            app_switch_settle_ms: default_app_switch_settle(),
            max_interval_ms: default_max_interval(),
            min_spacing_ms: default_min_spacing(),
            poll_ms: default_poll(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatteryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_battery_multiplier")]
    pub multiplier: f64,
}

impl Default for BatteryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            multiplier: default_battery_multiplier(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OcrGateConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_ocr_level")]
    pub level: String,
    #[serde(default = "default_ocr_timeout")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub executable: Option<String>,
}

impl Default for OcrGateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            level: default_ocr_level(),
            timeout_ms: default_ocr_timeout(),
            executable: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentConfig {
    #[serde(default = "default_codex")]
    pub provider: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_effort")]
    pub effort: String,
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default = "default_agent_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_observer_daily_limit")]
    pub daily_call_limit: u32,
    #[serde(default = "default_excerpt_max_chars")]
    pub text_excerpt_max_chars: usize,
    #[serde(default = "default_excerpt_count")]
    pub text_excerpt_max_count: usize,
    #[serde(default = "default_total_excerpt_chars")]
    pub text_total_max_chars: usize,
    #[serde(default = "default_changes_max")]
    pub changes_max_count: usize,
}

impl AgentConfig {
    fn default_observer() -> Self {
        Self {
            provider: default_codex(),
            model: default_model(),
            effort: default_effort(),
            executable: None,
            timeout_ms: default_agent_timeout(),
            daily_call_limit: default_observer_daily_limit(),
            text_excerpt_max_chars: default_excerpt_max_chars(),
            text_excerpt_max_count: default_excerpt_count(),
            text_total_max_chars: default_total_excerpt_chars(),
            changes_max_count: default_changes_max(),
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self::default_observer()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompanionConfig {
    #[serde(default = "default_codex")]
    pub provider: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_effort")]
    pub effort: String,
    #[serde(default)]
    pub executable: Option<String>,
    #[serde(default = "default_persona")]
    pub persona: String,
    #[serde(default = "default_display_name")]
    pub display_name: String,
    #[serde(default = "default_assertiveness")]
    pub assertiveness: String,
    #[serde(default = "default_agent_timeout")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub daily_proactive_limit: Option<u32>,
    #[serde(default = "default_wake_coalesce")]
    pub wake_coalesce_max: usize,
    #[serde(default = "default_session_max")]
    pub session_max_calls: usize,
    #[serde(default = "default_stuck_after")]
    pub stuck_after_ms: u64,
    #[serde(default = "default_pending_delivery_limit")]
    pub pending_delivery_limit: usize,
    #[serde(default = "default_pending_delivery_max_bytes")]
    pub pending_delivery_max_bytes: usize,
    #[serde(default = "default_context_refresh_calls")]
    pub context_refresh_calls: usize,
    #[serde(default = "default_review_time")]
    pub review_time: String,
    #[serde(default)]
    pub reminders: Vec<CompanionReminder>,
    #[serde(default, skip_serializing)]
    pub quiet_report_every: Option<u32>,
    #[serde(default = "default_proactive_quiet_minutes")]
    pub proactive_quiet_minutes: u64,
}

impl Default for CompanionConfig {
    fn default() -> Self {
        Self {
            provider: default_codex(),
            model: default_model(),
            effort: default_effort(),
            executable: None,
            persona: default_persona(),
            display_name: default_display_name(),
            assertiveness: default_assertiveness(),
            timeout_ms: default_agent_timeout(),
            daily_proactive_limit: None,
            wake_coalesce_max: default_wake_coalesce(),
            session_max_calls: default_session_max(),
            stuck_after_ms: default_stuck_after(),
            pending_delivery_limit: default_pending_delivery_limit(),
            pending_delivery_max_bytes: default_pending_delivery_max_bytes(),
            context_refresh_calls: default_context_refresh_calls(),
            review_time: default_review_time(),
            reminders: Vec::new(),
            quiet_report_every: None,
            proactive_quiet_minutes: default_proactive_quiet_minutes(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationConfig {
    #[serde(default = "default_notification_mode")]
    pub mode: String,
    #[serde(default = "default_notification_min_priority")]
    pub min_priority: String,
    #[serde(default = "default_notification_ttl")]
    pub bubble_duration_ms: u64,
    #[serde(default)]
    pub show_priority: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BubbleConfig {
    #[serde(default)]
    pub always_show: bool,
    #[serde(default = "default_bubble_keep_latest")]
    pub keep_latest: bool,
    #[serde(default = "default_bubble_max_stack")]
    pub max_stack: usize,
    #[serde(default = "default_bubble_position")]
    pub position: String,
    #[serde(default = "default_bubble_display")]
    pub display: String,
}

impl Default for BubbleConfig {
    fn default() -> Self {
        Self {
            always_show: false,
            keep_latest: default_bubble_keep_latest(),
            max_stack: default_bubble_max_stack(),
            position: default_bubble_position(),
            display: default_bubble_display(),
        }
    }
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            mode: default_notification_mode(),
            min_priority: default_notification_min_priority(),
            bubble_duration_ms: default_notification_ttl(),
            show_priority: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetentionConfig {
    #[serde(default = "default_observation_retention")]
    pub observation_days: u64,
    #[serde(default = "default_conversation_retention")]
    pub conversation_days: u64,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            observation_days: default_observation_retention(),
            conversation_days: default_conversation_retention(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub provider_consent: bool,
    #[serde(default = "default_memory_grace_minutes")]
    pub grace_minutes: u64,
    #[serde(default = "default_daily_memory_retention")]
    pub daily_retention_days: u64,
    #[serde(default = "default_weekly_memory_retention")]
    pub weekly_retention_weeks: u64,
    #[serde(default = "default_memory_job_retention")]
    pub job_retention_days: u64,
    #[serde(default = "default_memory_source_max_bytes")]
    pub source_max_bytes: usize,
    #[serde(default = "default_memory_prompt_max_bytes")]
    pub prompt_max_bytes: usize,
    #[serde(default = "default_memory_fact_limit")]
    pub fact_limit: usize,
    #[serde(default = "default_memory_fact_max_bytes")]
    pub fact_max_bytes: usize,
    #[serde(default = "default_memory_candidate_limit")]
    pub candidate_limit: usize,
    #[serde(default = "default_memory_candidate_max_bytes")]
    pub candidate_max_bytes: usize,
    #[serde(default = "default_memory_storage_max_bytes")]
    pub storage_max_bytes: usize,
    #[serde(default = "default_fact_prompt_daily_limit")]
    pub fact_prompt_daily_limit: u32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider_consent: false,
            grace_minutes: default_memory_grace_minutes(),
            daily_retention_days: default_daily_memory_retention(),
            weekly_retention_weeks: default_weekly_memory_retention(),
            job_retention_days: default_memory_job_retention(),
            source_max_bytes: default_memory_source_max_bytes(),
            prompt_max_bytes: default_memory_prompt_max_bytes(),
            fact_limit: default_memory_fact_limit(),
            fact_max_bytes: default_memory_fact_max_bytes(),
            candidate_limit: default_memory_candidate_limit(),
            candidate_max_bytes: default_memory_candidate_max_bytes(),
            storage_max_bytes: default_memory_storage_max_bytes(),
            fact_prompt_daily_limit: default_fact_prompt_daily_limit(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("設定ファイルを読み込めません: {0}")]
    Io(#[from] io::Error),
    #[error("設定 JSON が不正です: {0}")]
    Json(#[from] serde_json::Error),
    #[error("設定の検証に失敗しました: {0:?}")]
    Validation(Vec<ConfigValidationIssue>),
    #[error("設定バージョン {0} は未対応です")]
    UnsupportedVersion(u64),
    #[error("設定の lock を取得できません: {0}")]
    Persistence(#[from] PersistenceError),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfigValidationIssue {
    pub path: String,
    pub message: String,
}

impl ConfigError {
    pub fn format_for_user(&self) -> String {
        match self {
            Self::Validation(issues) => issues
                .iter()
                .map(|issue| format!("{}: {}", issue.path, issue.message))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => self.to_string(),
        }
    }
}

pub fn default_config() -> Config {
    Config::default()
}

pub fn normalize_audio_sources_on_enable(audio_was_enabled: bool, config: &mut Config) {
    if !audio_was_enabled && config.audio.enabled && !config.audio.speaker {
        config.audio.speaker = true;
    }
}

pub(super) fn normalize_config(mut config: Config) -> Config {
    config.audio.mic = false;
    for actions in [
        &mut config.popup.quick_actions.text,
        &mut config.popup.quick_actions.image,
    ] {
        for action in actions {
            action.label = action.label.trim().to_owned();
            action.message = action.message.trim().to_owned();
        }
    }
    config
}

pub fn parse_config(value: Value) -> Result<Config, ConfigError> {
    let version = value
        .get("configVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            ConfigError::Validation(vec![issue("configVersion", "3 の整数で指定してください。")])
        })?;
    if version != CONFIG_VERSION as u64 {
        return Err(ConfigError::UnsupportedVersion(version));
    }
    config_parse::parse_v3(value).map(normalize_config)
}

pub(super) fn issue(path: impl Into<String>, message: impl Into<String>) -> ConfigValidationIssue {
    let path = path.into();
    debug_assert!(
        config_issue_paths::is_known_issue_path(&path),
        "設定 issue path が catalog にありません: {path}"
    );
    ConfigValidationIssue {
        path,
        message: message.into(),
    }
}

pub(super) fn unknown_issue(
    scope: &str,
    key: &str,
    message: impl Into<String>,
) -> ConfigValidationIssue {
    debug_assert!(
        config_issue_paths::is_known_unknown_scope(scope),
        "設定の未知キー scope が catalog にありません: {scope}"
    );
    let path = if scope == "config" {
        key.to_owned()
    } else {
        format!("{scope}.{key}")
    };
    ConfigValidationIssue {
        path,
        message: message.into(),
    }
}

pub(super) fn is_valid_persona_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().enumerate().all(|(index, byte)| {
            if index == 0 {
                byte.is_ascii_alphanumeric()
            } else {
                byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
            }
        })
}

pub fn is_valid_avatar_path(value: &str) -> bool {
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        return false;
    }
    let mut components = std::path::Path::new(value).components();
    if !matches!(
        components.next(),
        Some(std::path::Component::Normal(component)) if component == "state"
    ) {
        return false;
    }
    let Some(std::path::Component::Normal(file_name)) = components.next() else {
        return false;
    };
    if components.next().is_some() {
        return false;
    }
    let Some(file_name) = file_name.to_str() else {
        return false;
    };
    let Some((stem, extension)) = file_name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && matches!(
            extension.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg"
        )
}

pub fn local_date() -> String {
    local_date_at(Utc::now())
}

pub fn local_date_at(now: DateTime<Utc>) -> String {
    local_date_at_in(now, &Local)
}

pub fn local_date_at_in<Tz>(now: DateTime<Utc>, timezone: &Tz) -> String
where
    Tz: TimeZone,
    Tz::Offset: std::fmt::Display,
{
    now.with_timezone(timezone).format("%Y-%m-%d").to_string()
}

