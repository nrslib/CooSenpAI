use crate::state::AudioObservationSource;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum PortError {
    #[error("platform adapter の I/O に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("platform adapter が利用できません: {0}")]
    Unavailable(String),
    #[error("画面収録の権限がありません: {0}")]
    ScreenCapturePermission(String),
    #[error("platform adapter が timeout しました")]
    Timeout,
}

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub const SELECTED_TEXT_POLL_INTERVAL: Duration = Duration::from_millis(10);
pub const SELECTED_TEXT_POLL_TIMEOUT: Duration = Duration::from_millis(1000);

pub trait ClipboardReader: Send + Sync {
    fn read_text(&self) -> Result<Option<String>, PortError>;

    fn change_count(&self) -> Result<i64, PortError>;
}

pub trait ClipboardWriter: Send + Sync {
    fn write_text(&self, text: &str) -> Result<(), PortError>;

    fn clear(&self) -> Result<(), PortError>;
}

#[async_trait]
pub trait SelectedTextCopyPort: Send + Sync {
    async fn synthesize_copy(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<SelectedTextCopyOutcome, PortError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedTextCopyOutcome {
    Sent { change_count_before_post: i64 },
    PermissionDenied,
    Cancelled,
    ReleaseTimeout,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: f64,
}

pub trait BubbleDisplayPort: Send + Sync {
    fn cursor_point(&self) -> Result<ScreenPoint, PortError>;

    fn frontmost_window_point(&self) -> Result<Option<ScreenPoint>, PortError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[async_trait]
pub trait ScreenCapturePort: Send + Sync {
    async fn capture(
        &self,
        destination: &Path,
        cancellation: CancellationToken,
    ) -> Result<PathBuf, PortError>;
}

#[async_trait]
pub trait InteractiveCapturePort: Send + Sync {
    /// `true` は選択完了、`false` はユーザーによる取消を表す。
    async fn capture_interactive(
        &self,
        destination: &Path,
        cancellation: CancellationToken,
    ) -> Result<bool, PortError>;
}

pub trait HelperResolverPort: Send + Sync {
    fn resolve_ocr_helper(
        &self,
        executable_dir: &Path,
        product_root: &Path,
        configured: Option<&str>,
    ) -> Option<PathBuf>;

    fn resolve_provider_bridge(
        &self,
        executable_dir: &Path,
        product_root: &Path,
        resource_root: Option<&Path>,
    ) -> Option<PathBuf>;

    fn resolve_speech_helper(&self, executable_dir: &Path, product_root: &Path) -> Option<PathBuf>;

    fn resolve_hearing_helper(&self, executable_dir: &Path, product_root: &Path)
        -> Option<PathBuf>;

    fn resolve_node(&self, path_value: &str) -> Option<PathBuf>;
}

pub trait ProviderApiKeyStore: Send + Sync {
    fn read(&self, provider: crate::provider::ProviderName) -> Result<Option<String>, PortError>;

    fn write(
        &self,
        provider: crate::provider::ProviderName,
        api_key: &str,
    ) -> Result<(), PortError>;

    fn delete(&self, provider: crate::provider::ProviderName) -> Result<(), PortError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemSettingsPane {
    ScreenCapture,
    Accessibility,
    Microphone,
    SpeechRecognition,
}

#[async_trait]
pub trait SystemSettingsPort: Send + Sync {
    async fn open(
        &self,
        pane: SystemSettingsPane,
        cancellation: CancellationToken,
    ) -> Result<(), PortError>;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SpeechPermissionKind {
    NotDetermined,
    Granted,
    Denied,
    Restricted,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechPermissions {
    pub microphone: SpeechPermissionKind,
    pub recognition: SpeechPermissionKind,
}

impl Default for SpeechPermissions {
    fn default() -> Self {
        Self {
            microphone: SpeechPermissionKind::NotDetermined,
            recognition: SpeechPermissionKind::NotDetermined,
        }
    }
}

#[async_trait]
pub trait SpeechPermissionPort: Send + Sync {
    fn current(&self) -> Result<SpeechPermissions, PortError>;

    async fn request(
        &self,
        cancellation: CancellationToken,
    ) -> Result<SpeechPermissions, PortError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SpeechEvent {
    Ready {
        locale: String,
        microphone: SpeechPermissionKind,
        recognition: SpeechPermissionKind,
    },
    Partial {
        text: String,
    },
    Final {
        text: String,
    },
    Warning {
        kind: String,
        message: String,
    },
    Error {
        kind: String,
        message: String,
    },
    Closed,
}

pub enum SpeechCommand {
    Finish,
    Cancel {
        completed: oneshot::Sender<Result<(), PortError>>,
    },
}

#[derive(Clone)]
pub struct SpeechSessionControl {
    commands: mpsc::Sender<SpeechCommand>,
}

pub struct SpeechSession {
    control: SpeechSessionControl,
    events: mpsc::Receiver<Result<SpeechEvent, PortError>>,
}

impl SpeechSession {
    pub fn from_channels(
        commands: mpsc::Sender<SpeechCommand>,
        events: mpsc::Receiver<Result<SpeechEvent, PortError>>,
    ) -> Self {
        Self {
            control: SpeechSessionControl { commands },
            events,
        }
    }

    pub fn control(&self) -> SpeechSessionControl {
        self.control.clone()
    }

    pub async fn next_event(&mut self) -> Option<Result<SpeechEvent, PortError>> {
        self.events.recv().await
    }
}

impl SpeechSessionControl {
    pub async fn finish(&self) -> Result<(), PortError> {
        self.commands
            .send(SpeechCommand::Finish)
            .await
            .map_err(|_| PortError::Unavailable("音声入力は停止しています".to_owned()))
    }

    pub async fn cancel(&self) -> Result<(), PortError> {
        let (completed, result) = oneshot::channel();
        self.commands
            .send(SpeechCommand::Cancel { completed })
            .await
            .map_err(|_| PortError::Unavailable("音声入力は停止しています".to_owned()))?;
        result.await.map_err(|_| {
            PortError::Unavailable("音声入力の終了を確認できませんでした".to_owned())
        })?
    }
}

#[async_trait]
pub trait SpeechPort: Send + Sync {
    async fn start(
        &self,
        locale: &str,
        input_device: &str,
        cancellation: CancellationToken,
    ) -> Result<SpeechSession, PortError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "kebab-case", deny_unknown_fields)]
pub enum HearingEvent {
    Ready {
        locale: String,
        microphone: SpeechPermissionKind,
        recognition: SpeechPermissionKind,
    },
    Final {
        source: AudioObservationSource,
        text: String,
    },
    Warning {
        kind: String,
        message: String,
    },
    Error {
        kind: String,
        message: String,
    },
    Closed,
}

pub enum HearingCommand {
    Cancel {
        completed: oneshot::Sender<Result<(), PortError>>,
    },
}

#[derive(Clone)]
pub struct HearingSessionControl {
    commands: mpsc::Sender<HearingCommand>,
    cancel_requested: CancellationToken,
}

pub struct HearingSession {
    control: HearingSessionControl,
    events: mpsc::Receiver<Result<HearingEvent, PortError>>,
}

impl HearingSession {
    pub fn from_channels(
        commands: mpsc::Sender<HearingCommand>,
        events: mpsc::Receiver<Result<HearingEvent, PortError>>,
    ) -> Self {
        Self::from_channels_with_cancellation(commands, events, CancellationToken::new())
    }

    pub fn from_channels_with_cancellation(
        commands: mpsc::Sender<HearingCommand>,
        events: mpsc::Receiver<Result<HearingEvent, PortError>>,
        cancel_requested: CancellationToken,
    ) -> Self {
        Self {
            control: HearingSessionControl {
                commands,
                cancel_requested,
            },
            events,
        }
    }

    pub fn control(&self) -> HearingSessionControl {
        self.control.clone()
    }

    pub async fn next_event(&mut self) -> Option<Result<HearingEvent, PortError>> {
        self.events.recv().await
    }
}

impl HearingSessionControl {
    pub async fn cancel(&self) -> Result<(), PortError> {
        let (completed, result) = oneshot::channel();
        self.cancel_requested.cancel();
        self.commands
            .send(HearingCommand::Cancel { completed })
            .await
            .map_err(|_| PortError::Unavailable("聴覚観察は停止しています".to_owned()))?;
        result.await.map_err(|_| {
            PortError::Unavailable("聴覚観察の終了を確認できませんでした".to_owned())
        })?
    }
}

#[async_trait]
pub trait HearingPort: Send + Sync {
    async fn start(
        &self,
        locale: &str,
        input_device: &str,
        sources: Vec<AudioObservationSource>,
        debug_dump_dir: Option<&str>,
        cancellation: CancellationToken,
    ) -> Result<HearingSession, PortError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SpeechInputDevice {
    pub id: String,
    pub name: String,
}

pub trait SpeechInputDevicePort: Send + Sync {
    fn input_devices(&self) -> Result<Vec<SpeechInputDevice>, PortError>;
}

pub trait SpeechKeyStatePort: Send + Sync {
    fn primary_key_pressed(&self, shortcut: &str) -> Result<bool, PortError>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunningApplication {
    pub bundle_id: String,
    pub name: String,
    pub icon_png: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundApplication {
    pub process_id: i32,
    pub bundle_id: Option<String>,
}

pub trait ForegroundApplicationPort: Send + Sync {
    fn frontmost_application(&self) -> Result<Option<ForegroundApplication>, PortError>;
    fn activate_application(&self, application: &ForegroundApplication) -> Result<(), PortError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationCapture {
    pub path: PathBuf,
    pub window_id: u32,
}

#[async_trait]
pub trait ApplicationCapturePort: Send + Sync {
    fn running_applications(&self) -> Result<Vec<RunningApplication>, PortError>;
    async fn capture_application(
        &self,
        bundle_id: &str,
        destination: &Path,
        cancellation: CancellationToken,
    ) -> Result<Option<ApplicationCapture>, PortError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenCapturePermissionKind {
    NotDetermined,
    Granted,
    Denied,
    Restricted,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenCapturePermission {
    pub kind: ScreenCapturePermissionKind,
    pub requestable: bool,
    pub capture_verified: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenCapturePresentation {
    pub status: &'static str,
    pub message: Option<&'static str>,
}

impl ScreenCapturePermission {
    pub fn from_preflight(granted: bool, request_already_attempted: bool) -> Self {
        if granted {
            Self {
                kind: ScreenCapturePermissionKind::Granted,
                requestable: false,
                capture_verified: None,
            }
        } else {
            Self {
                kind: if request_already_attempted {
                    ScreenCapturePermissionKind::Denied
                } else {
                    ScreenCapturePermissionKind::NotDetermined
                },
                requestable: !request_already_attempted,
                capture_verified: None,
            }
        }
    }

    pub fn after_request(accepted: bool, preflight_granted: bool) -> Self {
        if !accepted {
            return Self {
                kind: ScreenCapturePermissionKind::Denied,
                requestable: false,
                capture_verified: None,
            };
        }
        Self {
            kind: ScreenCapturePermissionKind::Granted,
            requestable: false,
            capture_verified: (!preflight_granted).then_some(false),
        }
    }

    pub fn with_capture_result(mut self, succeeded: bool) -> Self {
        self.capture_verified = Some(succeeded);
        self
    }

    pub fn requires_restart(self) -> bool {
        self.kind == ScreenCapturePermissionKind::Granted && self.capture_verified == Some(false)
    }

    pub fn presentation(self) -> ScreenCapturePresentation {
        const ALLOW: &str =
            "システム設定の画面収録で CooSenpAI を許可して、アプリを再起動してください";
        match (self.kind, self.capture_verified) {
            (ScreenCapturePermissionKind::Granted, Some(false)) => ScreenCapturePresentation {
                status: "not-granted",
                message: Some("画面収録は許可済みですが、反映にはアプリの再起動が必要です"),
            },
            (ScreenCapturePermissionKind::Granted, _) => ScreenCapturePresentation {
                status: "granted",
                message: None,
            },
            (ScreenCapturePermissionKind::NotDetermined, _)
            | (ScreenCapturePermissionKind::Denied, _) => ScreenCapturePresentation {
                status: "not-granted",
                message: Some(ALLOW),
            },
            (ScreenCapturePermissionKind::Restricted, _) => ScreenCapturePresentation {
                status: "not-granted",
                message: Some("この Mac の制限により画面収録を利用できません"),
            },
            (ScreenCapturePermissionKind::Unavailable, _) => ScreenCapturePresentation {
                status: "unknown",
                message: Some("画面収録の権限状態を確認できません"),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OwnWindowBounds {
    pub captured_at: DateTime<Utc>,
    /// 画像と同じ Retina 物理座標系の矩形。
    pub bounds: Vec<WindowBounds>,
}

impl OwnWindowBounds {
    pub fn is_fresh_at(&self, now: DateTime<Utc>) -> bool {
        let age = now.signed_duration_since(self.captured_at);
        age >= chrono::Duration::zero() && age <= chrono::Duration::seconds(5)
    }
}

#[async_trait]
pub trait OwnWindowBoundsPort: Send + Sync {
    async fn read_own_window_bounds(&self) -> Result<OwnWindowBounds, PortError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivitySnapshot {
    pub idle_ms: u64,
    pub front_app: Option<String>,
    pub front_app_bundle_id: Option<String>,
}

#[async_trait]
pub trait ActivityPort: Send + Sync {
    async fn read_activity(&self) -> Result<ActivitySnapshot, PortError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrTextBlock {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub confidence: f32,
}

#[async_trait]
pub trait OcrPort: Send + Sync {
    async fn recognize(
        &self,
        path: &Path,
        level: &str,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> Result<Vec<OcrTextBlock>, PortError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerEvent {
    Sleep,
    Wake,
    Lock,
    Unlock,
}

#[async_trait]
pub trait PowerEventPort: Send + Sync {
    async fn next(&mut self) -> Result<Option<PowerEvent>, PortError>;
}

#[async_trait]
pub trait NotificationPort: Send + Sync {
    async fn show(
        &self,
        message: &str,
        priority: &str,
        duration: Duration,
    ) -> Result<(), PortError>;
}

pub trait RuntimeLogger: Send + Sync {
    fn write(&self, level: &str, message: &str) -> Result<(), std::io::Error>;
}

