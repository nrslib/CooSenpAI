use crate::apple_termination;
use crate::state::DesktopState;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio::signal::unix::{signal, Signal, SignalKind};

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExitKind {
    Application,
    AppleEvent,
    Restart,
    Signal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShutdownPhase {
    Running,
    Cleaning,
    Cleaned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShutdownRequest {
    Start,
    Join,
    Cleaned,
}

struct ShutdownStatus {
    phase: ShutdownPhase,
    apple_reply_pending: bool,
}

struct ShutdownState {
    status: Mutex<ShutdownStatus>,
}

impl Default for ShutdownState {
    fn default() -> Self {
        Self {
            status: Mutex::new(ShutdownStatus {
                phase: ShutdownPhase::Running,
                apple_reply_pending: false,
            }),
        }
    }
}

impl ShutdownState {
    fn request(&self, kind: ExitKind) -> ShutdownRequest {
        let mut status = self.lock();
        if status.phase == ShutdownPhase::Cleaned {
            return ShutdownRequest::Cleaned;
        }
        if kind == ExitKind::AppleEvent {
            status.apple_reply_pending = true;
        }
        match status.phase {
            ShutdownPhase::Running => {
                status.phase = ShutdownPhase::Cleaning;
                ShutdownRequest::Start
            }
            ShutdownPhase::Cleaning => ShutdownRequest::Join,
            ShutdownPhase::Cleaned => ShutdownRequest::Cleaned,
        }
    }

    fn finish(&self) -> bool {
        let mut status = self.lock();
        status.phase = ShutdownPhase::Cleaned;
        std::mem::take(&mut status.apple_reply_pending)
    }

    fn lock(&self) -> MutexGuard<'_, ShutdownStatus> {
        // 終了処理は別 task の panic で lock が poison されても子プロセス回収を継続する。
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub struct ShutdownCoordinator {
    app: AppHandle,
    state: ShutdownState,
}

pub struct EarlySignals {
    interrupt: Signal,
    terminate: Signal,
}

pub async fn install_early_signals() -> Result<EarlySignals, std::io::Error> {
    Ok(EarlySignals {
        interrupt: signal(SignalKind::interrupt())?,
        terminate: signal(SignalKind::terminate())?,
    })
}

impl ShutdownCoordinator {
    pub fn new(app: AppHandle) -> Arc<Self> {
        Arc::new(Self {
            app,
            state: ShutdownState::default(),
        })
    }

    pub fn handle_exit_requested(self: &Arc<Self>, restart: bool) -> bool {
        self.request(if restart {
            ExitKind::Restart
        } else {
            ExitKind::Application
        })
    }

    pub fn handle_apple_event(self: &Arc<Self>) -> bool {
        self.request(ExitKind::AppleEvent)
    }

    fn request(self: &Arc<Self>, kind: ExitKind) -> bool {
        match self.state.request(kind) {
            ShutdownRequest::Cleaned => return false,
            ShutdownRequest::Join => return true,
            ShutdownRequest::Start => {}
        }
        let coordinator = self.clone();
        tauri::async_runtime::spawn(async move {
            coordinator.cleanup().await;
            let apple_reply_pending = coordinator.state.finish();
            coordinator.finish_exit(kind, apple_reply_pending);
        });
        true
    }

    fn finish_exit(self: &Arc<Self>, kind: ExitKind, apple_reply_pending: bool) {
        if apple_reply_pending {
            let coordinator = self.clone();
            if self
                .app
                .run_on_main_thread(move || {
                    apple_termination::reply_to_termination_request();
                    coordinator.exit_after_cleanup(kind);
                })
                .is_ok()
            {
                return;
            }
            self.app.exit(0);
            return;
        }
        self.exit_after_cleanup(kind);
    }

    fn exit_after_cleanup(&self, kind: ExitKind) {
        match kind {
            ExitKind::AppleEvent => {}
            ExitKind::Restart => self.app.restart(),
            ExitKind::Application | ExitKind::Signal => self.app.exit(0),
        }
    }

    async fn cleanup(&self) {
        if let Some(state) = self.app.try_state::<Arc<DesktopState>>() {
            if tokio::time::timeout(CLEANUP_TIMEOUT, state.shutdown())
                .await
                .is_err()
            {
                coosenpai_core::process::force_kill_provider_processes();
            }
        } else {
            coosenpai_core::process::force_kill_provider_processes();
        }
    }

    pub fn attach_signals(self: &Arc<Self>, signals: EarlySignals) {
        let coordinator = self.clone();
        tauri::async_runtime::spawn(async move {
            monitor_signals(coordinator, signals).await;
        });
    }
}

async fn monitor_signals(coordinator: Arc<ShutdownCoordinator>, mut signals: EarlySignals) {
    receive_signal(&mut signals).await;
    coordinator.request(ExitKind::Signal);
    receive_signal(&mut signals).await;
    coosenpai_core::process::force_kill_provider_processes();
    std::process::exit(0);
}

async fn receive_signal(signals: &mut EarlySignals) {
    tokio::select! {
        _ = signals.interrupt.recv() => {}
        _ = signals.terminate.recv() => {}
    }
}

