use super::{ActivationInput, OriginObservation};
use crate::capture::{CaptureOrigin, ReadyCapture};
use crate::ui_events::{EffectResult, UiEvent};
use coosenpai_core::ports::ForegroundApplication;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

const ORIGIN_RETURN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const KEY_OPERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

async fn before_deadline<T>(
    deadline: Option<tokio::time::Instant>,
    operation: impl std::future::Future<Output = Result<T, String>>,
    error: &str,
) -> Result<T, String> {
    match deadline {
        Some(deadline) => tokio::time::timeout_at(deadline, operation)
            .await
            .unwrap_or_else(|_| Err(error.to_owned())),
        None => operation.await,
    }
}

pub(super) async fn confirm_origin_returned<
    F: std::future::Future<Output = Result<bool, String>>,
>(
    changes: &mut tokio::sync::watch::Receiver<()>,
    mut observe: impl FnMut() -> F,
) -> Result<(), String> {
    loop {
        if observe().await? {
            return Ok(());
        }
        changes
            .changed()
            .await
            .map_err(|_| "アクティブ化の通知監視が終了しました".to_owned())?;
    }
}

#[derive(Clone)]
pub(super) struct ActivationLease {
    generation: u64,
    latest: Arc<Mutex<u64>>,
    cancellation: CancellationToken,
}

impl ActivationLease {
    pub(super) fn new() -> Self {
        Self {
            generation: 0,
            latest: Arc::new(Mutex::new(0)),
            cancellation: CancellationToken::new(),
        }
    }

    pub(super) fn supersede(&self, generation: u64) -> Self {
        let mut latest = self.latest.lock().expect("activation generation");
        self.cancellation.cancel();
        *latest = generation;
        Self {
            generation,
            latest: self.latest.clone(),
            cancellation: CancellationToken::new(),
        }
    }

    pub(super) fn execute<T>(
        &self,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        // 世代の更新と native 操作を同じ排他で囲み、検査直後の Supersede も防ぐ。
        let latest = self.latest.lock().expect("activation generation");
        if *latest != self.generation || self.cancellation.is_cancelled() {
            return Err("アクティベーション操作は取り消されました".into());
        }
        action()
    }
    pub(super) fn execute_key_before(
        &self,
        expires_at: Option<tokio::time::Instant>,
        action: impl FnOnce() -> Result<bool, String>,
    ) -> Result<bool, String> {
        self.execute_before(
            expires_at,
            "ポップアップの key 操作が3秒以内に完了しませんでした",
            action,
        )
    }

    pub(super) fn execute_before<T>(
        &self,
        expires_at: Option<tokio::time::Instant>,
        timeout_error: &str,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.execute(|| {
            if expires_at.is_some_and(|expires| tokio::time::Instant::now() >= expires) {
                Err(timeout_error.to_owned())
            } else {
                action()
            }
        })
    }
}

#[async_trait::async_trait]
pub(super) trait ActivationPort: Send + Sync + 'static {
    fn log(&self, message: &str);
    fn log_error(&self, message: &str);
    async fn observe_origin(
        &self,
        lease: ActivationLease,
        main_origin: Option<ForegroundApplication>,
    ) -> Result<OriginObservation, String>;
    async fn return_to_origin(
        &self,
        lease: ActivationLease,
        origin: ForegroundApplication,
        expires_at: Option<tokio::time::Instant>,
    ) -> Result<(), String>;
    async fn show_popup(
        &self,
        lease: ActivationLease,
        generation: u64,
        content: Arc<ReadyCapture>,
    ) -> Result<(), String>;
    async fn reacquire_popup_key(
        &self,
        lease: ActivationLease,
        generation: u64,
        expires_at: Option<tokio::time::Instant>,
    ) -> Result<bool, String>;
    async fn restore_origin(
        &self,
        lease: ActivationLease,
        origin: CaptureOrigin,
    ) -> Result<(), String>;
}

#[derive(Debug)]
pub(super) enum HandoffEffect {
    PrepareOrigin(Option<ForegroundApplication>),
    ReturnToOrigin(ForegroundApplication),
}

pub(crate) struct ActivationTask {
    port: Arc<dyn ActivationPort>,
    lease: ActivationLease,
    pub(super) effect: HandoffEffect,
    timeouts_enabled: bool,
}

impl std::fmt::Debug for ActivationTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ActivationTask").field(&self.effect).finish()
    }
}

impl ActivationTask {
    pub(super) fn new(
        port: Arc<dyn ActivationPort>,
        lease: ActivationLease,
        effect: HandoffEffect,
        timeouts_enabled: bool,
    ) -> Self {
        Self {
            port,
            lease,
            effect,
            timeouts_enabled,
        }
    }

    pub(crate) async fn run(self) -> Result<EffectResult, String> {
        let generation = self.lease.generation;
        let label = super::logging::handoff_label(&self.effect);
        let port = self.port.clone();
        port.log(&format!(
            "activation: generation={generation} effect={label} phase=begin"
        ));
        let result = self.run_inner().await;
        port.log(&format!(
            "activation: generation={generation} effect={label} phase=end result={}",
            super::logging::result_label(&result)
        ));
        result
    }

