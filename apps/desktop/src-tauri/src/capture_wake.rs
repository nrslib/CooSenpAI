use super::{CapturePresenter, CaptureState};
use crate::capture::effects::CaptureEffect;
use crate::ui_events::{
    UiEvent, VoiceAction, WakeFinalDisposition, WakeIdentity, WakeIdleIdentity,
};
use coosenpai_core::ports::HearingSegmentEndReason;
use tokio::sync::oneshot;

pub(super) struct WakeSession {
    identity: WakeIdentity,
    pub(super) display_name: String,
    locale: String,
    text: String,
    pending_partial: Option<String>,
    pending_final: Option<String>,
    awaiting_next: bool,
    following_partial: bool,
    waiting_for_idle: bool,
    final_sent: bool,
    pub(super) cancelled: bool,
}

fn transcript_separator(locale: &str) -> &'static str {
    let language = locale
        .split(['-', '_'])
        .next()
        .unwrap_or(locale)
        .to_ascii_lowercase();
    match language.as_str() {
        "ja" | "zh" | "th" | "lo" | "km" | "my" => "",
        _ => " ",
    }
}

fn join_transcript(prefix: &str, suffix: &str, locale: &str) -> String {
    if prefix.is_empty() {
        return suffix.to_owned();
    }
    if suffix.is_empty() {
        return prefix.to_owned();
    }
    let boundary =
        if prefix.ends_with(char::is_whitespace) || suffix.starts_with(char::is_whitespace) {
            ""
        } else {
            transcript_separator(locale)
        };
    format!("{prefix}{boundary}{suffix}")
}

fn same_stream(
    identity: &WakeIdentity,
    session_id: &str,
    source: coosenpai_core::state::AudioObservationSource,
) -> bool {
    identity.session_id == session_id && identity.source == source
}

impl CapturePresenter {
    pub(super) fn wake_recognizing(
        &mut self,
        identity: WakeIdentity,
        display_name: String,
        text: String,
        reply: oneshot::Sender<bool>,
    ) -> bool {
        let started = self.accept_wake_recognizing(identity, display_name, text);
        let _ = reply.send(started);
        started
    }

    fn accept_wake_recognizing(
        &mut self,
        identity: WakeIdentity,
        display_name: String,
        text: String,
    ) -> bool {
        if let Some(wake) = &mut self.wake {
            if wake.display_name != display_name || wake.final_sent || wake.cancelled {
                return false;
            }
            if wake.awaiting_next {
                let newer_segment =
                    identity.recognition_generation > wake.identity.recognition_generation;
                let newer_partial = wake.following_partial
                    && identity.recognition_generation == wake.identity.recognition_generation
                    && identity.sequence > wake.identity.sequence;
                if !same_stream(&identity, &wake.identity.session_id, wake.identity.source)
                    || (!newer_segment && !newer_partial)
                {
                    return false;
                }
                wake.waiting_for_idle = false;
                wake.following_partial = true;
                wake.pending_partial = Some(join_transcript(&wake.text, &text, &wake.locale));
                wake.identity = identity;
            } else {
                if !wake.identity.same_segment(&identity)
                    || identity.sequence <= wake.identity.sequence
                {
                    return false;
                }
                wake.identity.sequence = identity.sequence;
                let Some(matched) = crate::capture::match_wake(&text, &display_name) else {
                    return false;
                };
                wake.pending_partial = Some(matched.text);
            }
            self.flush_wake_input();
            return false;
        }
        if !matches!(self.state, CaptureState::Idle) {
            return false;
        }
        let Some(matched) = crate::capture::match_wake(&text, &display_name) else {
            return false;
        };
        if !matched.can_start() {
            return false;
        }
        self.pending_wake = Some(WakeSession {
            identity,
            display_name,
            locale: String::new(),
            text: String::new(),
            pending_partial: Some(matched.text),
            pending_final: None,
            awaiting_next: false,
            following_partial: false,
            waiting_for_idle: false,
            final_sent: false,
            cancelled: false,
        });
        true
    }

    pub(super) fn wake_start(&mut self, identity: WakeIdentity, accepted: bool) -> bool {
        if self
            .pending_wake
            .as_ref()
            .is_none_or(|wake| wake.identity != identity)
        {
            return false;
        }
        let Some(pending) = self.pending_wake.take() else {
            return false;
        };
        if !accepted || !matches!(self.state, CaptureState::Idle) {
            return false;
        }
        self.wake = Some(pending);
        true
    }

    pub(super) fn wake_display_name(&self) -> Option<&str> {
        self.wake
            .as_ref()
            .or(self.pending_wake.as_ref())
            .map(|wake| wake.display_name.as_str())
    }

