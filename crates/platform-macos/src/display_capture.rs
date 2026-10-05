use std::ptr::NonNull;
use std::sync::{mpsc, Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use block2::RcBlock;
use coosenpai_core::config::CaptureAllowlist;
use coosenpai_core::ports::{PortError, RuntimeLogger};
use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::AnyThread;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColor, CGColorSpace, CGContext,
    CGDirectDisplayID, CGDisplayBounds, CGDisplayPixelsHigh, CGDisplayPixelsWide, CGError,
    CGGetActiveDisplayList, CGImage, CGImageAlphaInfo, CGRectNull, CGWindowID, CGWindowImageOption,
    CGWindowListOption,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSInteger};
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
    allowlist: &CaptureAllowlist,
) -> Result<Vec<u8>> {
    let log = CaptureLog::new(
        logger,
        if matches!(target, CaptureTarget::Window(_)) {
            "application"
        } else {
            "fullscreen"
        },
    );
    let image = if allowlist.enabled {
        match target {
            CaptureTarget::Display(display_id) => {
                capture_allowed_display(display_id, allowlist, cancellation, log.clone())?
            }
            CaptureTarget::Window(_) => {
                anyhow::bail!("許可リスト付きのウィンドウ撮影経路がありません")
            }
        }
    } else {
        capture_target(target, cancellation, log.clone())?
    };
    let png = encode_watch_png(image, cancellation, &log)?;
    log.mask_succeeded();
    Ok(png)
}

pub(crate) fn capture_application_watch_pngs(
    window_ids: &[CGWindowID],
    expected_bundle_id: Option<&str>,
    cancellation: &CancellationToken,
    logger: Option<Arc<dyn RuntimeLogger>>,
) -> Result<Vec<Vec<u8>>> {
    if window_ids.is_empty() {
        return Ok(Vec::new());
    }
    let log = CaptureLog::new(logger, "application");
    let images =
        capture_application_targets(window_ids, expected_bundle_id, cancellation, log.clone())
            .map_err(CaptureError::into_error)?;
    let pngs = images
        .into_iter()
        .map(|image| encode_watch_png(image, cancellation, &log))
        .collect::<Result<Vec<_>>>()?;
    log.mask_succeeded();
    Ok(pngs)
}

#[derive(Clone)]
struct CaptureLog {
    logger: Option<Arc<dyn RuntimeLogger>>,
    target: &'static str,
    mask_counts: Arc<Mutex<Option<(usize, usize)>>>,
}

impl CaptureLog {
    fn new(logger: Option<Arc<dyn RuntimeLogger>>, target: &'static str) -> Self {
        Self {
            logger,
            target,
            mask_counts: Arc::new(Mutex::new(None)),
        }
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

    fn mask_succeeded(&self) {
        let counts = *self.mask_counts.lock().expect("capture mask counts");
        if let (Some(logger), Some((allowed, excluded))) = (&self.logger, counts) {
            let _ = logger.write(
                "INFO",
                &format!("撮影許可: allowed-windows={allowed} excluded-windows={excluded}"),
            );
        }
    }
}

fn capture_allowed_display(
    display_id: CGDirectDisplayID,
    allowlist: &CaptureAllowlist,
    cancellation: &CancellationToken,
    log: CaptureLog,
) -> Result<CFRetained<CGImage>> {
    anyhow::ensure!(
        !allowlist.apps.is_empty(),
        "撮影許可リストが空のため撮影を停止しました"
    );
    anyhow::ensure!(
        screenshot_manager_available(),
        "許可リスト撮影には macOS 14 以降が必要です"
    );
    check_cancellation(cancellation).map_err(CaptureError::into_error)?;
    let flight = acquire_capture_flight(&CAPTURE_FLIGHT, cancellation, SCK_CAPTURE_TIMEOUT)
        .map_err(CaptureError::into_error)?;
    let ids = allowlist
        .apps
        .iter()
        .map(|app| app.bundle_id.clone())
        .collect::<Vec<_>>();
    capture_with_retry(cancellation, || {
        capture_allowed_display_once(display_id, &ids, cancellation, flight.clone(), log.clone())
    })
    .map_err(CaptureError::into_error)
}

fn capture_allowed_display_once(
    display_id: CGDirectDisplayID,
    bundle_ids: &[String],
    cancellation: &CancellationToken,
    flight: Arc<OwnedSemaphorePermit>,
    log: CaptureLog,
) -> Result<CFRetained<CGImage>, CaptureError> {
    let (tx, rx) = mpsc::channel();
    let ids = bundle_ids.to_vec();
    let callback_cancellation = cancellation.clone();
    let content_started = Instant::now();
    let block = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let _flight = flight.clone();
            let result = start_allowed_display_capture(
                content,
                error,
                display_id,
                &ids,
                tx.clone(),
                &callback_cancellation,
                flight.clone(),
                log.clone(),
            );
            log.stage(
                "content",
                "allowed-window-list",
                content_started.elapsed(),
                result.is_ok(),
            );
            if let Err(error) = result {
                let _ = tx.send(Err(error));
            }
        },
    );
    // SAFETY: ScreenCaptureKit retains and calls the block with live objects.
    // Finder を許可してもデスクトップ面は撮影対象に含めない。
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(true, true, &block);
    }
    await_sck_reply(&rx, SCK_CAPTURE_TIMEOUT, cancellation)
}

