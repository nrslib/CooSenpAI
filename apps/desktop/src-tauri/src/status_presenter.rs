use crate::snapshot::AppSnapshot;
use crate::ui_events::{UiEffect, UiEvent, UiTask};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum UiText {
    Literal {
        text: String,
    },
    Message {
        key: &'static str,
        args: BTreeMap<&'static str, UiText>,
    },
    Join {
        parts: Vec<UiText>,
    },
}
impl UiText {
    pub(crate) fn message(key: &'static str) -> Self {
        Self::Message {
            key,
            args: BTreeMap::new(),
        }
    }
    pub(crate) fn literal(text: impl ToString) -> Self {
        Self::Literal {
            text: text.to_string(),
        }
    }
    pub(crate) fn arg(mut self, key: &'static str, value: Self) -> Self {
        if let Self::Message { args, .. } = &mut self {
            args.insert(key, value);
        }
        self
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PresenceView {
    pub mode: &'static str,
    pub text: UiText,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ThoughtView {
    pub text: UiText,
    pub leaving: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RecoveryAction {
    Settings,
    ScreenCapture,
    SystemAudio,
    Microphone,
    Recognition,
    Relaunch,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BannerView {
    pub tone: &'static str,
    pub message: UiText,
    pub action: Option<RecoveryAction>,
    pub action_label: Option<UiText>,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct StatusView {
    pub presence: PresenceView,
    pub thought: Option<ThoughtView>,
    pub banner: Option<BannerView>,
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum StatusDeadline {
    Presence(u64),
    Thought(u64),
}
pub(crate) struct StatusPresenter {
    view: StatusView,
    initialized: bool,
    candidate: Option<PresenceView>,
    presence_generation: u64,
    thought_generation: u64,
}
impl Default for StatusPresenter {
    fn default() -> Self {
        Self {
            view: StatusView {
                presence: PresenceView {
                    mode: "resting",
                    text: UiText::message("view.resting"),
                },
                thought: None,
                banner: None,
            },
            initialized: false,
            candidate: None,
            presence_generation: 0,
            thought_generation: 0,
        }
    }
}
impl StatusPresenter {
    pub(crate) fn observe(
        &mut self,
        snapshot: &AppSnapshot,
        watch_changing: bool,
        error: Option<&str>,
    ) -> Vec<UiEffect> {
        let mut effects = vec![];
        let next = presence(snapshot, watch_changing);
        if !self.initialized {
            self.view.presence = next;
            self.initialized = true;
        } else if next.mode == self.view.presence.mode {
            self.candidate = None;
            self.presence_generation += 1;
            self.view.presence.text = next.text;
        } else if self
            .candidate
            .as_ref()
            .is_none_or(|candidate| candidate.mode != next.mode)
        {
            self.candidate = Some(next);
            self.presence_generation += 1;
            effects.push(UiEffect::Spawn(UiTask::StatusDelay(
                StatusDeadline::Presence(self.presence_generation),
            )));
        } else {
            self.candidate = Some(next);
        }
        match thought(snapshot) {
            Some(text) => {
                self.thought_generation += 1;
                self.view.thought = Some(ThoughtView {
                    text,
                    leaving: false,
                });
            }
            None => {
                if let Some(displayed) = &mut self.view.thought {
                    if !displayed.leaving {
                        displayed.leaving = true;
                        self.thought_generation += 1;
                        effects.push(UiEffect::Spawn(UiTask::StatusDelay(
                            StatusDeadline::Thought(self.thought_generation),
                        )));
                    }
                }
            }
        }
        self.view.banner = error
            .map(|error| banner("error", UiText::literal(error), None))
            .or_else(|| status_banner(snapshot));
        effects.insert(0, self.render());
        effects
    }
    pub(crate) fn deadline(&mut self, deadline: StatusDeadline) -> Vec<UiEffect> {
        match deadline {
            StatusDeadline::Presence(generation) if generation == self.presence_generation => {
                if let Some(candidate) = self.candidate.take() {
                    self.view.presence = candidate;
                } else {
                    return vec![];
                }
            }
            StatusDeadline::Thought(generation)
                if generation == self.thought_generation
                    && self.view.thought.as_ref().is_some_and(|view| view.leaving) =>
            {
                self.view.thought = None
            }
            _ => return vec![],
        }
        vec![self.render()]
    }
    pub(crate) fn recovery(&self) -> Option<RecoveryAction> {
        self.view.banner.as_ref().and_then(|view| view.action)
    }
    pub(crate) fn render(&self) -> UiEffect {
        UiEffect::StatusRender(Box::new(self.view.clone()))
    }
}
fn response_in_progress(s: &AppSnapshot) -> bool {
    s.active_user_message_id.is_some()
        && s.last_error
            .as_ref()
            .and_then(|e| e.attachment_ocr.as_ref())
            .is_none_or(|e| Some(&e.input_id) != s.active_user_message_id.as_ref())
}
fn observer_error(s: &AppSnapshot) -> Option<UiText> {
    s.observer
        .error_message
        .as_ref()
        .map(UiText::literal)
        .or_else(|| {
            (s.observer.phase == crate::snapshot::ObserverViewPhase::Error)
                .then(|| UiText::message("view.observerErrorDefault"))
        })
}
fn presence(s: &AppSnapshot, changing: bool) -> PresenceView {
    let (mode, text) = if s.onboarding.setup_required {
        ("resting", UiText::message("view.setupWaiting"))
    } else if changing {
        ("switching", UiText::message("view.switching"))
    } else if let Some(error) = observer_error(s) {
        (
            "attention",
            UiText::message("view.observerError").arg("message", error),
        )
    } else if response_in_progress(s) {
        ("thinking", UiText::message("view.thinking"))
    } else if s.last_error.is_some() || s.delivery_outbox_blocked {
        ("attention", UiText::message("view.needsAttention"))
    } else if s.observer_running {
        ("watching", UiText::message("view.watching"))
    } else {
        ("resting", UiText::message("view.resting"))
    };
    PresenceView { mode, text }
}
fn thought(s: &AppSnapshot) -> Option<UiText> {
    if !s.config.ui.thought_bubble || !response_in_progress(s) {
        return None;
    }
    let Some(raw) = &s.companion_draft else {
        return Some(UiText::message("view.draft"));
    };
    let lines: Vec<_> = raw.lines().filter(|line| !line.trim().is_empty()).collect();
    let tail = if lines.is_empty() {
        raw.clone()
    } else {
        lines[lines.len().saturating_sub(3)..].join("\n")
    };
    let chars: Vec<_> = tail.chars().collect();
    Some(UiText::literal(if chars.len() <= 240 {
        tail
    } else {
        format!("…{}", chars[chars.len() - 239..].iter().collect::<String>())
    }))
}
fn banner(tone: &'static str, message: UiText, action: Option<RecoveryAction>) -> BannerView {
    let action_label = action.map(|action| {
        UiText::message(match action {
            RecoveryAction::Settings => "view.actionSettings",
            RecoveryAction::Relaunch => "view.relaunch",
            RecoveryAction::SystemAudio => "view.actionSystemAudioSettings",
            _ => "view.actionSystemSettings",
        })
    });
    BannerView {
        tone,
        message,
        action,
        action_label,
    }
}
fn audio_action(s: &AppSnapshot) -> Option<RecoveryAction> {
    use RecoveryAction::*;
    match s.audio.warning_kind.as_deref() {
        Some("permission-speech") => return Some(Recognition),
        Some("permission-microphone") => return Some(Microphone),
        Some("screen-capture") => return Some(ScreenCapture),
        Some("system-audio" | "system-audio-permission" | "system-audio-start-timeout") => {
            return Some(SystemAudio)
        }
        Some("system-audio-device" | "system-audio-format" | "system-audio-overflow") => {
            return None
        }
        _ => {}
    }
    if s.config.audio.speaker
        && !matches!(
            s.audio.screen_capture_permission.as_str(),
            "granted" | "not-required"
        )
    {
        Some(ScreenCapture)
    } else if s.config.audio.mic && s.audio.microphone_permission != "granted" {
        Some(Microphone)
    } else if s.audio.recognition_permission != "granted" {
        Some(Recognition)
    } else {
        None
    }
}
fn retry(seconds: Option<u64>) -> UiText {
    seconds.map_or_else(
        || UiText::literal(""),
        |seconds| UiText::message("view.retryAfter").arg("value", UiText::literal(seconds)),
    )
}
fn status_banner(s: &AppSnapshot) -> Option<BannerView> {
    use RecoveryAction::*;
    use UiText as Text;
    if s.onboarding.finish_pending {
        return Some(banner(
            "error",
            Text::message("app.tutorialFinishPending"),
            None,
        ));
    }
    if let Some(error) = &s.capture_shortcut_error {
        return Some(banner("warning", Text::literal(error), Some(Settings)));
    }
    if let Some(message) = &s.speech.message {
        let action = if s.speech.microphone_permission != "granted" {
            Some(Microphone)
        } else if s.speech.recognition_permission != "granted" {
            Some(Recognition)
        } else {
            None
        };
        return Some(banner("warning", Text::literal(message), action));
    }
    if let Some(message) = s.audio.message.as_ref().filter(|_| s.config.audio.enabled) {
        return Some(banner("warning", Text::literal(message), audio_action(s)));
    }
    if let Some(error) = &s.last_error {
        if error.kind == coosenpai_core::runtime::RuntimeErrorKind::Config {
            return Some(banner(
                "error",
                error
                    .message
                    .as_ref()
                    .map(Text::literal)
                    .unwrap_or_else(|| Text::message("view.configCheck")),
                Some(Settings),
            ));
        }
    }
    if let Some(error) = observer_error(s) {
        return Some(banner(
            "error",
            Text::message("view.observerFailed").arg("message", error),
            Some(Settings),
        ));
    }
    if let Some(failure) = s
        .last_error
        .as_ref()
        .and_then(|e| e.attachment_ocr.as_ref())
    {
        use coosenpai_core::companion::AttachmentOcrFailureKind;
        let key = match failure.reason {
            AttachmentOcrFailureKind::Capability => "view.attachmentCapability",
            AttachmentOcrFailureKind::HelperUnavailable => "view.attachmentHelper",
            AttachmentOcrFailureKind::Recognition => "view.attachmentRecognition",
            AttachmentOcrFailureKind::NoText => "view.attachmentNoText",
        };
        return Some(banner(
            "error",
            Text::Join {
                parts: vec![
                    Text::message(key),
                    retry(s.companion_retry_in_seconds.filter(|_| failure.retryable)),
                ],
            },
            None,
        ));
    }
    if s.watch_intent_active
        && !s.config.watch.fullscreen
        && !s.config.watch.apps.iter().any(|a| a.enabled)
    {
        return Some(banner(
            "info",
            Text::message("view.watchTargetHelp"),
            Some(Settings),
        ));
    }
    if s.watch_intent_active && s.screen_recording_status != "granted" {
        return Some(banner(
            "warning",
            s.screen_recording_message
                .as_ref()
                .map(Text::literal)
                .unwrap_or_else(|| Text::message("view.screenPermission")),
            Some(if s.screen_recording_restart_required {
                Relaunch
            } else {
                ScreenCapture
            }),
        ));
    }
    if s.delivery_outbox_blocked {
        return Some(banner(
            "warning",
            Text::message("view.pendingDelivery").arg("count", Text::literal(s.pending_deliveries)),
            None,
        ));
    }
    s.last_error.as_ref().map(|error| {
        banner(
            "info",
            Text::message("view.companionFailed")
                .arg("name", Text::literal(&s.companion_display_name))
                .arg("kind", Text::literal(error.kind.as_str()))
                .arg("retry", retry(s.companion_retry_in_seconds)),
            None,
        )
    })
}
pub(crate) async fn wait(deadline: StatusDeadline) -> UiEvent {
    let millis = match deadline {
        StatusDeadline::Presence(_) => 2000,
        StatusDeadline::Thought(_) => 200,
    };
    tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
    UiEvent::StatusDeadline(deadline)
}

