use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use coosenpai_core::ports::{
    ApplicationCapture, CapturedScreen, RuntimeLogger, ScreenDisplay, WindowBounds,
};
use tokio_util::sync::CancellationToken;

use crate::display_capture::{
    active_screen_displays, capture_application_watch_pngs, capture_watch_png, CaptureTarget,
};

// 同期 Quartz API が停止しても、見守りは従来どおり 10 秒で打ち切る。
const WATCH_CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn capture_screen(
    destination_directory: PathBuf,
    cancellation: CancellationToken,
) -> Result<Vec<CapturedScreen>> {
    capture_screen_with_logger(destination_directory, cancellation, None).await
}

pub(crate) async fn capture_screen_with_logger(
    destination_directory: PathBuf,
    cancellation: CancellationToken,
    logger: Option<Arc<dyn RuntimeLogger>>,
) -> Result<Vec<CapturedScreen>> {
    let enumeration_logger = logger.clone();
    capture_screens_to_directory(
        destination_directory,
        cancellation,
        move || {
            let started = std::time::Instant::now();
            let result = active_screen_displays();
            if let Some(logger) = &enumeration_logger {
                let detail = result.as_ref().err().map(|error| format!(" detail={error:#}")).unwrap_or_default();
                let _ = logger.write("DEBUG", &format!(
                    "撮影: target=fullscreen stage=displays mode=core-graphics elapsed-ms={} success={} display-count={}{}",
                    started.elapsed().as_millis(), result.is_ok(), result.as_ref().map_or(0, Vec::len), detail));
            }
            result
        },
        move |display, cancellation| {
            capture_watch_png(
                CaptureTarget::Display(display.id),
                cancellation,
                logger.clone(),
            )
        },
    )
    .await
}

async fn capture_screens_to_directory(
    destination_directory: PathBuf,
    cancellation: CancellationToken,
    displays: impl Fn() -> Result<Vec<ScreenDisplay>>,
    capture: impl Fn(ScreenDisplay, &CancellationToken) -> Result<Vec<u8>> + Clone + Send + 'static,
) -> Result<Vec<CapturedScreen>> {
    let cancellation = cancellation.child_token();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    ensure_not_cancelled(&cancellation)?;
    let expected = displays()?;
    if expected.is_empty() {
        anyhow::bail!("撮影可能なディスプレイがありません");
    }
    std::fs::create_dir_all(&destination_directory)?;
    let staging = tempfile::tempdir_in(destination_directory)?;
    let work = async {
        let mut screens = Vec::with_capacity(expected.len());
        for display in &expected {
            let display = *display;
            let capture = capture.clone();
            let path = capture_png_to_file(
                staging.path().join(format!("display-{}.png", display.id)),
                cancellation.clone(),
                move |cancel| capture(display, cancel),
            )
            .await
            .with_context(|| format!("ディスプレイ {} の撮影に失敗しました", display.id))?;
            screens.push(CapturedScreen { display, path });
        }
        ensure_not_cancelled(&cancellation)?;
        if displays()? != expected {
            anyhow::bail!("撮影中にディスプレイ構成が変わりました");
        }
        Ok(screens)
    };
    let screens = tokio::time::timeout(WATCH_CAPTURE_TIMEOUT, work)
        .await
        .context("全ディスプレイの撮影がタイムアウトしました")??;
    ensure_not_cancelled(&cancellation)?;
    let _retained_directory = staging.keep();
    Ok(screens)
}

pub async fn capture_application_window(
    bundle_id: &str,
    destination_directory: PathBuf,
    window_limit: usize,
    cancellation: CancellationToken,
) -> Result<Vec<ApplicationCapture>> {
    capture_application_window_with_logger(
        bundle_id,
        destination_directory,
        window_limit,
        cancellation,
        None,
    )
    .await
}