#[allow(clippy::too_many_arguments)]
fn start_allowed_display_capture(
    content: *mut SCShareableContent,
    error: *mut NSError,
    display_id: CGDirectDisplayID,
    bundle_ids: &[String],
    tx: mpsc::Sender<SckReply>,
    cancellation: &CancellationToken,
    flight: Arc<OwnedSemaphorePermit>,
    log: CaptureLog,
) -> Result<(), CaptureError> {
    check_cancellation(cancellation)?;
    if let Some(error) = NonNull::new(error) {
        // SAFETY: NSError is live for this callback.
        return Err(CaptureError::from_nserror(unsafe { error.as_ref() }));
    }
    let content = NonNull::new(content).ok_or(CaptureError::NoResult)?;
    // SAFETY: the callback owns the shareable content for its duration.
    let content = unsafe { content.as_ref() };
    let display = unsafe { content.displays() }
        .iter()
        .find(|display| unsafe { display.displayID() } == display_id)
        .ok_or(CaptureError::InvalidMode)?;
    let display_frame = unsafe { display.frame() };
    let windows = unsafe { content.windows() };
    let mut allowed = Vec::new();
    let mut excluded = 0;
    for window in windows.iter() {
        if !rectangles_overlap(unsafe { window.frame() }, display_frame) {
            continue;
        }
        let owner = unsafe { window.owningApplication() };
        let permitted = owner.as_ref().is_some_and(|owner| {
            let bundle = unsafe { owner.bundleIdentifier() };
            allowed_window_owner(
                &bundle.to_string(),
                unsafe { owner.processID() },
                std::process::id() as i32,
                bundle_ids,
            )
        });
        if permitted {
            allowed.push(window);
        } else {
            excluded += 1;
        }
    }
    *log.mask_counts.lock().expect("capture mask counts") = Some((allowed.len(), excluded));
    if allowed.is_empty() {
        let image = opaque_black_image(
            CGDisplayPixelsWide(display_id),
            CGDisplayPixelsHigh(display_id),
        )?;
        let _ = tx.send(Ok(image));
        return Ok(());
    }
    let included = NSArray::from_retained_slice(&allowed);
    // SAFETY: the display and windows belong to the current shareable-content snapshot.
    let filter = unsafe {
        SCContentFilter::initWithDisplay_includingWindows(
            SCContentFilter::alloc(),
            &display,
            &included,
        )
    };
    let bounds = unsafe { filter.contentRect() };
    let scale = unsafe { filter.pointPixelScale() } as f64;
    let (width, height) = window_pixel_size(bounds, scale)?;
    let configuration = unsafe { SCStreamConfiguration::new() };
    let black = CGColor::new_generic_rgb(0.0, 0.0, 0.0, 1.0);
    unsafe {
        configuration.setWidth(width);
        configuration.setHeight(height);
        configuration.setShowsCursor(false);
        configuration.setBackgroundColor(&black);
    }
    let block =
        capture_completion_with_background(tx, flight, log, "allowlist-filter", Some(black));
    // SAFETY: live filter and configuration; ScreenCaptureKit retains the completion block.
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            &filter,
            &configuration,
            Some(&block),
        );
    }
    Ok(())
}

