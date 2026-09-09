use crate::commands::{authorize_window, CommandOrigin, IpcResult, TauriIpcResult};
use crate::state::DesktopState;
use crate::update_install::InstallLocation;
use crate::update_transport::{PendingUpdate, UpdateClient};
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::RuntimeLogger;
use semver::Version;
use serde::Serialize;
use std::sync::Arc;
use tauri::{Manager, State, WebviewWindow};
use tokio::sync::{watch, Mutex};

pub(crate) enum CheckOrigin {
    Background,
    Manual,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub(crate) enum UpdateStatus {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        notes: Option<String>,
    },
    Downloading {
        version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Installing {
        version: String,
    },
    Installed {
        version: String,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct UpdateSnapshot {
    pub(crate) revision: u64,
    pub(crate) status: UpdateStatus,
}

pub(crate) struct AppUpdater {
    operation: Mutex<()>,
    pending: Mutex<Option<PendingUpdate>>,
    snapshot: watch::Sender<UpdateSnapshot>,
    installation: Arc<Mutex<()>>,
}

impl Default for AppUpdater {
    fn default() -> Self {
        Self {
            operation: Mutex::new(()),
            pending: Mutex::new(None),
            snapshot: watch::channel(UpdateSnapshot::default()).0,
            installation: Arc::new(Mutex::new(())),
        }
    }
}

impl AppUpdater {
    fn publish(&self, state: &DesktopState, status: UpdateStatus) {
        self.snapshot.send_modify(|snapshot| {
            snapshot.revision += 1;
            snapshot.status = status;
        });
        state.ui.input(
            crate::ui_events::UiView::Application,
            crate::ui_events::UiEvent::ChatProjection(crate::ui_events::ChatProjection::Update(
                self.snapshot.borrow().clone(),
            )),
        );
    }

    fn failed(&self, state: &DesktopState, message: String) -> String {
        let _ = state.logger.write("WARN", &message);
        self.publish(
            state,
            UpdateStatus::Failed {
                message: message.clone(),
            },
        );
        message
    }

    pub(crate) async fn check(
        &self,
        state: &DesktopState,
        origin: CheckOrigin,
    ) -> Result<Option<String>, String> {
        let locale = current_locale(state);
        ensure_enabled(state, locale)?;
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| text(TextKey::UpdateOperationInProgress, locale).to_owned())?;
        if matches!(
            self.snapshot.borrow().status,
            UpdateStatus::Installed { .. }
        ) {
            return Ok(None);
        }
        let previous_status = self.snapshot.borrow().status.clone();
        self.publish(state, UpdateStatus::Checking);
        let result = async {
            let updater = update_client(&state.app, locale)?;
            tokio::select! {
                _ = state.cancellation.cancelled() => Err(text(TextKey::UpdateShuttingDown, locale).to_owned()),
                result = updater.check() => result.map_err(|error| error.to_string()),
            }
        }
        .await;
        match result {
            Ok(Some(update)) => {
                let version = update.version.to_string();
                let status = UpdateStatus::Available {
                    version: version.clone(),
                    notes: update.notes.clone(),
                };
                *self.pending.lock().await = Some(update);
                self.publish(state, status);
                Ok(Some(version))
            }
            Ok(None) => {
                self.pending.lock().await.take();
                self.publish(state, UpdateStatus::UpToDate);
                Ok(None)
            }
            Err(error) => {
                let message = with_error(TextKey::UpdateCheckFailed, locale, &error);
                match origin {
                    CheckOrigin::Manual => Err(self.failed(state, message)),
                    CheckOrigin::Background => {
                        self.publish(state, previous_status);
                        Err(message)
                    }
                }
            }
        }
    }

    async fn install(&self, state: &DesktopState) -> Result<(), String> {
        let locale = current_locale(state);
        ensure_enabled(state, locale)?;
        if cfg!(debug_assertions) {
            return Err(text(TextKey::UpdateDebugBuild, locale).to_owned());
        }
        let location = InstallLocation::for_running_app_for_locale(
            &std::env::current_exe().map_err(|error| error.to_string())?,
            &state
                .app
                .path()
                .home_dir()
                .map_err(|error| error.to_string())?,
            locale,
        )?;
        let client = update_client(&state.app, locale)?;
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| text(TextKey::UpdateOperationInProgress, locale).to_owned())?;
        let update = self
            .pending
            .lock()
            .await
            .take()
            .ok_or_else(|| text(TextKey::UpdatePendingMissing, locale).to_owned())?;
        let version = update.version.to_string();
        self.publish(
            state,
            UpdateStatus::Downloading {
                version: version.clone(),
                downloaded: 0,
                total: None,
            },
        );
        let download = client.download(&update, |downloaded, total| {
            self.publish(
                state,
                UpdateStatus::Downloading {
                    version: version.clone(),
                    downloaded,
                    total,
                },
            );
        });
        let bytes = tokio::select! {
            _ = state.cancellation.cancelled() => Err(text(TextKey::UpdateShuttingDown, locale).to_owned()),
            result = download => result.map_err(|error| error.to_string()),
        }
        .map_err(|error| {
            self.failed(state, with_error(TextKey::UpdateDownloadVerifyFailed, locale, &error))
        })?;

        let installation = self.installation.clone().lock_owned().await;
        if state.cancellation.is_cancelled() {
            return Err(self.failed(state, text(TextKey::UpdateShuttingDown, locale).to_owned()));
        }
        self.publish(
            state,
            UpdateStatus::Installing {
                version: version.clone(),
            },
        );
        let cancellation = state.cancellation.clone();
        let current = Version::parse(env!("CARGO_PKG_VERSION")).expect("package version");
        let result = tokio::task::spawn_blocking(move || {
            let _installation = installation;
            let staged = crate::update_archive::prepare_for_locale(
                &bytes,
                location.parent(),
                &update.version,
                Some(&current),
                std::env::consts::ARCH,
                locale,
            )?;
            if cancellation.is_cancelled() {
                return Err(text(TextKey::UpdateShuttingDown, locale).to_owned());
            }
            location
                .swap(&staged)
                .map_err(|error| with_error(TextKey::UpdatePreviousPreserved, locale, &error))?;
            // 交換は完了しているため、旧版の掃除に失敗しても適用済みとして扱う。
            Ok::<_, String>(staged.close().err())
        })
        .await;
        match result {
            Ok(Ok(cleanup_error)) => {
                if let Some(error) = cleanup_error {
                    let _ = state.logger.write(
                        "WARN",
                        &format!("更新は適用済みですが旧版の掃除に失敗しました: {error}"),
                    );
                }
                self.publish(state, UpdateStatus::Installed { version });
                Ok(())
            }
            Ok(Err(error)) => Err(self.failed(
                state,
                with_error(TextKey::UpdateApplyFailed, locale, &error),
            )),
            Err(error) => Err(self.failed(
                state,
                with_error(TextKey::UpdateProcessFailed, locale, &error),
            )),
        }
    }

    pub(crate) async fn wait_for_installation(&self) {
        let _installation = self.installation.lock().await;
    }
}

fn update_client(app: &tauri::AppHandle, locale: Locale) -> Result<UpdateClient, String> {
    let config = app
        .config()
        .plugins
        .0
        .get("updater")
        .ok_or_else(|| text(TextKey::UpdateConfigMissing, locale).to_owned())?;
    UpdateClient::new_for_locale(
        config,
        app.package_info().version.clone(),
        std::env::consts::ARCH,
        locale,
    )
}

fn ensure_enabled(state: &DesktopState, locale: Locale) -> Result<(), String> {
    if state.cancellation.is_cancelled() {
        return Err(text(TextKey::UpdateShuttingDown, locale).to_owned());
    }
    if !state.runtime_config().app.check_for_updates {
        return Err(text(TextKey::UpdateDisabled, locale).to_owned());
    }
    Ok(())
}

fn current_locale(state: &DesktopState) -> Locale {
    Locale::from_config(&state.runtime_config().ui.language)
}

fn with_error(key: TextKey, locale: Locale, error: &impl std::fmt::Display) -> String {
    text(key, locale).replace("{error}", &error.to_string())
}

#[tauri::command]
pub(crate) fn app_update_status(
    window: WebviewWindow,
    updater: State<'_, AppUpdater>,
) -> TauriIpcResult<UpdateSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    Ok(IpcResult::success(updater.snapshot.borrow().clone()))
}

#[tauri::command]
pub(crate) async fn app_update_check(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    updater: State<'_, AppUpdater>,
) -> TauriIpcResult<UpdateSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    updater.check(state.inner(), CheckOrigin::Manual).await?;
    Ok(IpcResult::success(updater.snapshot.borrow().clone()))
}

#[tauri::command]
pub(crate) async fn app_update_install(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    updater: State<'_, AppUpdater>,
) -> TauriIpcResult<UpdateSnapshot> {
    authorize_window(&window, CommandOrigin::Main)?;
    updater.install(state.inner()).await?;
    Ok(IpcResult::success(updater.snapshot.borrow().clone()))
}

