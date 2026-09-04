use crate::{
    activate_application, capture_application_window, capture_interactive_region, capture_screen,
    frontmost_application, frontmost_application_identity, read_front_app_name,
    read_front_application, read_hid_idle_ms, recognize_text_with_helper, running_applications,
};
use async_trait::async_trait;
use coosenpai_core::ports::{
    ActivityPort, ActivitySnapshot, ApplicationCapture, ApplicationCapturePort,
    ForegroundApplication, ForegroundApplicationPort, HelperResolverPort, InteractiveCapturePort,
    NotificationPort, OcrPort, OcrTextBlock, OwnWindowBounds, OwnWindowBoundsPort, PortError,
    PowerEvent, PowerEventPort, RunningApplication, ScreenCapturePermission,
    ScreenCapturePermissionKind, ScreenCapturePort, SystemSettingsPane, SystemSettingsPort,
    WindowBounds,
};
use coosenpai_core::process::{ProcessRequest, ProcessRunner, TokioProcessRunner};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSNotificationCenter, NSNotificationName,
    NSObjectProtocol, NSOperationQueue, NSString,
};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::RwLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Default)]
pub struct MacForegroundApplications;

impl ForegroundApplicationPort for MacForegroundApplications {
    fn frontmost_application(&self) -> Result<Option<ForegroundApplication>, PortError> {
        Ok(frontmost_application_identity())
    }

    fn activate_application(&self, application: &ForegroundApplication) -> Result<(), PortError> {
        activate_application(application).map_err(|error| PortError::Unavailable(error.to_string()))
    }
}

#[derive(Debug, Clone, Default)]
pub struct MacOwnWindowBounds {
    captured: Option<(chrono::DateTime<chrono::Utc>, Vec<WindowBounds>)>,
}

impl MacOwnWindowBounds {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_physical(bounds: Vec<WindowBounds>) -> Self {
        Self::from_physical_at(chrono::Utc::now(), bounds)
    }

    pub fn from_physical_at(
        captured_at: chrono::DateTime<chrono::Utc>,
        bounds: Vec<WindowBounds>,
    ) -> Self {
        Self {
            captured: Some((captured_at, bounds)),
        }
    }

    /// Tauri が取得した論理座標の矩形を、capture/OCR と同じ物理座標へ固定する。
    pub fn from_logical_at(
        captured_at: chrono::DateTime<chrono::Utc>,
        logical_bounds: &[WindowBounds],
        display_scale: f64,
    ) -> Result<Self, PortError> {
        let physical = Self::to_physical(captured_at, logical_bounds, display_scale)?;
        Ok(Self::from_physical_at(captured_at, physical.bounds))
    }

