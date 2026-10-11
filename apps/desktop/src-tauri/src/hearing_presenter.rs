use crate::snapshot::{
    AppSnapshot, AudioLogEvent, AudioLogStage, AudioObservationView, AudioView, SpeakerCapture,
};
use coosenpai_core::locale::{localize_audio_message, speaker_missing_device_count, Locale};
use coosenpai_core::ports::{HearingSourceState, SpeechPermissionKind};
use coosenpai_core::state::{AudioObservation, AudioObservationSource};

#[derive(Debug)]
pub(crate) enum HearingResult {
    SpeakerResumed,
    Cancelled(u64),
    Inactive,
    Started(u64),
    PermissionLoaded {
        generation: u64,
        recognition: SpeechPermissionKind,
    },
    Ready {
        generation: u64,
        microphone: SpeechPermissionKind,
        recognition: SpeechPermissionKind,
        ready_sources: Vec<AudioObservationSource>,
    },
    SourceState {
        generation: u64,
        source: AudioObservationSource,
        state: HearingSourceState,
    },
    Recovered {
        generation: u64,
        source: AudioObservationSource,
        final_result: bool,
    },
    Warning {
        generation: u64,
        kind: String,
        message: String,
    },
    Recognition {
        generation: u64,
        created_at: String,
        source: AudioObservationSource,
        stage: AudioLogStage,
    },
    Observed {
        generation: u64,
        observation: AudioObservation,
        transcript_path: Option<String>,
    },
    Analyzed {
        generation: u64,
        observation: coosenpai_core::state::ObservationRecord,
        calls: u32,
    },
    Stopping(u64),
    Stopped(u64),
    Restarted(u64),
    Failed {
        generation: Option<u64>,
        kind: String,
        message: String,
        recognition: Option<SpeechPermissionKind>,
    },
}

fn can_replace_protocol_warning(current: Option<&str>, incoming: &str) -> bool {
    match current {
        Some("hearing-protocol") => incoming == "hearing-protocol",
        Some("speaker-protocol") => incoming == "speaker-protocol",
        Some("speaker-device-missing") => incoming == "speaker-device-missing",
        _ => true,
    }
}

fn clear_warning_after_stop(view: &mut AudioView, audio_enabled: bool) {
    if audio_enabled
        && matches!(view.speaker_capture, SpeakerCapture::Stopped { ref reason, .. } if !reason.is_empty())
    {
        return;
    }
    view.warning_kind = None;
    view.message = None;
    view.missing_speaker_device_count = None;
}