fn opaque_black_image(width: usize, height: usize) -> Result<CFRetained<CGImage>, CaptureError> {
    if width == 0 || height == 0 {
        return Err(CaptureError::InvalidMode);
    }
    let row_bytes = width.checked_mul(4).ok_or(CaptureError::InvalidMode)?;
    let length = row_bytes
        .checked_mul(height)
        .ok_or(CaptureError::InvalidMode)?;
    let mut pixels = vec![0_u8; length];
    for alpha in pixels[3..].iter_mut().step_by(4) {
        *alpha = 255;
    }
    let color_space = CGColorSpace::new_device_rgb().ok_or(CaptureError::NoResult)?;
    // SAFETY: the RGBA backing store remains live through image creation.
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
    CGBitmapContextCreateImage(Some(&context)).ok_or(CaptureError::NoResult)
}

fn allowed_window_owner(bundle_id: &str, pid: i32, own_pid: i32, bundle_ids: &[String]) -> bool {
    pid != own_pid && bundle_ids.iter().any(|id| id == bundle_id)
}

fn rectangles_overlap(left: CGRect, right: CGRect) -> bool {
    left.origin.x < right.origin.x + right.size.width
        && right.origin.x < left.origin.x + left.size.width
        && left.origin.y < right.origin.y + right.size.height
        && right.origin.y < left.origin.y + left.size.height
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

fn capture_application_targets(
    window_ids: &[CGWindowID],
    expected_bundle_id: Option<&str>,
    cancellation: &CancellationToken,
    log: CaptureLog,
) -> Result<Vec<CFRetained<CGImage>>, CaptureError> {
    check_cancellation(cancellation)?;
    let flight = acquire_capture_flight(&CAPTURE_FLIGHT, cancellation, SCK_CAPTURE_TIMEOUT)?;
    if screenshot_manager_available() {
        // SCShareableContent は集合ごとに一度だけ取得し、同じスナップショットを
        // 使って各ウインドウを直列に撮影する。撮影完了前に次の要求を発行しない。
        capture_with_retry(cancellation, || {
            capture_application_with_screenshot_kit(
                window_ids,
                expected_bundle_id,
                cancellation,
                flight.clone(),
                log.clone(),
            )
        })
    } else {
        check_capture_permission()?;
        window_ids
            .iter()
            .map(|window_id| {
                check_cancellation(cancellation)?;
                log.stage("content", "none-quartz", Duration::ZERO, true);
                let started = Instant::now();
                let result = capture_with_retry(cancellation, || {
                    capture_quartz_with_timeout(
                        CaptureTarget::Window(*window_id),
                        cancellation,
                        flight.clone(),
                    )
                });
                log.stage("capture", "quartz", started.elapsed(), result.is_ok());
                result
            })
            .collect()
    }
}

fn encode_watch_png(
    image: CFRetained<CGImage>,
    cancellation: &CancellationToken,
    log: &CaptureLog,
) -> Result<Vec<u8>> {
    check_cancellation(cancellation).map_err(CaptureError::into_error)?;
    let started = Instant::now();
    let result = cg_image_png(&image);
    log.stage("encode", "png", started.elapsed(), result.is_ok());
    result
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
    quartz_capture_permission(objc2_core_graphics::CGPreflightScreenCaptureAccess())
}

fn quartz_capture_permission(granted: bool) -> Result<(), CaptureError> {
    granted.then_some(()).ok_or(CaptureError::PermissionDenied)
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

pub(crate) fn screenshot_manager_available() -> bool {
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

fn capture_application_with_screenshot_kit(
    window_ids: &[CGWindowID],
    expected_bundle_id: Option<&str>,
    cancellation: &CancellationToken,
    flight: Arc<OwnedSemaphorePermit>,
    log: CaptureLog,
) -> Result<Vec<CFRetained<CGImage>>, CaptureError> {
    let content_started = Instant::now();
    let callback_cancellation = cancellation.clone();
    let callback_flight = flight.clone();
    let callback_window_ids = window_ids.to_owned();
    let callback_bundle_id = expected_bundle_id.map(str::to_owned);
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let _flight = callback_flight.clone();
            if let Err(error) = check_cancellation(&callback_cancellation) {
                log.stage("content", "window-list", content_started.elapsed(), false);
                let _ = tx.send(Err(error));
                return;
            }
            let requests = prepare_application_capture_requests(
                content,
                error,
                &callback_window_ids,
                callback_bundle_id.as_deref(),
            );
            log.stage(
                "content",
                "window-list",
                content_started.elapsed(),
                requests.is_ok(),
            );
            let (content, requests) = match requests {
                Ok(requests) => requests,
                Err(error) => {
                    let _ = tx.send(Err(error));
                    return;
                }
            };
            if callback_bundle_id.is_some() {
                let total = unsafe { content.windows() }.len();
                *log.mask_counts.lock().expect("capture mask counts") =
                    Some((requests.len(), total.saturating_sub(requests.len())));
            }
            // Keep the existing shared ownership contract for framework-retained callbacks.
            #[allow(clippy::arc_with_non_send_sync)]
            let state = Arc::new(Mutex::new(SckApplicationCaptureSequence {
                _content: content,
                requests,
                images: Vec::with_capacity(callback_window_ids.len()),
                next_index: 0,
                cancellation: callback_cancellation.clone(),
                flight: callback_flight.clone(),
                log: log.clone(),
                tx: tx.clone(),
                finished: false,
            }));
            start_next_application_capture(state);
        },
    );
    // SAFETY: the framework copies the block and invokes it exactly once on its own queue.
    // The raw pointers it passes are valid for the duration of the invocation.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(false, true, &block);
    }
    await_capture_reply(&rx, SCK_CAPTURE_TIMEOUT, cancellation)
}

