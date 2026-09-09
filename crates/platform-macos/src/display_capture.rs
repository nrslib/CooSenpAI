use std::ptr::NonNull;
use std::sync::{mpsc, Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use block2::RcBlock;
use coosenpai_core::ports::{PortError, RuntimeLogger};
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGDirectDisplayID, CGDisplayBounds, CGError,
    CGGetActiveDisplayList, CGImage, CGImageAlphaInfo, CGRectNull, CGWindowID, CGWindowImageOption,
    CGWindowListOption,
};
use objc2_foundation::{NSDictionary, NSError, NSInteger};
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
    SCStreamErrorCode, SCStreamErrorDomain,
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

const SCK_CAPTURE_TIMEOUT: Duration = Duration::from_secs(6);
const CAPTURE_POLL_INTERVAL: Duration = Duration::from_millis(20);
// 切替直後の約 1 秒の失敗を避ける。OS が失敗を返した場合だけ 1 回待って再試行する。
const CAPTURE_RETRY_DELAY: Duration = Duration::from_millis(1200);
pub(crate) static CAPTURE_FLIGHT: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(1)));

#[derive(Clone, Copy)]
pub(crate) enum CaptureTarget {
    Display(CGDirectDisplayID),
    Window(CGWindowID),
}

pub(crate) fn active_screen_displays() -> Result<Vec<coosenpai_core::ports::ScreenDisplay>> {
    active_screen_displays_with(
        |ids, count| {
            let pointer = if ids.is_empty() {
                std::ptr::null_mut()
            } else {
                ids.as_mut_ptr()
            };
            // SAFETY: the buffer length is passed as capacity, count is a live out-pointer.
            unsafe { CGGetActiveDisplayList(ids.len() as u32, pointer, count) }
        },
        |id| CGDisplayBounds(id),
    )
}

fn active_screen_displays_with(
    mut query: impl FnMut(&mut [CGDirectDisplayID], &mut u32) -> CGError,
    bounds_for: impl Fn(CGDirectDisplayID) -> CGRect,
) -> Result<Vec<coosenpai_core::ports::ScreenDisplay>> {
    let mut count = 0;
    let error = query(&mut [], &mut count);
    if error != CGError::Success || count == 0 {
        anyhow::bail!(
            "撮影可能なディスプレイ一覧を取得できません: count-query error={error:?} count={count}"
        );
    }
    let mut ids = vec![0; count as usize];
    let error = query(&mut ids, &mut count);
    if error != CGError::Success || count == 0 || count as usize > ids.len() {
        anyhow::bail!(
            "撮影可能なディスプレイ一覧を取得できません: list-query error={error:?} count={count}"
        );
    }
    ids.truncate(count as usize);
    ids.sort_unstable();
    anyhow::ensure!(
        ids.iter().all(|id| *id != 0) && ids.windows(2).all(|pair| pair[0] != pair[1]),
        "ディスプレイ ID が不正または重複しています"
    );
    ids.into_iter()
        .map(|id| {
            let bounds = bounds_for(id);
            if !bounds.origin.x.is_finite()
                || !bounds.origin.y.is_finite()
                || !bounds.size.width.is_finite()
                || !bounds.size.height.is_finite()
                || bounds.size.width <= 0.0
                || bounds.size.height <= 0.0
            {
                anyhow::bail!("ディスプレイ {id} の寸法が不正です");
            }
            Ok(coosenpai_core::ports::ScreenDisplay {
                id,
                bounds: coosenpai_core::ports::WindowBounds {
                    x: bounds.origin.x,
                    y: bounds.origin.y,
                    width: bounds.size.width,
                    height: bounds.size.height,
                },
            })
        })
        .collect()
}

pub(crate) fn capture_watch_png(
    target: CaptureTarget,
    cancellation: &CancellationToken,
    logger: Option<Arc<dyn RuntimeLogger>>,
) -> Result<Vec<u8>> {
    let log = CaptureLog::new(
        logger,
        if matches!(target, CaptureTarget::Window(_)) {
            "application"
        } else {
            "fullscreen"
        },
    );
    let image = capture_target(target, cancellation, log.clone())?;
    check_cancellation(cancellation).map_err(CaptureError::into_error)?;
    let started = Instant::now();
    let result = cg_image_png(&image);
    log.stage("encode", "png", started.elapsed(), result.is_ok());
    result
}