pub(crate) async fn capture_application_window_with_logger(
    bundle_id: &str,
    destination_directory: PathBuf,
    window_limit: usize,
    cancellation: CancellationToken,
    logger: Option<Arc<dyn RuntimeLogger>>,
) -> Result<Vec<ApplicationCapture>> {
    ensure_not_cancelled(&cancellation)?;
    let windows = crate::window_info::application_capture_windows(bundle_id, window_limit)?;
    if windows.is_empty() {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(&destination_directory)?;
    let staging = tempfile::tempdir_in(&destination_directory)?;
    let expected_ids = windows
        .iter()
        .map(|window| window.window.id)
        .collect::<Vec<_>>();
    let work = async {
        let paths = capture_application_pngs_to_files(
            windows
                .iter()
                .map(|window| {
                    staging
                        .path()
                        .join(format!("window-{}.png", window.window.id))
                })
                .collect(),
            expected_ids.clone(),
            cancellation.clone(),
            logger.clone(),
        )
        .await
        .context("アプリウインドウの撮影セットに失敗しました")?;
        let captures = windows
            .iter()
            .zip(paths)
            .map(|(window, path)| ApplicationCapture {
                path,
                window_id: window.window.id,
                window_bounds: WindowBounds {
                    x: window.window.bounds.origin.x,
                    y: window.window.bounds.origin.y,
                    width: window.window.bounds.size.width,
                    height: window.window.bounds.size.height,
                },
                display: window.display,
            })
            .collect::<Vec<_>>();
        ensure_not_cancelled(&cancellation)?;
        let current_windows =
            crate::window_info::application_capture_windows(bundle_id, window_limit)?;
        if !application_windows_are_unchanged(&windows, &current_windows) {
            anyhow::bail!("撮影中にアプリウィンドウ構成が変わりました");
        }
        Ok(captures)
    };
    let captures = tokio::time::timeout(WATCH_CAPTURE_TIMEOUT, work)
        .await
        .context("アプリウィンドウの撮影がタイムアウトしました")??;
    ensure_not_cancelled(&cancellation)?;
    let _retained_directory = staging.keep();
    Ok(captures)
}

fn application_windows_are_unchanged(
    expected: &[crate::window_info::ApplicationCaptureWindow],
    current: &[crate::window_info::ApplicationCaptureWindow],
) -> bool {
    expected.len() == current.len()
        && expected.iter().zip(current).all(|(expected, current)| {
            expected.window.id == current.window.id
                && expected.window.bounds.origin.x == current.window.bounds.origin.x
                && expected.window.bounds.origin.y == current.window.bounds.origin.y
                && expected.window.bounds.size.width == current.window.bounds.size.width
                && expected.window.bounds.size.height == current.window.bounds.size.height
                && expected.display == current.display
        })
}

async fn capture_application_pngs_to_files(
    destinations: Vec<PathBuf>,
    window_ids: Vec<u32>,
    cancellation: CancellationToken,
    logger: Option<Arc<dyn RuntimeLogger>>,
) -> Result<Vec<PathBuf>> {
    anyhow::ensure!(
        destinations.len() == window_ids.len(),
        "撮影対象と保存先の数が一致しません"
    );
    let cancellation = cancellation.child_token();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    ensure_not_cancelled(&cancellation)?;
    let task_cancellation = cancellation.clone();
    let task = tokio::task::spawn_blocking(move || -> Result<Vec<PathBuf>> {
        let pngs = capture_application_watch_pngs(&window_ids, &task_cancellation, logger)?;
        anyhow::ensure!(
            pngs.len() == destinations.len(),
            "撮影画像と保存先の数が一致しません"
        );
        let mut paths = Vec::with_capacity(destinations.len());
        for (destination, png) in destinations.into_iter().zip(pngs) {
            ensure_not_cancelled(&task_cancellation)?;
            let parent = destination
                .parent()
                .context("capture destination に親がありません")?
                .to_owned();
            std::fs::create_dir_all(&parent)?;
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            file.write_all(&png)?;
            ensure_not_cancelled(&task_cancellation)?;
            file.persist(&destination).map_err(|error| error.error)?;
            paths.push(destination);
        }
        Ok(paths)
    });
    let paths = tokio::select! {
        biased;
        () = cancellation.cancelled() => anyhow::bail!("画面の撮影が取り消されました"),
        result = tokio::time::timeout(WATCH_CAPTURE_TIMEOUT, task) => {
            result.context("画面の撮影がタイムアウトしました")?
                .context("画面撮影タスクを完了できませんでした")??
        }
    };
    ensure_not_cancelled(&cancellation)?;
    Ok(paths)
}

pub(crate) async fn capture_png_to_file(
    destination: PathBuf,
    cancellation: CancellationToken,
    capture: impl FnOnce(&CancellationToken) -> Result<Vec<u8>> + Send + 'static,
) -> Result<PathBuf> {
    let cancellation = cancellation.child_token();
    // 呼出元 future の破棄でも blocking task が遅れて画像を保存しないようにする。
    let _cancel_on_drop = cancellation.clone().drop_guard();
    ensure_not_cancelled(&cancellation)?;
    let parent = destination
        .parent()
        .context("capture destination に親がありません")?
        .to_owned();
    let task_cancellation = cancellation.clone();
    let task = tokio::task::spawn_blocking(move || -> Result<tempfile::NamedTempFile> {
        ensure_not_cancelled(&task_cancellation)?;
        let png = capture(&task_cancellation)?;
        ensure_not_cancelled(&task_cancellation)?;
        std::fs::create_dir_all(&parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&png)?;
        Ok(file)
    });
    let file = tokio::select! {
        biased;
        () = cancellation.cancelled() => anyhow::bail!("画面の撮影が取り消されました"),
        result = tokio::time::timeout(WATCH_CAPTURE_TIMEOUT, task) => {
            result.context("画面の撮影がタイムアウトしました")?
                .context("画面撮影タスクを完了できませんでした")??
        }
    };
    ensure_not_cancelled(&cancellation)?;
    file.persist(&destination).map_err(|error| error.error)?;
    Ok(destination)
}

fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<()> {
    if cancellation.is_cancelled() {
        anyhow::bail!("画面の撮影が取り消されました");
    }
    Ok(())
}