struct SckApplicationCaptureRequest {
    filter: Retained<SCContentFilter>,
    configuration: Retained<SCStreamConfiguration>,
}

struct SckApplicationCaptureSequence {
    _content: Retained<SCShareableContent>,
    requests: Vec<SckApplicationCaptureRequest>,
    images: Vec<CFRetained<CGImage>>,
    next_index: usize,
    cancellation: CancellationToken,
    flight: Arc<OwnedSemaphorePermit>,
    log: CaptureLog,
    tx: mpsc::Sender<Result<Vec<CFRetained<CGImage>>, CaptureError>>,
    finished: bool,
}

fn prepare_application_capture_requests(
    content: *mut SCShareableContent,
    error: *mut NSError,
    window_ids: &[CGWindowID],
    expected_bundle_id: Option<&str>,
) -> Result<
    (
        Retained<SCShareableContent>,
        Vec<SckApplicationCaptureRequest>,
    ),
    CaptureError,
> {
    if let Some(error) = NonNull::new(error) {
        // SAFETY: the framework passes a valid NSError for the duration of the handler.
        return Err(CaptureError::from_nserror(unsafe { error.as_ref() }));
    }
    let content = NonNull::new(content).ok_or(CaptureError::NoResult)?;
    // SAFETY: the framework passes a valid SCShareableContent for the duration of the handler.
    let content_ref = unsafe { content.as_ref() };
    // SAFETY: retain the content before the completion handler returns.
    let retained_content =
        unsafe { Retained::retain(content.as_ptr()) }.ok_or(CaptureError::NoResult)?;
    let requests = window_ids
        .iter()
        .map(|window_id| {
            let (filter, pixel_width, pixel_height) =
                capture_filter(content_ref, *window_id, expected_bundle_id)?;
            // SAFETY: SCStreamConfiguration is a plain NSObject with no thread affinity.
            let configuration = unsafe { SCStreamConfiguration::new() };
            // SAFETY: setters on a live configuration object with in-range pixel dimensions.
            unsafe {
                configuration.setWidth(pixel_width);
                configuration.setHeight(pixel_height);
                configuration.setShowsCursor(false);
                configuration.setIgnoreShadowsSingleWindow(true);
            }
            Ok(SckApplicationCaptureRequest {
                filter,
                configuration,
            })
        })
        .collect::<Result<Vec<_>, CaptureError>>()?;
    Ok((retained_content, requests))
}

