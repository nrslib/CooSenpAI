use crate::commands::{authorize_window, CommandOrigin, IpcResult, TauriIpcResult};
use crate::connectome::ConnectomeStatus;
use crate::connectome_download::{DownloadTask, DownloadView};
use crate::state::DesktopState;
use coosenpai_core::config::{BundledConnectome, Config};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{State, WebviewWindow};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

fn download_progress_view(
    received: u64,
    total: u64,
    last_reported_percent: &AtomicU64,
) -> Option<DownloadView> {
    if received == total {
        return Some(DownloadView::verifying());
    }
    let percent = (u128::from(received) * 100 / u128::from(total)) as u64;
    (last_reported_percent.fetch_max(percent, Ordering::Relaxed) < percent)
        .then(|| DownloadView::downloading(received, total))
}

#[async_trait::async_trait]
pub(crate) trait ConnectomeCommandHost: Send + Sync + 'static {
    fn runtime_config(&self) -> Config;
    async fn connectome_status(&self, config: Config) -> Result<ConnectomeStatus, &'static str>;
    async fn refresh_connectome(&self, force_full_hash: bool) -> Result<(), String>;
    async fn publish_status(&self, config_revision: u64, status: ConnectomeStatus);
    fn publish_download(&self, view: DownloadView);
    fn download_task(&self) -> &Mutex<Option<DownloadTask>>;
    fn cancellation(&self) -> &CancellationToken;
    fn resources(&self) -> Option<PathBuf>;
    async fn download(
        &self,
        resources: &Path,
        cancel: &CancellationToken,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), &'static str>;
}

#[async_trait::async_trait]
impl ConnectomeCommandHost for DesktopState {
    fn runtime_config(&self) -> Config {
        DesktopState::runtime_config(self)
    }

    async fn connectome_status(&self, config: Config) -> Result<ConnectomeStatus, &'static str> {
        self.factory.prepare_connectome_resolution(config).await
    }

    async fn refresh_connectome(&self, force_full_hash: bool) -> Result<(), String> {
        DesktopState::refresh_connectome(self, force_full_hash).await
    }

    async fn publish_status(&self, config_revision: u64, status: ConnectomeStatus) {
        self.publish_event(crate::snapshot_presenter::SnapshotEvent::ConnectomeStatus {
            config_revision,
            status,
        })
        .await;
    }

    fn publish_download(&self, view: DownloadView) {
        self.publish_connectome_download(view);
    }

    fn download_task(&self) -> &Mutex<Option<DownloadTask>> {
        &self.connectome_download
    }

    fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    fn resources(&self) -> Option<PathBuf> {
        self.factory.connectome_resources().map(Path::to_path_buf)
    }

    async fn download(
        &self,
        resources: &Path,
        cancel: &CancellationToken,
        progress: &(dyn Fn(u64, u64) + Send + Sync),
    ) -> Result<(), &'static str> {
        crate::connectome_download::download(&self.paths, resources, cancel, progress).await
    }
}

pub(crate) fn download_needed(status: &ConnectomeStatus) -> Result<bool, &'static str> {
    match (status.state.as_str(), status.reason.as_deref()) {
        ("ready", None) => Ok(false),
        (_, Some("pack-missing")) => Ok(true),
        _ => Err("配線データの取得は現在の設定では不要です"),
    }
}

#[tauri::command]
pub async fn connectome_pack_open_directory(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let directory = match crate::connectome::prepare_pack_directory(&state.paths) {
        Ok(directory) => directory,
        Err(_) => return Ok(IpcResult::failure("pack の保存先を作成できません")),
    };
    Ok(match crate::platform::open_file(&directory).await {
        Ok(()) => IpcResult::success(()),
        Err(_) => IpcResult::failure("pack の保存先を開けません"),
    })
}

#[tauri::command]
pub async fn connectome_pack_copy_path(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    let path = crate::connectome::pack_directory(&state.paths);
    Ok(
        match state.clipboard_writer.write_text(&path.to_string_lossy()) {
            Ok(()) => IpcResult::success(()),
            Err(_) => IpcResult::failure("pack の保存先をコピーできません"),
        },
    )
}

#[tauri::command]
pub async fn connectome_pack_download<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    connectome_pack_download_handler(window, Arc::clone(&state)).await
}

pub(crate) async fn connectome_pack_download_handler<
    R: tauri::Runtime,
    H: ConnectomeCommandHost,
>(
    window: WebviewWindow<R>,
    state: Arc<H>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(download_pack(state).await)
}