pub(crate) fn adopt(snapshot: &mut AppSnapshot, result: HearingResult) -> bool {
    let locale = Locale::from_config(&snapshot.config.ui.language);
    let view = &mut snapshot.audio;
    match result {
        HearingResult::SpeakerResumed
            if snapshot.config.audio.enabled && snapshot.config.audio.speaker =>
        {
            view.speaker_capture = SpeakerCapture::Starting;
            view.speaker_input_generations = None;
            if view.warning_kind.as_deref().is_some_and(|kind| {
                kind == "speaker-device-missing"
                    || kind == "speaker-device-waiting"
                    || kind == "screen-capture"
                    || kind == "system-audio"
                    || kind.starts_with("system-audio-")
            }) {
                view.warning_kind = None;
                view.message = None;
                view.missing_speaker_device_count = None;
            }
        }
        HearingResult::Inactive => {
            view.phase = "off".to_owned();
            view.speaker_input_generations = None;
            if !snapshot.config.audio.enabled {
                view.speaker_capture = SpeakerCapture::stopped_without_reason();
            } else if !matches!(view.speaker_capture, SpeakerCapture::Stopped { .. }) {
                view.speaker_capture = SpeakerCapture::Paused;
            }
            clear_warning_after_stop(view, snapshot.config.audio.enabled);
        }
        HearingResult::Cancelled(generation) if view.generation == generation => {
            view.phase = "off".to_owned();
            view.speaker_input_generations = None;
            if !matches!(view.speaker_capture, SpeakerCapture::Stopped { .. }) {
                view.speaker_capture = SpeakerCapture::Paused;
            }
            clear_warning_after_stop(view, snapshot.config.audio.enabled);
        }
        HearingResult::Started(generation) => {
            view.generation = generation;
            view.phase = "starting".to_owned();
            view.speaker_input_generations = None;
            if snapshot.config.audio.speaker
                && !matches!(view.speaker_capture, SpeakerCapture::Restarting { .. })
                && !matches!(view.speaker_capture, SpeakerCapture::Stopped { ref reason, .. } if !reason.is_empty())
            {
                view.speaker_capture = SpeakerCapture::Starting;
            }
        }
        HearingResult::PermissionLoaded {
            generation,
            recognition,
        } if snapshot.config.audio.enabled && view.generation == generation => {
            view.recognition_permission = crate::speech::permission_name(recognition);
        }
        HearingResult::SourceState {
            generation,
            source,
            state,
        } if snapshot.config.audio.enabled
            && view.generation == generation
            && view.phase != "stopping"
            && view.phase != "off" =>
        {
            if source == AudioObservationSource::Speaker && snapshot.config.audio.speaker {
                match state {
                    HearingSourceState::Ready { input_generation }
                        if !matches!(view.speaker_capture, SpeakerCapture::Stopped { .. }) =>
                    {
                        view.accept_speaker_input_generation(input_generation);
                        if matches!(
                            view.speaker_capture,
                            SpeakerCapture::Starting
                                | SpeakerCapture::Waiting { .. }
                                | SpeakerCapture::Restarting { .. }
                        ) {
                            view.speaker_capture = SpeakerCapture::Running;
                            if view.warning_kind.as_deref() == Some("speaker-device-waiting") {
                                view.warning_kind = None;
                                view.message = None;
                                view.missing_speaker_device_count = None;
                            }
                        }
                    }
                    HearingSourceState::Waiting { kind, message }
                        if !matches!(view.speaker_capture, SpeakerCapture::Stopped { .. }) =>
                    {
                        view.speaker_capture = SpeakerCapture::waiting(&kind, &message, locale);
                    }
                    HearingSourceState::Failed {
                        kind,
                        message,
                        retrying,
                    } if !matches!(view.speaker_capture, SpeakerCapture::Stopped { .. }) => {
                        view.speaker_input_generations = None;
                        view.speaker_capture = if retrying {
                            SpeakerCapture::restarting(&kind, &message, locale)
                        } else {
                            SpeakerCapture::stopped(&kind, &message, locale)
                        };
                    }
                    _ => {}
                }
            }
        }
        HearingResult::Ready {
            generation,
            microphone,
            recognition,
            ready_sources,
        } if snapshot.config.audio.enabled
            && view.generation == generation
            && view.phase != "stopping"
            && view.phase != "off" =>
        {
            view.phase = "listening".to_owned();
            if snapshot.config.audio.speaker
                && ready_sources.contains(&AudioObservationSource::Speaker)
                && matches!(
                    view.speaker_capture,
                    SpeakerCapture::Starting
                        | SpeakerCapture::Waiting { .. }
                        | SpeakerCapture::Restarting { .. }
                )
            {
                view.speaker_capture = SpeakerCapture::Running;
                if view.warning_kind.as_deref() == Some("speaker-device-waiting") {
                    view.warning_kind = None;
                    view.message = None;
                    view.missing_speaker_device_count = None;
                }
            }
            view.microphone_permission = crate::speech::permission_name(microphone);
            view.recognition_permission = crate::speech::permission_name(recognition);
        }
        HearingResult::Recovered {
            generation,
            source,
            final_result,
        } if view.generation == generation
            && matches!(view.phase.as_str(), "starting" | "listening") =>
        {
            let clears_warning = match view.warning_kind.as_deref() {
                Some("speaker-protocol") => {
                    final_result && source == AudioObservationSource::Speaker
                }
                Some("hearing-protocol") => {
                    final_result && source == AudioObservationSource::Microphone
                }
                Some("helper" | "helper-closed" | "helper-start") => true,
                _ => false,
            };
            if clears_warning {
                view.message = None;
                view.warning_kind = None;
                view.missing_speaker_device_count = None;
            }
        }
        HearingResult::Warning { generation, .. }
            if view.generation == generation
                && matches!(
                    view.warning_kind.as_deref(),
                    Some("system-audio-exhausted" | "helper-exhausted")
                ) => {}
        HearingResult::Warning {
            generation,
            kind,
            message,
        } if view.generation == generation && view.phase != "stopping" && view.phase != "off" => {
            if kind == "system-audio-restored" {
                if view.warning_kind.as_deref().is_some_and(|kind| {
                    kind == "speaker-device-missing"
                        || kind == "screen-capture"
                        || kind == "system-audio"
                        || kind.starts_with("system-audio-")
                }) {
                    view.message = None;
                    view.warning_kind = None;
                    view.missing_speaker_device_count = None;
                }
            } else if matches!(kind.as_str(), "system-audio-exhausted" | "helper-exhausted") {
                view.speaker_input_generations = None;
                view.speaker_capture = SpeakerCapture::stopped(&kind, &message, locale);
                view.message = Some(localize_audio_message(&kind, &message, locale));
                view.warning_kind = Some(kind);
            } else if kind == "speaker-device-waiting" {
                if !matches!(view.speaker_capture, SpeakerCapture::Stopped { .. }) {
                    view.speaker_capture = SpeakerCapture::waiting(&kind, &message, locale);
                }
                view.missing_speaker_device_count = speaker_missing_device_count(&message);
                view.message = Some(localize_audio_message(&kind, &message, locale));
                view.warning_kind = Some(kind);
            } else if kind == "screen-capture" {
                let reason = localize_audio_message(&kind, &message, locale);
                if !matches!(view.speaker_capture, SpeakerCapture::Restarting { .. }) {
                    view.speaker_capture = SpeakerCapture::stopped(&kind, &message, locale);
                }
                view.message = Some(reason);
                view.warning_kind = Some(kind);
            } else if kind == "speaker-device-missing" {
                if matches!(view.speaker_capture, SpeakerCapture::Starting) {
                    view.speaker_capture = SpeakerCapture::restarting(&kind, &message, locale);
                }
                view.missing_speaker_device_count = speaker_missing_device_count(&message);
                view.message = Some(localize_audio_message(&kind, &message, locale));
                view.warning_kind = Some(kind);
            } else if kind.starts_with("system-audio-") {
                if !matches!(view.speaker_capture, SpeakerCapture::Stopped { .. }) {
                    view.speaker_capture = SpeakerCapture::restarting(&kind, &message, locale);
                }
            } else {
                if kind == "speaker-protocol"
                    && !matches!(view.speaker_capture, SpeakerCapture::Stopped { .. })
                {
                    view.speaker_capture = SpeakerCapture::restarting(&kind, &message, locale);
                }
                if can_replace_protocol_warning(view.warning_kind.as_deref(), &kind) {
                    view.missing_speaker_device_count = (kind == "speaker-device-missing")
                        .then(|| speaker_missing_device_count(&message))
                        .flatten();
                    view.message = Some(localize_audio_message(&kind, &message, locale));
                    view.warning_kind = Some(kind);
                }
            }
        }
        HearingResult::Recognition {
            generation,
            created_at,
            source,
            stage,
        } if view.generation == generation
            && matches!(view.phase.as_str(), "starting" | "listening")
            && (source != AudioObservationSource::Speaker
                || matches!(view.speaker_capture, SpeakerCapture::Running)) =>
        {
            view.push_event(AudioLogEvent {
                id: uuid::Uuid::new_v4().to_string(),
                generation,
                created_at,
                source,
                transcript_path: None,
                stage,
            });
        }
        HearingResult::Observed {
            generation,
            observation,
            transcript_path,
        } => {
            if view
                .latest_observation
                .as_ref()
                .is_none_or(|latest| latest.created_at <= observation.created_at)
            {
                view.latest_observation =
                    Some(AudioObservationView::from_observation(&observation));
            }
            view.push_event(AudioLogEvent {
                id: observation.id,
                generation,
                created_at: observation.created_at,
                source: observation.source,
                transcript_path,
                stage: AudioLogStage::Confirmed {
                    text: observation.text,
                },
            });
        }
        HearingResult::Analyzed {
            generation,
            observation,
            calls,
        } if view.generation == generation && view.phase == "listening" => {
            snapshot.observer.record_observation(observation);
            snapshot.observer.ai_calls_today = calls;
        }
        HearingResult::Stopping(generation) if view.generation == generation => {
            view.phase = "stopping".to_owned();
            view.speaker_input_generations = None;
            if !matches!(view.speaker_capture, SpeakerCapture::Stopped { .. }) {
                view.speaker_capture = SpeakerCapture::Paused;
            }
        }
        HearingResult::Stopped(generation)
            if view.generation == generation && view.phase == "stopping" =>
        {
            view.phase = "off".to_owned();
            view.speaker_input_generations = None;
            if !matches!(view.speaker_capture, SpeakerCapture::Stopped { .. }) {
                view.speaker_capture = SpeakerCapture::Paused;
            }
            clear_warning_after_stop(view, snapshot.config.audio.enabled);
        }
        HearingResult::Restarted(generation)
            if snapshot.config.audio.enabled
                && view.generation == generation
                && view.phase != "stopping"
                && view.phase != "off" =>
        {
            view.phase = "starting".to_owned()
        }
        HearingResult::Failed {
            generation,
            kind,
            message,
            recognition,
        } if snapshot.config.audio.enabled
            && generation.is_none_or(|expected| view.generation == expected) =>
        {
            view.speaker_input_generations = None;
            match kind.as_str() {
                "permission-microphone" => view.microphone_permission = "denied".to_owned(),
                "permission-speech" => {
                    view.recognition_permission = crate::speech::permission_name(
                        recognition.unwrap_or(SpeechPermissionKind::Denied),
                    );
                }
                _ => {}
            }
            if snapshot.config.audio.speaker
                && !matches!(view.speaker_capture, SpeakerCapture::Stopped { ref reason, .. } if !reason.is_empty())
            {
                view.speaker_capture = if crate::hearing::retryable_audio_error(&kind) {
                    SpeakerCapture::restarting(&kind, &message, locale)
                } else {
                    SpeakerCapture::stopped(&kind, &message, locale)
                };
            }
            if matches!(
                view.warning_kind.as_deref(),
                Some("system-audio-exhausted" | "helper-exhausted")
            ) || matches!(kind.as_str(), "system-audio-exhausted" | "helper-exhausted")
            {
                view.phase = "error".to_owned();
            } else if can_replace_protocol_warning(view.warning_kind.as_deref(), &kind) {
                view.phase = "error".to_owned();
                view.missing_speaker_device_count = (kind == "speaker-device-missing")
                    .then(|| speaker_missing_device_count(&message))
                    .flatten();
                view.message = Some(localize_audio_message(&kind, &message, locale));
                view.warning_kind = Some(kind);
            }
        }
        _ => return false,
    }
    true
}
