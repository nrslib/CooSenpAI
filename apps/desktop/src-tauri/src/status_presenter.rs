use crate::snapshot::AppSnapshot;
use crate::snapshot::ObserverErrorIdentity;
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
    pub dismissible: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusView {
    pub presence: PresenceView,
    pub banner: Option<BannerView>,
    pub observer_error: Option<ObserverErrorView>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ObserverErrorView {
    pub message: UiText,
    pub occurrence: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct ObserverErrorIncident {
    identity: ObserverErrorIdentity,
    occurred_at: Option<String>,
    occurrence: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThoughtWindowView {
    pub thought: Option<ThoughtView>,
    pub bubble_tail: bool,
    pub theme: String,
    pub font: String,
    pub language: String,
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum StatusDeadline {
    Presence(u64),
    Thought(u64),
    ThoughtWindowRetry { generation: u64, delay_ms: u64 },
}
pub(crate) struct StatusPresenter {
    view: StatusView,
    thought: Option<ThoughtView>,
    thought_conversation_generation: Option<u64>,
    thought_theme: String,
    thought_font: String,
    thought_language: String,
    main_visible: bool,
    main_on_active_space: bool,
    main_geometry: Option<crate::window_thought::MainWindowGeometry>,
    thought_position: Option<crate::window_thought::ScreenPoint>,
    thought_window_visible: bool,
    thought_window_state_unknown: bool,
    thought_window_inspection_pending: bool,
    thought_window_operation_pending: bool,
    thought_window_retry_generation: u64,
    thought_window_retry_failures: u8,
    thought_bubble_tail: bool,
    thought_render_after_position: bool,
    initialized: bool,
    candidate: Option<PresenceView>,
    presence_generation: u64,
    thought_generation: u64,
    dismissed_banner: Option<BannerView>,
    banner_error_at: Option<String>,
    active_observer_error: Option<ObserverErrorIncident>,
    dismissed_observer_error: Option<ObserverErrorIncident>,
    observer_error_occurrences: BTreeMap<ObserverErrorIdentity, u64>,
}
impl Default for StatusPresenter {
    fn default() -> Self {
        Self {
            view: StatusView {
                presence: PresenceView {
                    mode: "resting",
                    text: UiText::message("view.resting"),
                },
                banner: None,
                observer_error: None,
            },
            thought: None,
            thought_conversation_generation: None,
            thought_theme: "system".into(),
            thought_font: "system".into(),
            thought_language: "ja".into(),
            main_visible: false,
            main_on_active_space: true,
            main_geometry: None,
            thought_position: None,
            thought_window_visible: false,
            thought_window_state_unknown: false,
            thought_window_inspection_pending: false,
            thought_window_operation_pending: false,
            thought_window_retry_generation: 0,
            thought_window_retry_failures: 0,
            thought_bubble_tail: false,
            thought_render_after_position: false,
            initialized: false,
            candidate: None,
            presence_generation: 0,
            thought_generation: 0,
            dismissed_banner: None,
            banner_error_at: None,
            active_observer_error: None,
            dismissed_observer_error: None,
            observer_error_occurrences: BTreeMap::new(),
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
        let previous_thought = self.thought.clone();
        let previous_thought_conversation_generation = self.thought_conversation_generation;
        let previous_appearance = (
            self.thought_theme.clone(),
            self.thought_font.clone(),
            self.thought_language.clone(),
        );
        self.thought_theme = snapshot.config.ui.theme.clone();
        self.thought_font = snapshot.config.ui.font.clone();
        self.thought_language = snapshot.config.ui.language.clone();
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
        let conversation_generation = snapshot.selected_conversation_generation;
        let thought_generation_changed = self
            .thought_conversation_generation
            .is_some_and(|previous| previous != conversation_generation);
        if !snapshot.config.ui.thought_bubble {
            if self.thought.take().is_some() {
                self.thought_generation += 1;
            }
            self.thought_conversation_generation = None;
        } else if thought_generation_changed {
            if self.thought.take().is_some() {
                self.thought_generation += 1;
            }
            self.thought_conversation_generation = None;
            if let Some(text) = thought(snapshot) {
                self.thought_generation += 1;
                self.thought = Some(ThoughtView {
                    text,
                    leaving: false,
                });
                self.thought_conversation_generation = Some(conversation_generation);
            }
        } else {
            match thought(snapshot) {
                Some(text) => {
                    self.thought_generation += 1;
                    self.thought = Some(ThoughtView {
                        text,
                        leaving: false,
                    });
                    self.thought_conversation_generation = Some(conversation_generation);
                }
                None => {
                    if let Some(displayed) = &mut self.thought {
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
        }
        let error_at = snapshot
            .last_error
            .as_ref()
            .map(|error| error.occurred_at.clone());
        if self.banner_error_at != error_at {
            self.dismissed_banner = None;
            self.banner_error_at = error_at;
        }
        let observer_error_identity = observer_error_identity(snapshot);
        let observer_error_key = observer_error_identity
            .as_ref()
            .map(|identity| (identity, snapshot.observer.error_occurred_at.as_ref()));
        if self
            .active_observer_error
            .as_ref()
            .map(|incident| (&incident.identity, incident.occurred_at.as_ref()))
            != observer_error_key
        {
            self.active_observer_error = observer_error_identity.map(|identity| {
                let occurrence = self
                    .observer_error_occurrences
                    .entry(identity.clone())
                    .and_modify(|count| *count = count.saturating_add(1))
                    .or_insert(1);
                ObserverErrorIncident {
                    identity,
                    occurred_at: snapshot.observer.error_occurred_at.clone(),
                    occurrence: *occurrence,
                }
            });
        }
        if self.active_observer_error != self.dismissed_observer_error
            && self.dismissed_observer_error.is_some()
        {
            self.dismissed_observer_error = None;
        }
        let observer_error_view =
            self.active_observer_error
                .as_ref()
                .map(|incident| ObserverErrorView {
                    message: observer_error(snapshot)
                        .expect("active observer error has a projection"),
                    occurrence: incident.occurrence,
                });
        let observer_error_banner = observer_error_view.as_ref().map(observer_error_banner);
        let next_banner = error
            .map(|error| banner("error", UiText::literal(error), None))
            .or_else(|| status_banner(snapshot));
        let next_banner = next_banner.map(|banner| {
            with_observer_error_occurrence(
                banner,
                self.active_observer_error
                    .as_ref()
                    .map_or(0, |incident| incident.occurrence),
            )
        });
        let is_dismissed_observer_error = self.active_observer_error
            == self.dismissed_observer_error
            && self.dismissed_observer_error.is_some()
            && next_banner == observer_error_banner;
        if next_banner != self.dismissed_banner || is_dismissed_observer_error {
            if next_banner != self.dismissed_banner {
                self.dismissed_banner = None;
            }
            self.view.banner = if is_dismissed_observer_error {
                None
            } else {
                next_banner
            };
        } else {
            self.view.banner = None;
        }
        self.view.observer_error = if self.active_observer_error == self.dismissed_observer_error
            && self.dismissed_observer_error.is_some()
        {
            None
        } else {
            observer_error_view
        };
        let thought_projection_changed = previous_thought != self.thought
            || previous_thought_conversation_generation != self.thought_conversation_generation
            || previous_appearance
                != (
                    self.thought_theme.clone(),
                    self.thought_font.clone(),
                    self.thought_language.clone(),
                );
        effects.insert(0, self.render());
        if thought_projection_changed {
            self.restart_thought_window_retry_cycle();
            effects.extend(self.sync_thought_window(true));
        }
        effects
    }
    pub(crate) fn deadline(&mut self, deadline: StatusDeadline) -> Vec<UiEffect> {
        let thought_changed = matches!(deadline, StatusDeadline::Thought(_));
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
                    && self.thought.as_ref().is_some_and(|view| view.leaving) =>
            {
                self.thought = None
            }
            StatusDeadline::ThoughtWindowRetry { generation, .. }
                if generation == self.thought_window_retry_generation =>
            {
                return self.sync_thought_window(false);
            }
            _ => return vec![],
        }
        let mut effects = vec![self.render()];
        if thought_changed {
            effects.extend(self.sync_thought_window(true));
        }
        effects
    }
    pub(crate) fn recovery(&self) -> Option<RecoveryAction> {
        self.view.banner.as_ref().and_then(|view| view.action)
    }
    pub(crate) fn dismiss_banner(&mut self) -> Vec<UiEffect> {
        if self
            .view
            .banner
            .as_ref()
            .is_some_and(|banner| banner.dismissible)
        {
            let observer_error_banner_is_visible =
                self.view
                    .observer_error
                    .as_ref()
                    .is_some_and(|observer_error| {
                        Some(observer_error_banner(observer_error)) == self.view.banner
                    });
            if observer_error_banner_is_visible {
                self.dismissed_observer_error = self.active_observer_error.clone();
                self.view.observer_error = None;
            } else {
                self.dismissed_banner = self.view.banner.clone();
            }
            self.view.banner = None;
            vec![self.render()]
        } else {
            vec![]
        }
    }
    pub(crate) fn render(&self) -> UiEffect {
        UiEffect::StatusRender(Box::new(self.view.clone()))
    }
    pub(crate) fn thought_window_mounted(&mut self) -> Vec<UiEffect> {
        self.sync_thought_window(true)
    }
    pub(crate) fn main_visibility(&mut self, visible: bool) -> Vec<UiEffect> {
        if self.main_visible == visible {
            return vec![];
        }
        self.main_visible = visible;
        self.restart_thought_window_retry_cycle();
        self.sync_thought_window(true)
    }
    pub(crate) fn main_active_space_changed(&mut self, active: bool) -> Vec<UiEffect> {
        if self.main_on_active_space == active {
            return vec![];
        }
        self.main_on_active_space = active;
        self.restart_thought_window_retry_cycle();
        self.sync_thought_window(true)
    }
    pub(crate) fn main_geometry_changed(
        &mut self,
        geometry: crate::window_thought::MainWindowGeometry,
    ) -> Vec<UiEffect> {
        let changed = self.main_geometry != Some(geometry);
        let should_render = self.thought_window_should_be_visible() && !self.thought_window_visible;
        self.main_geometry = Some(geometry);
        if changed {
            self.restart_thought_window_retry_cycle();
        }
        self.sync_thought_window(should_render)
    }
    fn render_thought(&self) -> UiEffect {
        UiEffect::ThoughtRender(Box::new(ThoughtWindowView {
            thought: self.thought.clone(),
            bubble_tail: self.thought_bubble_tail,
            theme: self.thought_theme.clone(),
            font: self.thought_font.clone(),
            language: self.thought_language.clone(),
        }))
    }
    fn thought_window_should_be_visible(&self) -> bool {
        self.main_visible
            && self.main_on_active_space
            && self.main_geometry.is_some()
            && self.thought.is_some()
    }
    fn sync_thought_window(&mut self, render: bool) -> Vec<UiEffect> {
        if self.thought_window_operation_pending {
            if render && self.thought_window_should_be_visible() {
                self.thought_render_after_position = true;
            }
            return vec![];
        }

        if !self.thought_window_should_be_visible() {
            if self.thought_window_state_unknown {
                return self.inspect_thought_window();
            }
            if self.thought_window_visible {
                self.thought_window_operation_pending = true;
                return vec![UiEffect::ThoughtWindowHide];
            }
            self.mark_thought_window_reconciled();
            return vec![];
        }

        let mut effects = Vec::new();
        let Some(geometry) = self.main_geometry else {
            return effects;
        };
        let placement = crate::window_thought::target_placement(geometry);
        if self.thought_window_state_unknown {
            self.thought_render_after_position |= render;
            return self.inspect_thought_window();
        }
        if self.thought_window_inspection_pending {
            self.thought_render_after_position |= render;
            return effects;
        }
        let position_changed = self.thought_position != Some(placement.position);
        if position_changed {
            self.thought_render_after_position |= render;
            self.thought_window_operation_pending = true;
            effects.push(UiEffect::ThoughtWindowPosition(placement));
            return effects;
        }
        let tail_changed = self.thought_bubble_tail != placement.bubble_tail;
        if tail_changed {
            self.thought_bubble_tail = placement.bubble_tail;
        }
        let deferred_render = std::mem::take(&mut self.thought_render_after_position);
        if render || deferred_render || tail_changed {
            effects.push(self.render_thought());
        }
        if !self.thought_window_visible {
            self.thought_window_operation_pending = true;
            effects.push(UiEffect::ThoughtWindowShow);
        } else {
            self.mark_thought_window_reconciled();
        }
        effects
    }

    fn inspect_thought_window(&mut self) -> Vec<UiEffect> {
        if self.thought_window_inspection_pending || self.thought_window_operation_pending {
            return vec![];
        }
        self.thought_window_inspection_pending = true;
        vec![UiEffect::ThoughtWindowInspect]
    }

    fn mark_thought_window_reconciled(&mut self) {
        if self.thought_window_retry_failures != 0 {
            self.thought_window_retry_failures = 0;
            self.thought_window_retry_generation =
                self.thought_window_retry_generation.wrapping_add(1);
        }
    }

    fn restart_thought_window_retry_cycle(&mut self) {
        self.thought_window_retry_failures = 0;
        self.thought_window_retry_generation = self.thought_window_retry_generation.wrapping_add(1);
    }

    fn native_operation_failed(&mut self, error: String) -> Vec<UiEffect> {
        self.thought_window_state_unknown = true;
        self.thought_window_inspection_pending = false;
        self.thought_window_operation_pending = false;
        self.thought_window_retry_failures = self.thought_window_retry_failures.saturating_add(1);
        self.thought_window_retry_generation = self.thought_window_retry_generation.wrapping_add(1);

        let mut effects = Vec::new();
        if self.thought_window_retry_failures == 1 {
            effects.push(UiEffect::Log(format!(
                "思考吹き出しの native 操作に失敗しました: {error}"
            )));
        }
        if self.thought_window_retry_failures >= THOUGHT_WINDOW_RETRY_LIMIT {
            effects.push(UiEffect::Log(format!(
                "思考吹き出しの native 操作を{}回失敗したため再試行を停止しました: {error}",
                THOUGHT_WINDOW_RETRY_LIMIT
            )));
            return effects;
        }

        effects.push(UiEffect::Spawn(UiTask::StatusDelay(
            StatusDeadline::ThoughtWindowRetry {
                generation: self.thought_window_retry_generation,
                delay_ms: thought_window_retry_delay_ms(self.thought_window_retry_failures),
            },
        )));
        effects
    }

    pub(crate) fn thought_window_operation_completed(
        &mut self,
        operation: crate::window_thought::ThoughtWindowOperation,
        result: Result<(), String>,
    ) -> Vec<UiEffect> {
        if let Err(error) = result {
            return self.native_operation_failed(error);
        }

        self.thought_window_operation_pending = false;
        let mut effects = Vec::new();
        match operation {
            crate::window_thought::ThoughtWindowOperation::Position(placement) => {
                self.thought_position = Some(placement.position);
                let tail_changed = self.thought_bubble_tail != placement.bubble_tail;
                let render = std::mem::take(&mut self.thought_render_after_position)
                    && self.thought_window_should_be_visible();
                if tail_changed {
                    self.thought_bubble_tail = placement.bubble_tail;
                }
                if self.thought_window_should_be_visible() && (tail_changed || render) {
                    effects.push(self.render_thought());
                }
            }
            crate::window_thought::ThoughtWindowOperation::Show => {
                self.thought_window_visible = true;
            }
            crate::window_thought::ThoughtWindowOperation::Hide => {
                self.thought_window_visible = false;
            }
        }
        effects.extend(self.sync_thought_window(false));
        effects
    }

    pub(crate) fn thought_window_state_observed(
        &mut self,
        result: Result<crate::window_thought::ThoughtWindowNativeState, String>,
    ) -> Vec<UiEffect> {
        self.thought_window_inspection_pending = false;
        match result {
            Ok(state) => {
                self.thought_window_state_unknown = false;
                self.thought_window_visible = state.visible;
                self.thought_position = Some(state.position);
                self.sync_thought_window(false)
            }
            Err(error) => self.native_operation_failed(error),
        }
    }
}

const THOUGHT_WINDOW_RETRY_LIMIT: u8 = 8;

fn thought_window_retry_delay_ms(failures: u8) -> u64 {
    200_u64
        .saturating_mul(1_u64 << u32::from(failures.saturating_sub(1).min(5)))
        .min(5_000)
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
fn observer_error_identity(s: &AppSnapshot) -> Option<ObserverErrorIdentity> {
    observer_error(s).map(|_| {
        s.observer
            .error_identity
            .clone()
            .unwrap_or_else(ObserverErrorIdentity::unknown)
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
    } else if s
        .last_error
        .as_ref()
        .is_some_and(|error| !error.is_user_response_error())
        || s.delivery_outbox_blocked
    {
        ("attention", UiText::message("view.needsAttention"))
    } else if s.observer_running {
        ("watching", UiText::message("view.watching"))
    } else {
        ("resting", UiText::message("view.resting"))
    };
    PresenceView { mode, text }
}
fn thought(s: &AppSnapshot) -> Option<UiText> {
    if !s.config.ui.thought_bubble
        || s.latest_companion_thought_generation != Some(s.selected_conversation_generation)
    {
        return None;
    }
    let raw = s.latest_companion_thought.as_deref()?;
    let chars: Vec<_> = raw.chars().collect();
    Some(UiText::literal(if chars.len() <= 240 {
        raw.to_owned()
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
        dismissible: matches!(tone, "error" | "warning")
            && action != Some(RecoveryAction::SystemAudio),
    }
}
fn observer_error_banner(error: &ObserverErrorView) -> BannerView {
    with_observer_error_occurrence(
        banner(
            "error",
            UiText::message("view.observerFailed").arg("message", error.message.clone()),
            Some(RecoveryAction::Settings),
        ),
        error.occurrence,
    )
}
fn with_observer_error_occurrence(mut banner: BannerView, occurrence: u64) -> BannerView {
    if matches!(
        &banner.message,
        UiText::Message {
            key: "view.observerFailed",
            ..
        }
    ) {
        banner.message = UiText::Join {
            parts: vec![
                banner.message,
                UiText::message("view.observerErrorOccurrence")
                    .arg("count", UiText::literal(occurrence)),
            ],
        };
    }
    banner
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
        Some("speaker-protocol" | "hearing-protocol") => return None,
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
    if let (Some(error), Some(seconds)) = (&s.last_error, s.companion_retry_in_seconds) {
        let key = if error.kind == coosenpai_core::runtime::RuntimeErrorKind::ProviderTimeout {
            "view.companionRetryTimeout"
        } else {
            "view.companionRetry"
        };
        return Some(banner(
            "info",
            Text::message(key)
                .arg("name", Text::literal(&s.companion_display_name))
                .arg("kind", Text::literal(error.kind.as_str()))
                .arg("seconds", Text::literal(seconds)),
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
    s.last_error
        .as_ref()
        .filter(|error| !error.is_user_response_error())
        .map(|error| {
            let key = if error.kind == coosenpai_core::runtime::RuntimeErrorKind::ProviderTimeout {
                "view.companionFailedTimeout"
            } else {
                "view.companionFailed"
            };
            banner(
                "info",
                Text::message(key)
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
        StatusDeadline::ThoughtWindowRetry { delay_ms, .. } => delay_ms,
    };
    tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
    UiEvent::StatusDeadline(deadline)
}

pub(crate) fn user_response_failure_text(
    failure: &coosenpai_core::runtime::RuntimeUserResponseFailure,
    kind: coosenpai_core::runtime::RuntimeErrorKind,
) -> UiText {
    let key = if kind == coosenpai_core::runtime::RuntimeErrorKind::ProviderTimeout {
        "view.userResponseStoppedTimeout"
    } else {
        "view.userResponseStopped"
    };
    let stopped = UiText::message(key).arg("attempts", UiText::literal(failure.attempts));
    let Some(provider) = &failure.provider else {
        return stopped;
    };
    use coosenpai_core::provider::ProviderErrorKind;
    let key = match provider.kind {
        ProviderErrorKind::Retryable => "view.providerFailure.retryable",
        ProviderErrorKind::Timeout => "view.providerFailure.timeout",
        ProviderErrorKind::Auth => "view.providerFailure.auth",
        ProviderErrorKind::Unsupported => "view.providerFailure.unsupported",
        ProviderErrorKind::InvalidModel => "view.providerFailure.invalid-model",
        ProviderErrorKind::InvalidRequest => "view.providerFailure.invalid-request",
        ProviderErrorKind::Permission => "view.providerFailure.permission",
        ProviderErrorKind::Quota => "view.providerFailure.quota",
        ProviderErrorKind::RateLimit => "view.providerFailure.rate-limit",
        ProviderErrorKind::InvalidOutput => "view.providerFailure.invalid-output",
    };
    let reason = UiText::message(key);
    let reason = match &provider.model {
        Some(model) => UiText::message("view.providerFailureModel")
            .arg("model", UiText::literal(model))
            .arg("reason", reason),
        None => reason,
    };
    UiText::Join {
        parts: vec![reason, UiText::literal(" "), stopped],
    }
}