#[derive(Clone)]
struct CaptureLog {
    logger: Option<Arc<dyn RuntimeLogger>>,
    target: &'static str,
}

impl CaptureLog {
    fn new(logger: Option<Arc<dyn RuntimeLogger>>, target: &'static str) -> Self {
        Self { logger, target }
    }

    fn stage(&self, stage: &str, mode: &str, elapsed: Duration, success: bool) {
        if let Some(logger) = &self.logger {
            let _ = logger.write(
                "DEBUG",
                &format!(
                    "撮影: target={} stage={stage} mode={mode} elapsed-ms={} success={success}",
                    self.target,
                    elapsed.as_millis(),
                ),
            );
        }
    }
}

fn capture_target(
    target: CaptureTarget,
    cancellation: &CancellationToken,
    log: CaptureLog,
) -> Result<CFRetained<CGImage>> {
    check_cancellation(cancellation).map_err(CaptureError::into_error)?;
    let flight = acquire_capture_flight(&CAPTURE_FLIGHT, cancellation, SCK_CAPTURE_TIMEOUT)
        .map_err(CaptureError::into_error)?;
    capture_with_retry(cancellation, || {
        capture_backend(
            target,
            screenshot_manager_available(),
            |window_id| {
                capture_once_with_screenshot_kit(
                    window_id,
                    cancellation.clone(),
                    flight.clone(),
                    log.clone(),
                )
            },
            |target| {
                check_capture_permission()?;
                log.stage("content", "none-quartz", Duration::ZERO, true);
                let started = Instant::now();
                let result = capture_quartz_with_timeout(target, cancellation, flight.clone());
                log.stage("capture", "quartz", started.elapsed(), result.is_ok());
                result
            },
        )
    })
    .map_err(CaptureError::into_error)
}