fn start_next_application_capture(state: Arc<Mutex<SckApplicationCaptureSequence>>) {
    let (filter, configuration, flight) = {
        let mut sequence = state.lock().expect("capture sequence");
        if sequence.finished {
            return;
        }
        let Some(request) = sequence.requests.get(sequence.next_index) else {
            sequence.finished = true;
            let reply = Ok(std::mem::take(&mut sequence.images));
            let _ = sequence.tx.send(reply);
            return;
        };
        let filter = request.filter.clone();
        let configuration = request.configuration.clone();
        sequence.next_index += 1;
        (filter, configuration, sequence.flight.clone())
    };
    let block = application_capture_completion(state, flight);
    // This function is called from the shareable-content or previous image completion handler,
    // so every SCScreenshotManager request stays on the framework's callback queue.
    // SAFETY: live filter/configuration; the framework copies and invokes the block once.
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
            &filter,
            &configuration,
            Some(&block),
        );
    }
}

fn application_capture_completion(
    state: Arc<Mutex<SckApplicationCaptureSequence>>,
    flight: Arc<OwnedSemaphorePermit>,
) -> RcBlock<dyn Fn(*mut CGImage, *mut NSError)> {
    let started = Instant::now();
    RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let reply = if let Some(image) = NonNull::new(image) {
            // SAFETY: the handler receives a valid CGImage; retain it before returning.
            Ok(unsafe { CFRetained::retain(image) })
        } else if let Some(error) = NonNull::new(error) {
            // SAFETY: the framework passes a valid NSError for the duration of the handler.
            Err(CaptureError::from_nserror(unsafe { error.as_ref() }))
        } else {
            Err(CaptureError::NoResult)
        };
        let _flight = flight.clone();
        let mut next = None;
        let mut final_reply = None;
        {
            let mut sequence = state.lock().expect("capture sequence");
            if sequence.finished {
                return;
            }
            match reply {
                Ok(image) if !sequence.cancellation.is_cancelled() => {
                    sequence.images.push(image);
                    sequence
                        .log
                        .stage("capture", "filter", started.elapsed(), true);
                    if sequence.next_index == sequence.requests.len() {
                        sequence.finished = true;
                        final_reply = Some(Ok(std::mem::take(&mut sequence.images)));
                    } else {
                        next = Some(state.clone());
                    }
                }
                Ok(_) => {
                    sequence.finished = true;
                    sequence
                        .log
                        .stage("capture", "filter", started.elapsed(), false);
                    final_reply = Some(Err(CaptureError::Cancelled));
                }
                Err(error) => {
                    sequence.finished = true;
                    sequence
                        .log
                        .stage("capture", "filter", started.elapsed(), false);
                    final_reply = Some(Err(error));
                }
            }
        }
        if let Some(reply) = final_reply {
            let _ = state.lock().expect("capture sequence").tx.send(reply);
        }
        if let Some(state) = next {
            start_next_application_capture(state);
        }
    })
}

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
    let (filter, pixel_width, pixel_height) = capture_filter(content, window_id, None)?;
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

fn await_capture_reply<T>(
    rx: &mpsc::Receiver<Result<T, CaptureError>>,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<T, CaptureError> {
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

fn capture_completion(
    tx: mpsc::Sender<SckReply>,
    flight: Arc<OwnedSemaphorePermit>,
    log: CaptureLog,
    mode: &'static str,
) -> RcBlock<dyn Fn(*mut CGImage, *mut NSError)> {
    capture_completion_with_background(tx, flight, log, mode, None)
}

fn capture_completion_with_background(
    tx: mpsc::Sender<SckReply>,
    flight: Arc<OwnedSemaphorePermit>,
    log: CaptureLog,
    mode: &'static str,
    background: Option<CFRetained<CGColor>>,
) -> RcBlock<dyn Fn(*mut CGImage, *mut NSError)> {
    let started = Instant::now();
    let flight = Mutex::new(Some(flight));
    RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let _background = &background;
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
    expected_bundle_id: Option<&str>,
) -> Result<(Retained<SCContentFilter>, usize, usize), CaptureError> {
    // SAFETY: windows are from the current shareable-content snapshot.
    let windows = unsafe { content.windows() };
    let window = windows
        .iter()
        .find(|window| unsafe { window.windowID() } == window_id)
        .ok_or(CaptureError::WindowNotFound)?;
    if expected_bundle_id.is_some_and(|expected| {
        unsafe { window.owningApplication() }.is_none_or(|owner| {
            unsafe { owner.bundleIdentifier() }.to_string() != expected
                || unsafe { owner.processID() } == std::process::id() as i32
        })
    }) {
        return Err(CaptureError::WindowNotFound);
    }
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
    await_capture_reply(rx, timeout, cancellation)
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
