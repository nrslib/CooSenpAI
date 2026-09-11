use crate::snapshot::{AppSnapshot, ObserverViewPhase};
use crate::ui_events::{UiEffect, UiEvent, UiTask};
use crate::watch::{trigger_name, CaptureDisposition};
use coosenpai_core::locale::{localize_error_message, Locale, TextKey};
use coosenpai_core::state::{ActivityTriggerKind, ObservationRecord};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub(crate) enum WatchResult {
    Started,
    Stopped,
    Finished {
        failed: bool,
    },
    Suspended,
    CaptureStarted,
    CaptureSkipped,
    Buffered {
        captured_at: String,
        trigger: ActivityTriggerKind,
        front_app: Option<String>,
        frame_count: usize,
        next_send: String,
    },
    CaptureDecision {
        trigger: ActivityTriggerKind,
        front_app: Option<String>,
        disposition: CaptureDisposition,
    },
    ObservationStarted,
    Observed {
        observation: ObservationRecord,
        calls: u32,
    },
    NotDelivered,
    TargetForeground {
        bundle_id: String,
        foreground: bool,
    },
    TargetCaptured {
        bundle_id: String,
        trigger: ActivityTriggerKind,
        disposition: CaptureDisposition,
        captured_at: Option<String>,
        frame_count: usize,
        next_send_at: Option<String>,
    },
    HeartbeatObserved(ObservationRecord),
    FailureObserved {
        attempt: u32,
        detail: String,
        tutorial: bool,
        cancellation: CancellationToken,
    },
    ExitObserved {
        detail: String,
        tutorial: bool,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    RecoveryExpired(u64),
    Recovered,
    OcrConfigured(bool),
}

#[derive(Default)]
pub(crate) struct WatchPresenter {
    generation: u64,
    active: bool,
    error_epoch: u64,
    pending_error: Option<(String, CancellationToken)>,
    effects: Vec<UiEffect>,
}

impl WatchPresenter {
    pub(crate) fn take_effects(&mut self) -> Vec<UiEffect> {
        std::mem::take(&mut self.effects)
    }

    fn clear_pending_error(&mut self) {
        self.error_epoch = self.error_epoch.saturating_add(1);
        self.pending_error = None;
    }

    pub(crate) fn adopt(
        &mut self,
        snapshot: &mut AppSnapshot,
        generation: u64,
        result: WatchResult,
    ) -> bool {
        match result {
            WatchResult::ExitObserved {
                detail,
                tutorial,
                reply,
            } => {
                let failed = generation == self.generation && self.active && !tutorial;
                if failed {
                    self.clear_pending_error();
                    snapshot.observer.record_error(localize_error_message(
                        &detail,
                        TextKey::WatchOperationFailed,
                        Locale::from_config(&snapshot.config.ui.language),
                    ));
                }
                let _ = reply.send(failed);
                return failed;
            }
            WatchResult::Started => {
                if generation < self.generation {
                    return false;
                }
                self.clear_pending_error();
                self.generation = generation;
                self.active = true;
                snapshot.observer_running = true;
                snapshot.watch_intent_active = true;
                snapshot.observer.phase = ObserverViewPhase::Idle;
                snapshot.observer.error_message = None;
                return true;
            }
            WatchResult::Stopped => {
                if generation < self.generation {
                    return false;
                }
                self.clear_pending_error();
                self.generation = generation;
                self.active = false;
                snapshot.observer_running = false;
                snapshot.watch_intent_active = false;
                snapshot.observer.phase = ObserverViewPhase::Stopped;
                snapshot.observer.pending_frame_count = 0;
                snapshot.observer.next_send_at = None;
                return true;
            }
            WatchResult::Suspended if generation == self.generation && !self.active => {
                snapshot.observer.phase = ObserverViewPhase::Suspended;
                return true;
            }
            _ => {}
        }
        if generation != self.generation || !self.active {
            return false;
        }
        let locale = Locale::from_config(&snapshot.config.ui.language);
        let view = &mut snapshot.observer;
        match result {
            WatchResult::Finished { failed } => {
                self.clear_pending_error();
                self.active = false;
                snapshot.finish_watch(failed);
            }
            WatchResult::CaptureStarted => view.phase = ObserverViewPhase::Capturing,
            WatchResult::CaptureSkipped => view.phase = ObserverViewPhase::Idle,
            WatchResult::Buffered {
                captured_at,
                trigger,
                front_app,
                frame_count,
                next_send,
            } => {
                view.phase = ObserverViewPhase::Idle;
                view.last_captured_at = Some(captured_at);
                view.last_trigger = Some(trigger_name(trigger).to_owned());
                view.front_app = front_app;
                view.pending_frame_count = frame_count;
                view.next_send_at = Some(next_send);
            }
            WatchResult::CaptureDecision {
                trigger,
                front_app,
                disposition,
            } => {
                view.last_trigger = Some(trigger_name(trigger).to_owned());
                view.front_app = front_app;
                view.last_capture_disposition =
                    Some(disposition.display_for_locale(locale).to_owned());
            }
            WatchResult::ObservationStarted => view.phase = ObserverViewPhase::Thinking,
            WatchResult::Observed { observation, calls } => {
                view.phase = ObserverViewPhase::Idle;
                view.error_message = None;
                view.pending_frame_count = 0;
                view.next_send_at = None;
                view.record_observation(observation);
                view.ai_calls_today = calls;
            }
            WatchResult::NotDelivered => {
                view.phase = ObserverViewPhase::Idle;
                view.error_message = None;
                view.pending_frame_count = 0;
                view.next_send_at = None;
            }
            WatchResult::TargetForeground {
                bundle_id,
                foreground,
            } => {
                if let Some(target) = view
                    .targets
                    .iter_mut()
                    .find(|target| target.target == format!("app:{bundle_id}"))
                {
                    target.foreground = foreground;
                }
            }
            WatchResult::TargetCaptured {
                bundle_id,
                trigger,
                disposition,
                captured_at,
                frame_count,
                next_send_at,
            } => {
                view.phase = ObserverViewPhase::Idle;
                view.last_trigger = Some(trigger_name(trigger).to_owned());
                view.last_capture_disposition =
                    Some(disposition.display_for_locale(locale).to_owned());
                if disposition == CaptureDisposition::Accepted {
                    view.pending_frame_count = view.pending_frame_count.saturating_add(frame_count);
                    view.next_send_at = next_send_at;
                }
                if captured_at.is_some() {
                    view.last_captured_at = captured_at.clone();
                }
                if let Some(target) = view
                    .targets
                    .iter_mut()
                    .find(|target| target.target == format!("app:{bundle_id}"))
                {
                    target.last_trigger = Some(trigger_name(trigger).to_owned());
                    if captured_at.is_some() {
                        target.last_captured_at = captured_at;
                    }
                }
            }
            WatchResult::HeartbeatObserved(observation) => view.record_observation(observation),
            WatchResult::FailureObserved {
                attempt,
                detail,
                tutorial,
                cancellation,
            } => {
                if tutorial || cancellation.is_cancelled() {
                    return false;
                }
                if attempt >= 3 {
                    self.clear_pending_error();
                    view.record_recoverable_error(localize_error_message(
                        &detail,
                        TextKey::WatchOperationFailed,
                        locale,
                    ));
                } else {
                    if let Some((latest, _)) = &mut self.pending_error {
                        *latest = detail;
                    } else {
                        self.error_epoch = self.error_epoch.saturating_add(1);
                        self.pending_error = Some((detail, cancellation));
                        self.effects.push(UiEffect::Spawn(UiTask::Delay {
                            duration: std::time::Duration::from_secs(10),
                            event: UiEvent::SnapshotCompleted(Box::new(
                                crate::snapshot_presenter::SnapshotEvent::Watch {
                                    generation,
                                    event: WatchResult::RecoveryExpired(self.error_epoch),
                                },
                            )),
                        }));
                    }
                    return false;
                }
            }
            WatchResult::RecoveryExpired(epoch) => {
                if epoch != self.error_epoch {
                    return false;
                }
                let Some((detail, cancellation)) = self.pending_error.take() else {
                    return false;
                };
                if cancellation.is_cancelled() {
                    return false;
                }
                view.record_recoverable_error(localize_error_message(
                    &detail,
                    TextKey::WatchOperationFailed,
                    locale,
                ));
            }
            WatchResult::Recovered => {
                self.clear_pending_error();
                view.clear_error();
            }
            WatchResult::ExitObserved { .. } => {
                unreachable!("exit result was handled before generation filtering")
            }
            WatchResult::OcrConfigured(enabled) => view.ocr_gate_enabled = enabled,
            WatchResult::Started | WatchResult::Stopped | WatchResult::Suspended => return false,
        }
        true
    }
}

