use crate::state::DesktopState;
use coosenpai_core::config::VoiceOutputConfig;
use coosenpai_core::locale::{text as localized_text, Locale, TextKey};
use coosenpai_core::ports::RuntimeLogger;
use coosenpai_core::voice_output::VoiceOutputProviderFactory;
use serde::Serialize;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{watch, Mutex, Notify};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct VoiceOutputSnapshot {
    pub revision: u64,
    pub speaking: bool,
    pub message: Option<String>,
}

struct VoiceRequest {
    text: String,
    generation: u64,
    config: VoiceOutputConfig,
    locale: Locale,
    cancellation: CancellationToken,
}

pub(crate) struct VoiceOutputController {
    providers: Arc<dyn VoiceOutputProviderFactory>,
    pending: Mutex<Option<VoiceRequest>>,
    ready: Notify,
    epoch: Mutex<CancellationToken>,
    operation: Mutex<()>,
    pub(crate) start_gate: Mutex<()>,
    active: AtomicBool,
    snapshot: watch::Sender<VoiceOutputSnapshot>,
}

impl VoiceOutputController {
    pub(crate) fn new(providers: Arc<dyn VoiceOutputProviderFactory>) -> Self {
        Self {
            providers,
            pending: Mutex::new(None),
            ready: Notify::new(),
            epoch: Mutex::new(CancellationToken::new()),
            operation: Mutex::new(()),
            start_gate: Mutex::new(()),
            active: AtomicBool::new(false),
            snapshot: watch::channel(VoiceOutputSnapshot::default()).0,
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    pub(crate) fn snapshot(&self) -> VoiceOutputSnapshot {
        self.snapshot.borrow().clone()
    }

    fn publish(&self, state: &DesktopState, message: Option<String>) {
        self.snapshot.send_modify(|snapshot| {
            snapshot.revision += 1;
            snapshot.speaking = self.is_active();
            snapshot.message = message;
        });
        state.ui.input(
            crate::ui_events::UiView::Application,
            crate::ui_events::UiEvent::ChatProjection(
                crate::ui_events::ChatProjection::VoiceOutput(self.snapshot()),
            ),
        );
    }

    pub(crate) fn start(self: &Arc<Self>, state: &Arc<DesktopState>) {
        let controller = self.clone();
        let weak_state = Arc::downgrade(state);
        let shutdown = state.cancellation.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown.cancelled() => break,
                    _ = controller.ready.notified() => {},
                }
                let Some(request) = controller.pending.lock().await.take() else {
                    continue;
                };
                let Some(state) = weak_state.upgrade() else {
                    break;
                };
                controller.play(&state, request).await;
            }
        });
    }

    pub(crate) async fn enqueue(
        &self,
        state: &DesktopState,
        text: String,
        generation: u64,
    ) -> Result<(), String> {
        let epoch = self.epoch.lock().await;
        let runtime_config = state.runtime_config();
        let config = runtime_config.voice_output;
        let locale = Locale::from_config(&runtime_config.ui.language);
        if !config.enabled {
            return Err(localized_text(TextKey::VoiceOutputDisabled, locale).to_owned());
        }
        if text.trim().is_empty() || text.len() > 16 * 1024 {
            return Err(localized_text(TextKey::VoiceOutputTextTooLong, locale).to_owned());
        }
        let cancellation = epoch.child_token();
        let mut pending = self.pending.lock().await;
        if pending.is_some() {
            return Err(localized_text(TextKey::VoiceOutputPending, locale).to_owned());
        }
        *pending = Some(VoiceRequest {
            text,
            generation,
            config,
            locale,
            cancellation,
        });
        self.ready.notify_one();
        Ok(())
    }

    async fn play(&self, state: &Arc<DesktopState>, request: VoiceRequest) {
        // Speech 開始も同じ gate を持ち、再生の停止完了後に録音状態へ遷移する。
        let start = self.start_gate.lock().await;
        let _operation = self.operation.lock().await;
        let mut runtime_changes = state.core_runtime().subscribe_snapshots();
        if request.cancellation.is_cancelled()
            || state.is_shutting_down()
            || !state.is_runtime_active()
            || state.runtime_config().voice_output != request.config
            || state.speech_resource_phase() != crate::command_guard::ResourcePhase::Idle
            || state.voice_conversation_generation().await != request.generation
        {
            return;
        }
        let provider = match self.providers.create(&request.config, request.locale) {
            Ok(provider) => provider,
            Err(message) => {
                self.publish(state, Some(message));
                return;
            }
        };
        self.active.store(true, Ordering::Release);
        drop(start);
        state.cancel_audio_and_wait().await;
        self.publish(state, None);
        let token = request.cancellation.clone();
        let result = if token.is_cancelled() || state.cancellation.is_cancelled() {
            Ok(())
        } else {
            let stop_on_shutdown = token.clone();
            let shutdown = state.cancellation.clone();
            let config_changed = async {
                loop {
                    if state.runtime_config().voice_output != request.config
                        || runtime_changes.changed().await.is_err()
                    {
                        return;
                    }
                }
            };
            let speaking = provider.speak(
                &request.text,
                request.config.rate,
                request.locale,
                token.clone(),
            );
            tokio::pin!(speaking);
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => { stop_on_shutdown.cancel(); speaking.await },
                _ = config_changed => { token.cancel(); speaking.await },
                result = &mut speaking => result,
            }
        };
        // 入力再開より先に Provider の停止を確認する。認識済みの残留音声は前のsessionと共に破棄済み。
        self.active.store(false, Ordering::Release);
        let message = result.err();
        if message.is_some() {
            let _ = state
                .logger
                .write("WARN", "音声 Provider の再生に失敗しました");
        }
        self.publish(state, message);
        if !state.is_shutting_down() {
            state.sync_audio();
        }
    }

    pub(crate) async fn stop(&self) {
        let mut epoch = self.epoch.lock().await;
        epoch.cancel();
        *epoch = CancellationToken::new();
        self.pending.lock().await.take();
        // worker は epoch を取得しないため、停止まで新しい受付を待たせられる。
        let _operation = self.operation.lock().await;
    }
}

impl DesktopState {
    pub(crate) async fn accept_voice_notification(
        &self,
        record: &coosenpai_core::notification::NotificationRecord,
    ) {
        if record.message_kind != "chat" || !self.runtime_config().voice_output.enabled {
            return;
        }
        if let Err(message) = self
            .voice_output
            .enqueue(self, record.message.clone(), record.conversation_generation)
            .await
        {
            self.voice_output.publish(self, Some(message));
        }
    }
}
