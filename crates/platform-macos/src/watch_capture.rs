use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use coosenpai_core::ports::{ApplicationCapture, CapturedScreen, RuntimeLogger, ScreenDisplay};
use tokio_util::sync::CancellationToken;

use crate::display_capture::{active_screen_displays, capture_watch_png, CaptureTarget};

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
    destination: PathBuf,
    cancellation: CancellationToken,
) -> Result<Option<ApplicationCapture>> {
    capture_application_window_with_logger(bundle_id, destination, cancellation, None).await
}

pub(crate) async fn capture_application_window_with_logger(
    bundle_id: &str,
    destination: PathBuf,
    cancellation: CancellationToken,
    logger: Option<Arc<dyn RuntimeLogger>>,
) -> Result<Option<ApplicationCapture>> {
    ensure_not_cancelled(&cancellation)?;
    let Some(window) = crate::window_info::application_window(bundle_id)? else {
        return Ok(None);
    };
    let result = capture_png_to_file(destination, cancellation, move |cancellation| {
        capture_watch_png(CaptureTarget::Window(window.id), cancellation, logger)
    })
    .await;
    let path = match result {
        Ok(path) => path,
        Err(error) if error.is::<crate::display_capture::WindowUnavailable>() => return Ok(None),
        Err(error) => return Err(error),
    };
    Ok(Some(ApplicationCapture {
        path,
        window_id: window.id,
    }))
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