pub(crate) async fn download_pack<H: ConnectomeCommandHost>(state: Arc<H>) -> IpcResult<()> {
    let config = state.runtime_config();
    let status = match state.connectome_status(config.clone()).await {
        Ok(status) => status,
        Err(_) => return IpcResult::failure("配線データを確認できません"),
    };
    if status.reason.as_deref() != Some("pack-missing") {
        return IpcResult::failure("配線データの取得は現在の設定では不要です");
    }
    let mut running = state.download_task().lock().await;
    if state.cancellation().is_cancelled() {
        return IpcResult::failure("アプリを終了中です");
    }
    if running
        .as_ref()
        .is_some_and(|task| !task.join.is_finished())
    {
        return IpcResult::failure("配線データを取得中です");
    }
    *running = None;
    if state.refresh_connectome(true).await.is_err() {
        return IpcResult::failure("配線データを再確認できません");
    }
    let config = state.runtime_config();
    let status = match state.connectome_status(config).await {
        Ok(status) => status,
        Err(_) => return IpcResult::failure("配線データを確認できません"),
    };
    match download_needed(&status) {
        Ok(false) => {
            state.publish_download(DownloadView::default());
            return IpcResult::success(());
        }
        Err(reason) => return IpcResult::failure(reason),
        Ok(true) => {}
    }
    let Some(resources) = state.resources() else {
        return IpcResult::failure("学習済みファイルが見つかりません");
    };
    let cancel = state.cancellation().child_token();
    state.publish_download(DownloadView::downloading(
        0,
        crate::connectome_download::ARCHIVE_BYTES,
    ));
    let state = Arc::clone(&state);
    let task_cancel = cancel.clone();
    let join = tokio::spawn(async move {
        let last_reported_percent = AtomicU64::new(0);
        let result = state
            .download(&resources, &cancel, &|received, total| {
                if let Some(view) = download_progress_view(received, total, &last_reported_percent)
                {
                    state.publish_download(view);
                }
            })
            .await;
        let result = match result {
            Ok(()) => state
                .refresh_connectome(false)
                .await
                .map_err(|_| "runtime-refresh-failed"),
            Err("pack-already-exists") => match state.refresh_connectome(true).await {
                Ok(()) => Err("pack-already-exists"),
                Err(_) => Err("runtime-refresh-failed"),
            },
            Err(reason) => Err(reason),
        };
        state.publish_download(match result {
            Ok(()) => DownloadView::complete(),
            Err("cancelled") => DownloadView::default(),
            Err(reason) => DownloadView::failed(reason),
        });
    });
    *running = Some(DownloadTask {
        cancel: task_cancel,
        join,
    });
    IpcResult::success(())
}

#[tauri::command]
pub async fn connectome_pack_cancel_download(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    if let Some(task) = state.connectome_download.lock().await.as_ref() {
        task.cancel.cancel();
    }
    Ok(IpcResult::success(()))
}

#[tauri::command]
pub async fn connectome_pack_recheck<R: tauri::Runtime>(
    window: WebviewWindow<R>,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<()> {
    connectome_pack_recheck_handler(window, Arc::clone(&state)).await
}

pub(crate) async fn connectome_pack_recheck_handler<R: tauri::Runtime, H: ConnectomeCommandHost>(
    window: WebviewWindow<R>,
    state: Arc<H>,
) -> TauriIpcResult<()> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(recheck_pack(state).await)
}

pub(crate) async fn recheck_pack<H: ConnectomeCommandHost>(state: Arc<H>) -> IpcResult<()> {
    let config = state.runtime_config();
    if config.judge.bundled_connectome != BundledConnectome::On || !config.judge.modules.is_empty()
    {
        return IpcResult::failure("同梱の判断役が有効ではありません");
    }
    if state
        .download_task()
        .lock()
        .await
        .as_ref()
        .is_some_and(|task| !task.join.is_finished())
    {
        return IpcResult::failure("配線データを取得中です");
    }
    let mut checking = match state.connectome_status(config.clone()).await {
        Ok(status) => status,
        Err(_) => return IpcResult::failure("配線データを再確認できません"),
    };
    checking.state = "checking".to_owned();
    checking.reason = Some("pack-verifying".to_owned());
    state
        .publish_status(config.revision, checking.clone())
        .await;
    match state.refresh_connectome(true).await {
        Ok(()) => IpcResult::success(()),
        Err(_) => {
            let mut failed = checking;
            failed.state = "unavailable".to_owned();
            failed.reason = Some("runtime-refresh-failed".to_owned());
            state.publish_status(config.revision, failed).await;
            IpcResult::failure("配線データを再確認できません")
        }
    }
}
