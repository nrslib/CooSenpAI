use crate::capture::effects::{CaptureEffect, CaptureOutput, CaptureTask};
use crate::capture::{CaptureEvent, CaptureKind, ReadyCapture};
use crate::command_guard::CommandSource;
use crate::ui_events::{UiEffect, UiEvent, UiTask, UiView, ViewCommand};
use coosenpai_core::ports::ForegroundApplication;
use runtime::{ActivationEffect, ActivationLease, ActivationPort, HandoffEffect};
use std::sync::Arc;
const KEY_RECOVERY_LIMIT: u8 = 3;

struct KeyRecovery {
    attempts: u64,
    failures: u8,
    in_flight: Option<u64>,
    exhausted_logged: bool,
}

impl KeyRecovery {
    fn new() -> Self {
        Self {
            attempts: 0,
            failures: 0,
            in_flight: None,
            exhausted_logged: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SelectionOpen {
    pub generation: u64,
    pub kind: CaptureKind,
    pub source: CommandSource,
}

#[derive(Default, Clone, Copy, Debug)]
enum Handoff {
    #[default]
    Ready,
    Preparing(SelectionOpen),
    Returning(SelectionOpen),
}

#[derive(Debug)]
pub(crate) enum ActivationInput {
    MainWindow(ViewCommand),
    MainShown(Option<ForegroundApplication>),
    Open(SelectionOpen),
    Supersede(u64),
    Prepared {
        generation: u64,
        result: Result<PreparedOrigin, String>,
    },
    Returned {
        generation: u64,
        result: Result<(), String>,
    },
    Popup {
        generation: u64,
        content: Arc<ReadyCapture>,
        frame: Vec<CaptureEffect>,
    },
    PopupFramePrepared {
        generation: u64,
        content: Arc<ReadyCapture>,
        result: Result<(), String>,
    },
    PopupShown {
        generation: u64,
        result: Result<(), String>,
    },
    HidePopup,
    PopupClosed {
        generation: u64,
        origin: crate::capture::CaptureOrigin,
        sent: bool,
        restarting: bool,
    },
    PopupHideFailed(u64),
    Deactivated,
    OtherApplicationActivated,
    PopupKeyLost(u64),
    PopupKeyReacquired {
        generation: u64,
        attempt: u64,
        result: Result<bool, String>,
    },
    PopupFailed(u64),
    NavigationFailed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PopupState {
    Preparing(u64),
    Shown(u64),
    Closing(u64),
}

impl PopupState {
    fn generation(self) -> u64 {
        match self {
            Self::Preparing(generation) | Self::Shown(generation) | Self::Closing(generation) => {
                generation
            }
        }
    }
}

/// 選択前の前面アプリ復帰と、非アクティブ化パネルの key 所有をまとめる。
pub(crate) struct ActivationPolicy {
    handoff: Handoff,
    generation: u64,
    popup: Option<PopupState>,
    key_recovery: Option<KeyRecovery>,
    main_origin: Option<ForegroundApplication>,
    origin: crate::capture::CaptureOrigin,
    port: Arc<dyn ActivationPort>,
    lease: ActivationLease,
    timeouts_enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CaptureOrigin {
    frontmost_application: Option<ForegroundApplication>,
}

#[derive(Debug, Default)]
pub(super) struct OriginObservation {
    self_active: bool,
    self_pid: i32,
    frontmost: Option<ForegroundApplication>,
    main_origin: Option<ForegroundApplication>,
}

#[derive(Debug, Default)]
pub(crate) struct PreparedOrigin {
    capture_origin: crate::capture::CaptureOrigin,
    return_to: Option<ForegroundApplication>,
}

impl ActivationPolicy {
    pub(crate) fn native(
        state: Arc<crate::state::DesktopState>,
        popup: crate::capture::window::CapturePopupView,
    ) -> Self {
        Self::with_port(native::NativeActivationPort::new(state, popup))
    }

    fn with_port(port: impl ActivationPort) -> Self {
        Self {
            handoff: Handoff::Ready,
            generation: 0,
            popup: None,
            key_recovery: None,
            main_origin: None,
            origin: Default::default(),
            port: Arc::new(port),
            lease: ActivationLease::new(),
            timeouts_enabled: true,
        }
    }

    fn supersede(&mut self, generation: u64) {
        self.generation = generation;
        self.lease = self.lease.supersede(generation);
        if let Some(recovery) = &mut self.key_recovery {
            recovery.in_flight = None;
        }
    }

    fn task(&self, effect: HandoffEffect) -> UiEffect {
        UiEffect::Spawn(UiTask::Activation(runtime::ActivationTask::new(
            self.port.clone(),
            self.lease.clone(),
            effect,
            self.timeouts_enabled,
        )))
    }

    fn fail_popup(&mut self, generation: u64) {
        self.supersede(generation);
        self.popup = None;
        self.key_recovery = None;
    }

    fn view(&self, effect: ActivationEffect) -> UiEffect {
        UiEffect::ActivationView(runtime::ActivationView::new(
            self.port.clone(),
            self.lease.clone(),
            effect,
            self.timeouts_enabled,
        ))
    }

    pub(crate) fn transition(&mut self, input: ActivationInput) -> Vec<UiEffect> {
        let before = format!("{:?}/{:?}", self.handoff, self.popup);
        let event = logging::input_label(&input);
        let effects = self.reduce(input);
        let actions: Vec<_> = effects.iter().map(logging::effect_label).collect();
        self.port.log(&format!(
            "activation: generation={} state={before} event={event} action=[{}] -> {:?}/{:?}",
            self.generation,
            actions.join(","),
            self.handoff,
            self.popup
        ));
        effects
    }

    fn reduce(&mut self, input: ActivationInput) -> Vec<UiEffect> {
        use ActivationInput as I;
        match input {
            I::MainWindow(command)
                if self.popup.is_none() && matches!(self.handoff, Handoff::Ready) =>
            {
                vec![self.view(ActivationEffect::MainWindow {
                    command,
                    main_origin: self.main_origin.clone(),
                })]
            }
            I::MainShown(origin) => {
                self.main_origin = origin;
                Vec::new()
            }
            // 表示を開始した popup の key 所有は、Hide の成功通知まで維持する。
            I::Open(_) | I::Supersede(_) | I::Prepared { .. } | I::Returned { .. }
                if self.popup.is_some() =>
            {
                vec![UiEffect::Log(
                    "activation: ignored=true reason=popup-protected".into(),
                )]
            }
            I::Open(open) if open.generation >= self.generation => {
                self.supersede(open.generation);
                self.handoff = Handoff::Preparing(open);
                self.popup = None;
                vec![self.task(HandoffEffect::PrepareOrigin(self.main_origin.clone()))]
            }
            I::Supersede(generation) if generation >= self.generation => {
                self.supersede(generation);
                self.handoff = Handoff::Ready;
                self.popup = None;
                Vec::new()
            }
            I::Prepared { generation, result } if matches!(self.handoff, Handoff::Preparing(open) if open.generation == generation) =>
            {
                let Handoff::Preparing(open) = self.handoff else {
                    unreachable!()
                };
                match result {
                    Err(error) => self.failed(open, error),
                    Ok(origin) => {
                        self.origin = origin.capture_origin;
                        self.handoff = Handoff::Returning(open);
                        let mut effects = crate::ui_root::hide_navigation();
                        effects.push(match origin.return_to {
                            Some(origin) => self.task(HandoffEffect::ReturnToOrigin(origin)),
                            None => UiEffect::Activation(I::Returned {
                                generation,
                                result: Ok(()),
                            }),
                        });
                        effects
                    }
                }
            }
            I::Returned { generation, result } if matches!(self.handoff, Handoff::Returning(open) if open.generation == generation) =>
            {
                let Handoff::Returning(open) = self.handoff else {
                    unreachable!()
                };
                match result {
                    Err(error) => self.failed(open, error),
                    Ok(()) => {
                        self.handoff = Handoff::Ready;
                        vec![UiEffect::Spawn(UiTask::Capture {
                            effect: CaptureEffect::OpenSelection {
                                generation,
                                kind: open.kind,
                                source: open.source,
                                origin: self.origin.clone(),
                            },
                            completion: CaptureTask::Selection(generation),
                        })]
                    }
                }
            }
            I::Popup {
                generation,
                content,
                frame,
            } if generation >= self.generation => {
                self.supersede(generation);
                self.handoff = Handoff::Ready;
                self.popup = Some(PopupState::Preparing(generation));
                self.key_recovery = None;
                vec![UiEffect::CaptureView {
                    effects: frame,
                    completion: CaptureOutput::PopupFramePrepared {
                        generation,
                        content,
                    },
                }]
            }
            I::PopupFramePrepared {
                generation,
                content,
                result,
            } if self.popup == Some(PopupState::Preparing(generation)) => match result {
                Ok(()) => vec![self.view(ActivationEffect::ShowPopup {
                    generation,
                    content,
                })],
                Err(error) => {
                    self.fail_popup(generation);
                    vec![UiEffect::Deliver {
                        child: crate::ui_events::PresenterId::Capture,
                        event: UiEvent::CaptureCompleted(Box::new(CaptureEvent::Applied(
                            crate::capture::effects::CaptureApplied {
                                completion: CaptureOutput::Presented {
                                    view: UiView::CapturePopup,
                                    generation,
                                },
                                result: Err(error),
                            },
                        ))),
                    }]
                }
            },
            I::PopupShown { generation, result }
                if self.popup == Some(PopupState::Preparing(generation)) =>
            {
                let shown = result.is_ok();
                if shown {
                    self.popup = Some(PopupState::Shown(generation));
                    self.key_recovery = Some(KeyRecovery::new());
                } else {
                    self.fail_popup(generation);
                }
                let mut effects = vec![UiEffect::Deliver {
                    child: crate::ui_events::PresenterId::Capture,
                    event: UiEvent::CaptureCompleted(Box::new(CaptureEvent::Applied(
                        crate::capture::effects::CaptureApplied {
                            completion: CaptureOutput::Presented {
                                view: UiView::CapturePopup,
                                generation,
                            },
                            result,
                        },
                    ))),
                }];
                if shown {
                    effects.extend(self.reacquire_key(generation));
                }
                effects
            }
            I::HidePopup => {
                self.supersede(self.generation);
                self.popup = self
                    .popup
                    .map(|popup| PopupState::Closing(popup.generation()));
                Vec::new()
            }
            I::PopupHideFailed(generation) if generation == self.generation => {
                self.popup = Some(PopupState::Shown(generation));
                Vec::new()
            }
            I::PopupClosed {
                generation,
                origin,
                sent,
                restarting,
            } if self.popup == Some(PopupState::Closing(generation)) => {
                self.popup = None;
                if sent || restarting || origin.frontmost_application.is_none() {
                    Vec::new()
                } else {
                    vec![self.view(ActivationEffect::RestoreOrigin(origin))]
                }
            }
            I::PopupFailed(generation)
                if self
                    .popup
                    .is_some_and(|popup| popup.generation() == generation) =>
            {
                self.fail_popup(generation);
                Vec::new()
            }
            I::NavigationFailed(error) => match self.handoff {
                Handoff::Preparing(open) | Handoff::Returning(open) => self.failed(open, error),
                Handoff::Ready => Vec::new(),
            },
            I::Deactivated | I::OtherApplicationActivated => match self.popup {
                Some(PopupState::Shown(generation)) => self.reacquire_key(generation),
                _ => Vec::new(),
            },
            I::PopupKeyLost(generation) if self.popup == Some(PopupState::Shown(generation)) => {
                self.reacquire_key(generation)
            }
            I::PopupKeyReacquired {
                generation,
                attempt,
                result,
            } if self.popup == Some(PopupState::Shown(generation))
                && self
                    .key_recovery
                    .as_ref()
                    .is_some_and(|recovery| recovery.in_flight == Some(attempt)) =>
            {
                self.key_recovery
                    .as_mut()
                    .expect("current key attempt")
                    .in_flight = None;
                match result {
                    Ok(true) => {
                        self.key_recovery
                            .as_mut()
                            .expect("current key attempt")
                            .failures = 0;
                        Vec::new()
                    }
                    Ok(false) => {
                        let recovery = self.key_recovery.as_mut().expect("current key attempt");
                        recovery.failures += 1;
                        if recovery.failures >= KEY_RECOVERY_LIMIT {
                            self.key_failed(generation, "attempt-limit")
                        } else {
                            self.reacquire_key(generation)
                        }
                    }
                    Err(error) => self.key_failed(generation, &error),
                }
            }
            _ => vec![UiEffect::Log(
                "activation: ignored=true reason=stale-or-inapplicable".into(),
            )],
        }
    }

    fn reacquire_key(&mut self, generation: u64) -> Vec<UiEffect> {
        let Some(recovery) = &mut self.key_recovery else {
            return Vec::new();
        };
        if recovery.in_flight.is_some() {
            return Vec::new();
        }
        if recovery.exhausted_logged {
            return Vec::new();
        }
        recovery.attempts += 1;
        let attempt = recovery.attempts;
        recovery.in_flight = Some(attempt);
        vec![self.view(ActivationEffect::ReacquirePopupKey {
            generation,
            attempt,
        })]
    }

    fn key_failed(&mut self, generation: u64, reason: &str) -> Vec<UiEffect> {
        if let Some(recovery) = &mut self.key_recovery {
            recovery.exhausted_logged = true;
        }
        self.port.log_error(&format!("activation: generation={generation} error=popup-key-recovery-exhausted reason={reason}"));
        vec![UiEffect::Deliver {
            child: crate::ui_events::PresenterId::Capture,
            event: UiEvent::CaptureCompleted(Box::new(CaptureEvent::PopupCancel {
                generation,
                source: crate::capture::manager::CancelSource::ActivationFailure,
            })),
        }]
    }

    fn failed(&mut self, open: SelectionOpen, error: String) -> Vec<UiEffect> {
        self.port.log_error(&format!(
            "activation: generation={} error={error}",
            open.generation
        ));
        self.supersede(open.generation);
        self.handoff = Handoff::Ready;
        vec![UiEffect::Deliver {
            child: crate::ui_events::PresenterId::Capture,
            event: UiEvent::CaptureCompleted(Box::new(CaptureEvent::Selection {
                generation: open.generation,
                event: crate::capture::manager::SelectionEvent::Failed(error),
            })),
        }]
    }
}

fn external_application(
    application: Option<ForegroundApplication>,
    self_pid: i32,
) -> Option<ForegroundApplication> {
    application.filter(|application| application.process_id != self_pid)
}

fn prepare_origin(observation: OriginObservation) -> PreparedOrigin {
    let return_to = if observation.self_active {
        external_application(observation.main_origin, observation.self_pid)
    } else {
        None
    };
    let application = if observation.self_active {
        return_to.clone()
    } else {
        external_application(observation.frontmost, observation.self_pid)
    };
    PreparedOrigin {
        capture_origin: CaptureOrigin {
            frontmost_application: application,
        },
        return_to,
    }
}

#[path = "activation_native.rs"]
mod native;
#[path = "activation_runtime.rs"]
mod runtime;
pub(crate) use runtime::{ActivationTask, ActivationView};

#[cfg(test)]
#[path = "activation_test_support.rs"]
pub(crate) mod test_support;

#[path = "activation_logging.rs"]
mod logging;