    pub fn to_physical(
        captured_at: chrono::DateTime<chrono::Utc>,
        logical_bounds: &[WindowBounds],
        display_scale: f64,
    ) -> Result<OwnWindowBounds, PortError> {
        if !display_scale.is_finite() || display_scale <= 0.0 {
            return Err(PortError::Unavailable("Retina 倍率が不正です".to_owned()));
        }
        let bounds = logical_bounds
            .iter()
            .map(|bound| {
                if !bound.x.is_finite()
                    || !bound.y.is_finite()
                    || !bound.width.is_finite()
                    || !bound.height.is_finite()
                    || bound.width <= 0.0
                    || bound.height <= 0.0
                {
                    return Err(PortError::Unavailable(
                        "自ウィンドウの矩形が不正です".to_owned(),
                    ));
                }
                Ok(WindowBounds {
                    x: (bound.x * display_scale).round(),
                    y: (bound.y * display_scale).round(),
                    width: (bound.width * display_scale).round().max(1.0),
                    height: (bound.height * display_scale).round().max(1.0),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(OwnWindowBounds {
            captured_at,
            bounds,
        })
    }
}

#[async_trait]
impl OwnWindowBoundsPort for MacOwnWindowBounds {
    async fn read_own_window_bounds(&self) -> Result<OwnWindowBounds, PortError> {
        match &self.captured {
            Some((captured_at, bounds)) => Ok(OwnWindowBounds {
                captured_at: *captured_at,
                bounds: bounds.clone(),
            }),
            None => Ok(OwnWindowBounds {
                captured_at: chrono::Utc::now(),
                bounds: Vec::new(),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MacScreenCapture;

#[async_trait]
impl ScreenCapturePort for MacScreenCapture {
    async fn capture(
        &self,
        destination: &Path,
        cancellation: CancellationToken,
    ) -> Result<PathBuf, PortError> {
        capture_screen(destination.to_owned(), cancellation)
            .await
            .map_err(|error| {
                map_screen_capture_error(&error.to_string(), crate::screen_capture_permission())
            })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MacSystemSettings;

#[async_trait]
impl SystemSettingsPort for MacSystemSettings {
    async fn open(
        &self,
        pane: SystemSettingsPane,
        cancellation: CancellationToken,
    ) -> Result<(), PortError> {
        let section = match pane {
            SystemSettingsPane::ScreenCapture => "Privacy_ScreenCapture",
            SystemSettingsPane::Accessibility => "Privacy_Accessibility",
            SystemSettingsPane::Microphone => "Privacy_Microphone",
            SystemSettingsPane::SpeechRecognition => "Privacy_SpeechRecognition",
        };
        let output = TokioProcessRunner
            .run(
                ProcessRequest {
                    executable: PathBuf::from("/usr/bin/open"),
                    args: vec![format!(
                        "x-apple.systempreferences:com.apple.preference.security?{section}"
                    )],
                    env: Vec::new(),
                    cwd: None,
                    stdin: Vec::new(),
                    timeout: Duration::from_secs(5),
                },
                cancellation,
            )
            .await
            .map_err(|error| PortError::Unavailable(error.to_string()))?;
        if output.status == Some(0) {
            Ok(())
        } else {
            Err(PortError::Unavailable(
                "システム設定を開けませんでした".to_owned(),
            ))
        }
    }
}

pub async fn open_external_url(url: &str) -> Result<(), PortError> {
    let output = TokioProcessRunner
        .run(
            ProcessRequest {
                executable: PathBuf::from("/usr/bin/open"),
                args: vec![url.to_owned()],
                env: Vec::new(),
                cwd: None,
                stdin: Vec::new(),
                timeout: Duration::from_secs(5),
            },
            CancellationToken::new(),
        )
        .await
        .map_err(|error| PortError::Unavailable(error.to_string()))?;
    if output.status == Some(0) {
        Ok(())
    } else {
        Err(PortError::Unavailable(
            "外部リンクを開けませんでした".to_owned(),
        ))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MacInteractiveCapture;

#[async_trait]
impl InteractiveCapturePort for MacInteractiveCapture {
    async fn capture_interactive(
        &self,
        destination: &Path,
        cancellation: CancellationToken,
    ) -> Result<bool, PortError> {
        capture_interactive_region(destination.to_owned(), cancellation)
            .await
            .map_err(|error| {
                map_screen_capture_error(&error.to_string(), crate::screen_capture_permission())
            })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MacApplicationCapture;

#[derive(Debug, Clone, Copy, Default)]
pub struct MacHelperResolver;

impl HelperResolverPort for MacHelperResolver {
    fn resolve_ocr_helper(
        &self,
        executable_dir: &Path,
        product_root: &Path,
        configured: Option<&str>,
    ) -> Option<PathBuf> {
        [
            Some(executable_dir.join("coosenpai-ocr")),
            Some(product_root.join("helpers/coosenpai-ocr")),
            configured.map(PathBuf::from),
        ]
        .into_iter()
        .flatten()
        .find(|path| is_executable(path))
    }

    fn resolve_provider_bridge(
        &self,
        executable_dir: &Path,
        product_root: &Path,
        resource_root: Option<&Path>,
    ) -> Option<PathBuf> {
        [
            resource_root.map(|root| root.join("provider-bridge/bridge.js")),
            resource_root.map(|root| root.join("bridge.js")),
            Some(executable_dir.join("provider-bridge.js")),
            Some(product_root.join("helpers/provider-bridge.js")),
        ]
        .into_iter()
        .flatten()
        .find(|path| path.is_file())
    }

    fn resolve_speech_helper(&self, executable_dir: &Path, product_root: &Path) -> Option<PathBuf> {
        [
            executable_dir.join("coosenpai-speech"),
            product_root.join("helpers/coosenpai-speech"),
        ]
        .into_iter()
        .find(|path| is_executable(path))
    }

    fn resolve_hearing_helper(
        &self,
        executable_dir: &Path,
        product_root: &Path,
    ) -> Option<PathBuf> {
        [
            executable_dir.join("coosenpai-hearing"),
            product_root.join("helpers/coosenpai-hearing"),
        ]
        .into_iter()
        .find(|path| is_executable(path))
    }

    fn resolve_node(&self, path_value: &str) -> Option<PathBuf> {
        std::env::split_paths(path_value)
            .filter(|directory| directory.is_absolute())
            .map(|directory| directory.join("node"))
            .find(|path| is_executable(path))
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

impl MacApplicationCapture {
    pub fn running() -> Result<Vec<RunningApplication>, PortError> {
        running_applications().map_err(|error| PortError::Unavailable(error.to_string()))
    }
}

#[async_trait]
impl ApplicationCapturePort for MacApplicationCapture {
    fn running_applications(&self) -> Result<Vec<RunningApplication>, PortError> {
        running_applications().map_err(|error| PortError::Unavailable(error.to_string()))
    }

    async fn capture_application(
        &self,
        bundle_id: &str,
        destination: &Path,
        cancellation: CancellationToken,
    ) -> Result<Option<ApplicationCapture>, PortError> {
        capture_application_window(bundle_id, destination.to_owned(), cancellation)
            .await
            .map_err(|error| {
                map_screen_capture_error(&error.to_string(), crate::screen_capture_permission())
            })
    }
}

fn map_screen_capture_error(error: &str, permission: ScreenCapturePermission) -> PortError {
    if permission.kind == ScreenCapturePermissionKind::Granted {
        PortError::Unavailable(format!("画面キャプチャに失敗しました: {error}"))
    } else {
        PortError::ScreenCapturePermission(
            permission
                .with_capture_result(false)
                .presentation()
                .message
                .unwrap_or("画面収録の権限がありません")
                .to_owned(),
        )
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MacActivity;

#[async_trait]
impl ActivityPort for MacActivity {
    async fn read_activity(&self) -> Result<ActivitySnapshot, PortError> {
        let idle_ms = read_hid_idle_ms()
            .await
            .map_err(|_| PortError::Unavailable("HID の入力時刻を取得できません".to_owned()))?;
        if !idle_ms.is_finite() || idle_ms < 0.0 || idle_ms > u64::MAX as f64 {
            return Err(PortError::Unavailable(
                "HID の入力時刻が不正です".to_owned(),
            ));
        }
        let frontmost = read_front_application()
            .await
            .ok()
            .or_else(frontmost_application);
        Ok(ActivitySnapshot {
            idle_ms: idle_ms.round() as u64,
            front_app: match &frontmost {
                Some(application) => Some(application.name.clone()),
                None => read_front_app_name().await.ok(),
            },
            front_app_bundle_id: frontmost.map(|application| application.bundle_id),
        })
    }
}

#[derive(Debug, Default)]
pub struct MacOcr {
    helper: RwLock<Option<PathBuf>>,
}

impl MacOcr {
    pub fn new(helper: Option<PathBuf>) -> Self {
        Self {
            helper: RwLock::new(helper),
        }
    }

    pub fn set_helper(&self, helper: Option<PathBuf>) -> Result<(), PortError> {
        *self.helper.write().map_err(|_| {
            PortError::Unavailable("OCR helper の状態を更新できません".to_owned())
        })? = helper;
        Ok(())
    }
}

#[async_trait]
impl OcrPort for MacOcr {
    async fn recognize(
        &self,
        path: &Path,
        level: &str,
        timeout: Duration,
        cancellation: CancellationToken,
    ) -> Result<Vec<OcrTextBlock>, PortError> {
        let helper = self
            .helper
            .read()
            .map_err(|_| PortError::Unavailable("OCR helper の状態を読み取れません".to_owned()))?
            .clone()
            .ok_or_else(|| PortError::Unavailable("OCR helper が見つかりません".to_owned()))?;
        let mut results =
            recognize_text_with_helper(&helper, &[path.to_owned()], level, timeout, cancellation)
                .await
                .map_err(|_| PortError::Unavailable("OCR helper に失敗しました".to_owned()))?;
        let blocks = results
            .pop()
            .map(|result| result.blocks)
            .ok_or_else(|| PortError::Unavailable("OCR 結果がありません".to_owned()))?;
        Ok(blocks
            .into_iter()
            .map(|block| OcrTextBlock {
                text: block.text,
                x: block.x,
                y: block.y,
                width: block.width,
                height: block.height,
                confidence: block.confidence,
            })
            .collect())
    }
}

pub struct MacPowerEvents {
    receiver: mpsc::Receiver<PowerEvent>,
    workspace_center: Retained<NSNotificationCenter>,
    distributed_center: Retained<NSDistributedNotificationCenter>,
    workspace_tokens: Vec<usize>,
    distributed_tokens: Vec<usize>,
}

impl MacPowerEvents {
    pub fn new() -> Result<Self, PortError> {
        let (sender, receiver) = mpsc::channel(16);
        let workspace_center = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: these are immutable notification-name constants supplied by AppKit and are
        // valid for the lifetime of the notification center.
        let names = unsafe {
            [
                (NSWorkspaceWillSleepNotification, PowerEvent::Sleep),
                (NSWorkspaceDidWakeNotification, PowerEvent::Wake),
            ]
        };
        let workspace_tokens = names
            .into_iter()
            .map(|(name, event)| {
                register_workspace_notification(&workspace_center, name, event, sender.clone())
            })
            .collect();

        let distributed_center = NSDistributedNotificationCenter::defaultCenter();
        let distributed_tokens = [
            ("com.apple.screenIsLocked", PowerEvent::Lock),
            ("com.apple.screenIsUnlocked", PowerEvent::Unlock),
        ]
        .into_iter()
        .map(|(name, event)| {
            let name = NSString::from_str(name);
            register_distributed_notification(&distributed_center, &name, event, sender.clone())
        })
        .collect();
        Ok(Self {
            receiver,
            workspace_center,
            distributed_center,
            workspace_tokens,
            distributed_tokens,
        })
    }
}

#[async_trait]
impl PowerEventPort for MacPowerEvents {
    async fn next(&mut self) -> Result<Option<PowerEvent>, PortError> {
        Ok(self.receiver.recv().await)
    }
}

fn register_workspace_notification(
    center: &NSNotificationCenter,
    name: &'static NSNotificationName,
    event: PowerEvent,
    sender: mpsc::Sender<PowerEvent>,
) -> usize {
    let block = block2::RcBlock::new(move |_notification: NonNull<NSNotification>| {
        let _ = sender.try_send(event);
    });
    // SAFETY: name and block signature match NSNotificationCenter's API. The callback only
    // performs a bounded, non-blocking channel send.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
    };
    Retained::into_raw(token) as usize
}

fn register_distributed_notification(
    center: &NSDistributedNotificationCenter,
    name: &NSNotificationName,
    event: PowerEvent,
    sender: mpsc::Sender<PowerEvent>,
) -> usize {
    let main_queue = NSOperationQueue::mainQueue();
    let block = block2::RcBlock::new(move |_notification: NonNull<NSNotification>| {
        let _ = sender.try_send(event);
    });
    // SAFETY: name and block signature match NSNotificationCenter's inherited API. The callback
    // does not dereference the notification and only sends a bounded event.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(name),
            None,
            Some(&main_queue),
            &block,
        )
    };
    Retained::into_raw(token) as usize
}

impl Drop for MacPowerEvents {
    fn drop(&mut self) {
        for token in &self.workspace_tokens {
            release_observer(&self.workspace_center, *token);
        }
        for token in &self.distributed_tokens {
            release_observer(&self.distributed_center, *token);
        }
    }
}

fn release_observer(center: &NSNotificationCenter, address: usize) {
    let pointer = address as *mut ProtocolObject<dyn NSObjectProtocol>;
    // SAFETY: the address came from Retained::into_raw at registration, is kept exactly once,
    // and is removed and reconstructed exactly once during Drop.
    unsafe {
        center.removeObserver(&*pointer.cast::<AnyObject>());
        drop(Retained::from_raw(pointer));
    }
}

#[derive(Debug, Clone)]
pub struct MacNotifier {
    title: String,
}

impl MacNotifier {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
        }
    }
}

impl Default for MacNotifier {
    fn default() -> Self {
        Self::new("CooSenpAI")
    }
}

#[async_trait]
impl NotificationPort for MacNotifier {
    async fn show(
        &self,
        message: &str,
        _priority: &str,
        duration: Duration,
    ) -> Result<(), PortError> {
        if message.is_empty() {
            return Err(PortError::Unavailable("通知本文が空です".to_owned()));
        }
        let script = r#"on run argv
display notification (item 1 of argv) with title (item 2 of argv)
end run"#;
        let output = TokioProcessRunner
            .run(
                ProcessRequest {
                    executable: PathBuf::from("/usr/bin/osascript"),
                    args: vec![
                        "-e".to_owned(),
                        script.to_owned(),
                        "--".to_owned(),
                        message.to_owned(),
                        self.title.clone(),
                    ],
                    env: Vec::new(),
                    cwd: None,
                    stdin: Vec::new(),
                    timeout: duration.max(Duration::from_secs(1)),
                },
                CancellationToken::new(),
            )
            .await
            .map_err(|_| PortError::Unavailable("OS 通知に失敗しました".to_owned()))?;
        if output.status == Some(0) {
            Ok(())
        } else {
            Err(PortError::Unavailable("OS 通知に失敗しました".to_owned()))
        }
    }
}