fn capture_with_retry<T>(
    cancellation: &CancellationToken,
    mut capture: impl FnMut() -> Result<T, CaptureError>,
) -> Result<T, CaptureError> {
    let mut attempts = 0;
    loop {
        check_cancellation(cancellation)?;
        let result = capture();
        attempts += 1;
        check_cancellation(cancellation)?;
        match result {
            Ok(image) => return Ok(image),
            Err(error) if attempts == 1 && error.is_transient() => {
                wait_for_retry(cancellation, CAPTURE_RETRY_DELAY)?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn wait_for_retry(cancellation: &CancellationToken, delay: Duration) -> Result<(), CaptureError> {
    let deadline = Instant::now() + delay;
    loop {
        check_cancellation(cancellation)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        std::thread::sleep(remaining.min(CAPTURE_POLL_INTERVAL));
    }
}

pub(crate) fn acquire_capture_flight(
    gate: &Arc<Semaphore>,
    cancellation: &CancellationToken,
    timeout: Duration,
) -> Result<Arc<OwnedSemaphorePermit>, CaptureError> {
    let deadline = Instant::now() + timeout;
    loop {
        check_cancellation(cancellation)?;
        if let Ok(permit) = gate.clone().try_acquire_owned() {
            return Ok(Arc::new(permit));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(CaptureError::Timeout);
        }
        std::thread::sleep(remaining.min(CAPTURE_POLL_INTERVAL));
    }
}

fn check_capture_permission() -> Result<(), CaptureError> {
    if objc2_core_graphics::CGPreflightScreenCaptureAccess() {
        Ok(())
    } else {
        Err(CaptureError::PermissionDenied)
    }
}

fn check_cancellation(cancellation: &CancellationToken) -> Result<(), CaptureError> {
    if cancellation.is_cancelled() {
        Err(CaptureError::Cancelled)
    } else {
        Ok(())
    }
}

fn capture_backend<T>(
    target: CaptureTarget,
    screenshot_available: bool,
    sck: impl FnOnce(CGWindowID) -> Result<T, CaptureError>,
    quartz: impl FnOnce(CaptureTarget) -> Result<T, CaptureError>,
) -> Result<T, CaptureError> {
    match target {
        CaptureTarget::Window(id) if screenshot_available => sck(id),
        _ => quartz(target),
    }
}

fn screenshot_manager_available() -> bool {
    AnyClass::get(c"SCScreenshotManager").is_some()
}

// 全画面は全対応 OS で Quartz を使う。アプリ単体も SCScreenshotManager がない
// macOS 13 では Quartz で対象ウィンドウを取得する。
#[allow(deprecated)]
fn capture_with_quartz(target: CaptureTarget) -> Result<CFRetained<CGImage>, CaptureError> {
    let image = match target {
        CaptureTarget::Display(display_id) => objc2_core_graphics::CGDisplayCreateImage(display_id),
        CaptureTarget::Window(window_id) => {
            // SAFETY: CGRectNull is an immutable framework constant.
            objc2_core_graphics::CGWindowListCreateImage(
                unsafe { CGRectNull },
                CGWindowListOption::OptionIncludingWindow,
                window_id,
                CGWindowImageOption::BoundsIgnoreFraming | CGWindowImageOption::BestResolution,
            )
        }
    };
    let image = image.ok_or(CaptureError::QuartzUnavailable)?;
    if matches!(target, CaptureTarget::Window(_)) && image_is_fully_transparent(&image)? {
        return Err(CaptureError::WindowNotFound);
    }
    Ok(image)
}

fn capture_quartz_with_timeout(
    target: CaptureTarget,
    cancellation: &CancellationToken,
    flight: Arc<OwnedSemaphorePermit>,
) -> Result<CFRetained<CGImage>, CaptureError> {
    capture_sync_with_timeout(cancellation, flight, SCK_CAPTURE_TIMEOUT, move || {
        capture_with_quartz(target)
    })
}

pub(crate) fn capture_sync_with_timeout(
    cancellation: &CancellationToken,
    flight: Arc<OwnedSemaphorePermit>,
    timeout: Duration,
    capture: impl FnOnce() -> SckReply + Send + 'static,
) -> SckReply {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Quartz cannot be interrupted. Retain the flight until the native call returns,
        // even if its caller has already timed out or cancelled.
        let _flight = flight;
        let _ = tx.send(capture());
    });
    await_sck_reply(&rx, timeout, cancellation)
}

fn image_is_fully_transparent(image: &CGImage) -> Result<bool, CaptureError> {
    let width = CGImage::width(Some(image));
    let height = CGImage::height(Some(image));
    let row_bytes = width.checked_mul(4).ok_or(CaptureError::InvalidMode)?;
    let length = row_bytes
        .checked_mul(height)
        .ok_or(CaptureError::InvalidMode)?;
    if length == 0 {
        return Ok(true);
    }
    let mut pixels = vec![0u8; length];
    let color_space = CGColorSpace::new_device_rgb().ok_or(CaptureError::NoResult)?;
    // SAFETY: the RGBA backing store spans every row and stays alive until context drop.
    let context = unsafe {
        CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            width,
            height,
            8,
            row_bytes,
            Some(&color_space),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }
    .ok_or(CaptureError::NoResult)?;
    CGContext::draw_image(
        Some(&context),
        CGRect::new(CGPoint::ZERO, CGSize::new(width as f64, height as f64)),
        Some(image),
    );
    drop(context);
    Ok(pixels.as_chunks::<4>().0.iter().all(|pixel| pixel[3] == 0))
}

#[derive(Debug)]
pub(crate) struct WindowUnavailable;

impl std::fmt::Display for WindowUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("撮影対象のアプリウィンドウが見つかりません")
    }
}

impl std::error::Error for WindowUnavailable {}

type SckReply = Result<CFRetained<CGImage>, CaptureError>;