    async fn run_inner(self) -> Result<EffectResult, String> {
        let generation = self.lease.generation;
        let cancellation = self.lease.cancellation.clone();
        let operation = async move {
            let event = match self.effect {
                HandoffEffect::PrepareOrigin(main_origin) => ActivationInput::Prepared {
                    generation,
                    result: self
                        .port
                        .observe_origin(self.lease, main_origin)
                        .await
                        .map(super::prepare_origin),
                },
                HandoffEffect::ReturnToOrigin(origin) => {
                    let expires_at = self
                        .timeouts_enabled
                        .then(|| tokio::time::Instant::now() + ORIGIN_RETURN_TIMEOUT);
                    ActivationInput::Returned {
                        generation,
                        result: before_deadline(
                            expires_at,
                            self.port.return_to_origin(self.lease, origin, expires_at),
                            "撮影前の前面アプリへの復帰を2秒以内に確認できませんでした",
                        )
                        .await,
                    }
                }
            };
            EffectResult {
                value: None,
                events: vec![UiEvent::Activation(event)],
            }
        };
        tokio::select! { biased;
            () = cancellation.cancelled() => Ok(EffectResult::done()),
            result = operation => Ok(result),
        }
    }
}

#[derive(Debug)]
pub(super) enum ActivationEffect {
    MainWindow {
        command: crate::ui_events::ViewCommand,
        main_origin: Option<ForegroundApplication>,
    },
    ShowPopup {
        generation: u64,
        content: Arc<ReadyCapture>,
    },
    ReacquirePopupKey {
        generation: u64,
        attempt: u64,
    },
    RestoreOrigin(CaptureOrigin),
}

pub(crate) struct ActivationView {
    port: Arc<dyn ActivationPort>,
    lease: ActivationLease,
    pub(super) effect: ActivationEffect,
    timeouts_enabled: bool,
}

impl std::fmt::Debug for ActivationView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ActivationView").field(&self.effect).finish()
    }
}

impl ActivationView {
    pub(super) fn new(
        port: Arc<dyn ActivationPort>,
        lease: ActivationLease,
        effect: ActivationEffect,
        timeouts_enabled: bool,
    ) -> Self {
        Self {
            port,
            lease,
            effect,
            timeouts_enabled,
        }
    }

    pub(crate) fn presents_main(&self) -> bool {
        matches!(self.effect, ActivationEffect::MainWindow { .. })
    }

    pub(crate) async fn apply(
        self,
        ui: &dyn crate::ui_root::UiPort,
    ) -> Result<EffectResult, String> {
        let label = super::logging::view_label(&self.effect);
        let generation = self.lease.generation;
        let port = self.port.clone();
        port.log(&format!(
            "activation: generation={generation} effect={label} phase=begin"
        ));
        let result = self.apply_inner(ui).await;
        port.log(&format!(
            "activation: generation={generation} effect={label} phase=end result={}",
            super::logging::result_label(&result)
        ));
        result
    }

    async fn apply_inner(self, ui: &dyn crate::ui_root::UiPort) -> Result<EffectResult, String> {
        match self.effect {
            ActivationEffect::MainWindow {
                command,
                main_origin,
            } => {
                let origin = if command == crate::ui_events::ViewCommand::Show {
                    let observed = self.port.observe_origin(self.lease, main_origin).await?;
                    // 自アプリ内の再提示では、生存確認済みの表示開始時の origin を保持する。
                    let candidate = if observed.self_active {
                        observed.main_origin
                    } else {
                        observed.frontmost
                    };
                    Some(super::external_application(candidate, observed.self_pid))
                } else {
                    None
                };
                let mut result = ui
                    .execute(crate::ui_events::UiEffect::View {
                        view: crate::ui_events::PresenterId::Chat,
                        command,
                    })
                    .await?;
                if let Some(origin) = origin {
                    result
                        .events
                        .insert(0, UiEvent::Activation(ActivationInput::MainShown(origin)));
                }
                Ok(result)
            }
            ActivationEffect::ShowPopup {
                generation,
                content,
            } => {
                let result = self.port.show_popup(self.lease, generation, content).await;
                Ok(EffectResult {
                    value: None,
                    events: vec![UiEvent::Activation(ActivationInput::PopupShown {
                        generation,
                        result,
                    })],
                })
            }
            ActivationEffect::ReacquirePopupKey {
                generation,
                attempt,
            } => {
                let expires_at = self
                    .timeouts_enabled
                    .then(|| tokio::time::Instant::now() + KEY_OPERATION_TIMEOUT);
                let result = before_deadline(
                    expires_at,
                    self.port
                        .reacquire_popup_key(self.lease, generation, expires_at),
                    "ポップアップの key 操作が3秒以内に完了しませんでした",
                )
                .await;
                self.port.log(&format!("activation: generation={generation} effect=ReacquirePopupKey attempt={attempt} ok={}", matches!(result, Ok(true))));
                Ok(EffectResult {
                    value: None,
                    events: vec![UiEvent::Activation(ActivationInput::PopupKeyReacquired {
                        generation,
                        attempt,
                        result,
                    })],
                })
            }
            ActivationEffect::RestoreOrigin(origin) => {
                self.port.restore_origin(self.lease, origin).await?;
                Ok(EffectResult::done())
            }
        }
    }
}