    pub(super) fn wake_final(
        &mut self,
        identity: WakeIdentity,
        display_name: String,
        text: String,
        locale: String,
        end_reason: Option<HearingSegmentEndReason>,
        reply: oneshot::Sender<WakeFinalDisposition>,
    ) {
        let disposition = self.accept_wake_final(identity, display_name, text, locale, end_reason);
        let _ = reply.send(disposition);
    }

    fn accept_wake_final(
        &mut self,
        identity: WakeIdentity,
        display_name: String,
        text: String,
        locale: String,
        end_reason: Option<HearingSegmentEndReason>,
    ) -> WakeFinalDisposition {
        if self.wake_consumed.contains(&identity) {
            return WakeFinalDisposition::Accepted;
        }
        let Some(wake) = &mut self.wake else {
            return WakeFinalDisposition::Rejected;
        };
        if wake.display_name != display_name || wake.final_sent || wake.cancelled {
            return WakeFinalDisposition::Rejected;
        }
        let initial = !wake.awaiting_next
            && wake.identity.same_segment(&identity)
            && wake.identity.sequence.checked_add(1) == Some(identity.sequence);
        let subsequent = wake.awaiting_next
            && same_stream(&identity, &wake.identity.session_id, wake.identity.source)
            && (identity.recognition_generation > wake.identity.recognition_generation
                || (wake.following_partial
                    && identity.recognition_generation == wake.identity.recognition_generation
                    && wake.identity.sequence.checked_add(1) == Some(identity.sequence)));
        if !initial && !subsequent {
            return WakeFinalDisposition::Rejected;
        }
        let wake_only = if initial {
            let Some(matched) = crate::capture::match_wake(&text, &display_name) else {
                self.cancel_wake();
                return WakeFinalDisposition::Rejected;
            };
            wake.locale = locale;
            wake.text = matched.text;
            !matched.has_body
        } else {
            wake.text = join_transcript(&wake.text, &text, &wake.locale);
            false
        };
        wake.identity = identity.clone();
        wake.awaiting_next = true;
        wake.following_partial = false;
        wake.waiting_for_idle = wake_only;
        wake.pending_partial = None;
        self.remember_wake_final(identity);
        if wake_only {
            return WakeFinalDisposition::WakeOnly;
        }
        if end_reason == Some(HearingSegmentEndReason::Maximum) {
            return WakeFinalDisposition::Accepted;
        }
        let wake = self.wake.as_mut().expect("accepted wake session");
        wake.final_sent = true;
        wake.pending_final = Some(wake.text.clone());
        self.flush_wake_input();
        WakeFinalDisposition::Accepted
    }

    fn remember_wake_final(&mut self, identity: WakeIdentity) {
        self.wake_consumed.push_back(identity);
        if self.wake_consumed.len() > 64 {
            self.wake_consumed.pop_front();
        }
    }

    pub(super) fn wake_idle(&mut self, identity: WakeIdleIdentity) {
        let current = self.wake.as_ref().is_some_and(|wake| {
            wake.awaiting_next
                && wake.waiting_for_idle
                && !wake.final_sent
                && !wake.cancelled
                && wake.identity.session_id == identity.session_id
                && wake.identity.source == identity.source
                && wake.identity.recognition_generation == identity.recognition_generation
        });
        if current {
            self.cancel_wake();
        }
    }

    pub(super) fn flush_wake_input(&mut self) {
        let Some(generation) = self.voice_snapshot.as_ref().and_then(|snapshot| {
            (snapshot.speech.phase == "recording"
                && snapshot.speech.source.as_deref() == Some("wake")
                && matches!(self.state, CaptureState::Voice { speech_generation: Some(current), .. } if current == snapshot.speech.generation))
            .then_some(snapshot.speech.generation)
        }) else {
            return;
        };
        let Some(wake) = &mut self.wake else {
            return;
        };
        let partial = wake.pending_partial.take();
        let final_text = wake.pending_final.take();
        if let Some(text) = partial {
            self.output(CaptureEffect::WakePartial { generation, text });
        }
        if let Some(text) = final_text {
            self.run(
                CaptureEffect::WakeFinal { generation, text },
                crate::capture::effects::CaptureTask::WakeFinal,
            );
        }
    }

    pub(super) fn invalidate_wake(&mut self) {
        self.pending_wake = None;
        if let Some((_, reply)) = self.pending_voice_start.take() {
            let _ = reply.send(Err("音声入力の切替を取り消しました".into()));
        }
        if matches!(self.state, CaptureState::Voice { .. }) {
            self.wake = None;
        } else if let Some(wake) = &mut self.wake {
            wake.cancelled = true;
        }
    }

    pub(super) fn cancel_wake(&mut self) {
        let active = self.wake.is_some() && matches!(self.state, CaptureState::Voice { .. });
        self.invalidate_wake();
        if active {
            self.bubble(UiEvent::Voice(VoiceAction::Cancel));
        }
    }
}