fn capture_once_with_screenshot_kit(
    window_id: CGWindowID,
    cancellation: CancellationToken,
    flight: Arc<OwnedSemaphorePermit>,
    log: CaptureLog,
) -> Result<CFRetained<CGImage>, CaptureError> {
    let deadline = Instant::now() + SCK_CAPTURE_TIMEOUT;
    let content_started = Instant::now();
    let callback_cancellation = cancellation.clone();
    let flight = Mutex::new(Some(flight));
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let flight = flight
                .lock()
                .expect("capture flight")
                .take()
                .expect("completion called once");
            log.stage(
                "content",
                "window-list",
                content_started.elapsed(),
                error.is_null() && !content.is_null(),
            );
            if let Err(error) = start_capture(
                content,
                error,
                window_id,
                tx.clone(),
                &callback_cancellation,
                deadline,
                flight,
                log.clone(),
            ) {
                let _ = tx.send(Err(error));
            }
        },
    );
    // SAFETY: the framework copies the block and invokes it exactly once on its own queue.
    // The raw pointers it passes are valid for the duration of the invocation.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(false, true, &block);
    }
    await_sck_reply(&rx, SCK_CAPTURE_TIMEOUT, &cancellation)
}

// SCK の completion handler から受ける生ポインタ 2 つと、撮影 1 回分の文脈をそのまま渡す。
// 引数をまとめる構造体を足すより、handler と同じ形で読める方を優先している。
#[allow(clippy::too_many_arguments)]
fn start_capture(
    content: *mut SCShareableContent,
    error: *mut NSError,
    window_id: CGWindowID,
    tx: mpsc::Sender<SckReply>,
    cancellation: &CancellationToken,
    deadline: Instant,
    flight: Arc<OwnedSemaphorePermit>,
    log: CaptureLog,
) -> Result<(), CaptureError> {
    check_cancellation(cancellation)?;
    if Instant::now() >= deadline {
        return Err(CaptureError::Timeout);
    }
    if let Some(error) = NonNull::new(error) {
        // SAFETY: the framework passes a valid NSError for the duration of the handler.
        return Err(CaptureError::from_nserror(unsafe { error.as_ref() }));
    }
    let content = NonNull::new(content).ok_or(CaptureError::NoResult)?;
    // SAFETY: the framework passes a valid SCShareableContent for the duration of the handler.
    let content = unsafe { content.as_ref() };
    let (filter, pixel_width, pixel_height) = capture_filter(content, window_id)?;
    // SAFETY: SCStreamConfiguration is a plain NSObject with no thread affinity.
    let configuration = unsafe { SCStreamConfiguration::new() };
    // SAFETY: setters on a live configuration object with in-range pixel dimensions.
    unsafe {
        configuration.setWidth(pixel_width);
        configuration.setHeight(pixel_height);
        configuration.setShowsCursor(false);
        configuration.setIgnoreShadowsSingleWindow(true);
    }
    let block = capture_completion(tx, flight, log, "filter");
    // SAFETY: live filter/configuration; the framework copies and invokes the block once.
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            &filter,
            &configuration,
            Some(&block),
        );
    }
    Ok(())
}

fn capture_completion(
    tx: mpsc::Sender<SckReply>,
    flight: Arc<OwnedSemaphorePermit>,
    log: CaptureLog,
    mode: &'static str,
) -> RcBlock<dyn Fn(*mut CGImage, *mut NSError)> {
    let started = Instant::now();
    let flight = Mutex::new(Some(flight));
    RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        // タイムアウトや取消後も OS の完了まで保持し、block の保管期間には依存しない。
        let _flight = flight.lock().expect("capture flight").take();
        let reply = if let Some(image) = NonNull::new(image) {
            // SAFETY: the handler receives a valid CGImage; we retain it before the
            // framework releases its reference when the handler returns.
            Ok(unsafe { CFRetained::retain(image) })
        } else if let Some(error) = NonNull::new(error) {
            // SAFETY: the framework passes a valid NSError for the duration of the handler.
            Err(CaptureError::from_nserror(unsafe { error.as_ref() }))
        } else {
            Err(CaptureError::NoResult)
        };
        log.stage("capture", mode, started.elapsed(), reply.is_ok());
        let _ = tx.send(reply);
    })
}

fn capture_filter(
    content: &SCShareableContent,
    window_id: CGWindowID,
) -> Result<(Retained<SCContentFilter>, usize, usize), CaptureError> {
    // SAFETY: windows are from the current shareable-content snapshot.
    let windows = unsafe { content.windows() };
    let window = windows
        .iter()
        .find(|window| unsafe { window.windowID() } == window_id)
        .ok_or(CaptureError::WindowNotFound)?;
    let filter = unsafe {
        SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window)
    };
    // These properties are available since macOS 14, as is SCScreenshotManager.
    let bounds = unsafe { filter.contentRect() };
    let scale = unsafe { filter.pointPixelScale() } as f64;
    let (width, height) = window_pixel_size(bounds, scale)?;
    Ok((filter, width, height))
}

fn window_pixel_size(bounds: CGRect, scale: f64) -> Result<(usize, usize), CaptureError> {
    let width = (bounds.size.width * scale).round();
    let height = (bounds.size.height * scale).round();
    if !scale.is_finite()
        || scale <= 0.0
        || !width.is_finite()
        || !height.is_finite()
        || width < 1.0
        || height < 1.0
        || width >= usize::MAX as f64
        || height >= usize::MAX as f64
    {
        return Err(CaptureError::InvalidMode);
    }
    Ok((width as usize, height as usize))
}

fn await_sck_reply(
    rx: &mpsc::Receiver<SckReply>,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<CFRetained<CGImage>, CaptureError> {
    let deadline = Instant::now() + timeout;
    loop {
        check_cancellation(cancellation)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining.min(CAPTURE_POLL_INTERVAL)) {
            Ok(reply) => return reply,
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                return Err(CaptureError::Timeout)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(CaptureError::Disconnected),
        }
    }
}

fn sc_stream_error_domain() -> String {
    // SAFETY: framework constant initialized by dyld before use and lives for the process.
    unsafe { SCStreamErrorDomain }.to_string()
}

fn is_permission_denied(domain: &str, code: NSInteger) -> bool {
    domain == sc_stream_error_domain() && code == SCStreamErrorCode::UserDeclined.0
}

#[derive(Debug)]
pub(crate) enum CaptureError {
    Failure { domain: String, code: NSInteger },
    WindowNotFound,
    QuartzUnavailable,
    Cancelled,
    PermissionDenied,
    InvalidMode,
    NoResult,
    Timeout,
    Disconnected,
}

impl CaptureError {
    fn is_transient(&self) -> bool {
        match self {
            Self::Failure { domain, code } => {
                domain == &sc_stream_error_domain()
                    && (*code == SCStreamErrorCode::FailedToStart.0
                        || *code == SCStreamErrorCode::InternalError.0)
            }
            Self::QuartzUnavailable => true,
            _ => false,
        }
    }

    fn from_nserror(error: &NSError) -> Self {
        Self::Failure {
            domain: error.domain().to_string(),
            code: error.code(),
        }
    }

    fn into_error(self) -> anyhow::Error {
        match self {
            Self::Failure { domain, code } if is_permission_denied(&domain, code) => {
                PortError::ScreenCapturePermission(
                    "ScreenCaptureKit が画面収録へのアクセスを拒否しました".to_owned(),
                )
                .into()
            }
            Self::Failure { domain, code } => {
                anyhow::anyhow!("ScreenCaptureKit の撮影に失敗しました: {domain} code={code}")
            }

            Self::WindowNotFound => WindowUnavailable.into(),

            Self::QuartzUnavailable => anyhow::anyhow!("画面を撮影できませんでした"),
            Self::Cancelled => anyhow::anyhow!("画面の撮影が取り消されました"),
            Self::PermissionDenied => PortError::ScreenCapturePermission(
                "画面収録へのアクセスが許可されていません".to_owned(),
            )
            .into(),
            Self::InvalidMode => {
                anyhow::anyhow!("ディスプレイの物理ピクセル寸法を取得できません")
            }
            Self::NoResult => {
                anyhow::anyhow!("ScreenCaptureKit から画像もエラーも返されませんでした")
            }
            Self::Timeout => anyhow::anyhow!("画面の撮影がタイムアウトしました"),
            Self::Disconnected => {
                anyhow::anyhow!("撮影の完了ハンドラから結果を取得できませんでした")
            }
        }
    }
}

fn cg_image_png(image: &CGImage) -> Result<Vec<u8>> {
    let bitmap = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), image);
    let properties = NSDictionary::new();
    // SAFETY: an empty property dictionary is valid for PNG encoding and its key/value
    // generic parameters match AppKit's declared contract.
    let png = unsafe {
        bitmap.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties)
    }
    .context("画面画像を PNG へ変換できませんでした")?;
    Ok(png.to_vec())
}

