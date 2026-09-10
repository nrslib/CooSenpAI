use crate::snapshot::{AppSnapshot, AudioLogEvent, AudioLogStage, AudioObservationView};
use coosenpai_core::locale::{localize_audio_message, Locale};
use coosenpai_core::ports::SpeechPermissionKind;
use coosenpai_core::state::{AudioObservation, AudioObservationSource};

#[derive(Debug)]
pub(crate) enum HearingResult {
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

pub(crate) fn adopt(snapshot: &mut AppSnapshot, result: HearingResult) -> bool {
    let locale = Locale::from_config(&snapshot.config.ui.language);
    let view = &mut snapshot.audio;
    match result {
        HearingResult::Inactive => {
            view.phase = "off".to_owned();
            view.warning_kind = None;
            view.message = None;
        }
        HearingResult::Cancelled(generation) if view.generation == generation => {
            view.phase = "off".to_owned();
            view.warning_kind = None;
            view.message = None;
        }
        HearingResult::Started(generation) => {
            view.generation = generation;
            view.phase = "starting".to_owned();
            view.warning_kind = None;
            view.message = None;
        }
        HearingResult::PermissionLoaded {
            generation,
            recognition,
        } if snapshot.config.audio.enabled && view.generation == generation => {
            view.recognition_permission = crate::speech::permission_name(recognition);
            snapshot.speech.recognition_permission = crate::speech::permission_name(recognition);
        }
        HearingResult::Ready {
            generation,
            microphone,
            recognition,
        } if view.generation == generation => {
            view.phase = "listening".to_owned();
            view.microphone_permission = crate::speech::permission_name(microphone);
            view.recognition_permission = crate::speech::permission_name(recognition);
        }
        HearingResult::Warning {
            generation,
            kind,
            message,
        } if view.generation == generation => {
            if kind == "system-audio-restored" {
                if view
                    .warning_kind
                    .as_deref()
                    .is_some_and(|kind| kind == "system-audio" || kind.starts_with("system-audio-"))
                {
                    view.message = None;
                    view.warning_kind = None;
                }
            } else {
                view.message = Some(localize_audio_message(&kind, &message, locale));
                view.warning_kind = Some(kind);
            }
        }
        HearingResult::Recognition {
            generation,
            created_at,
            source,
            stage,
        } if view.generation == generation && view.phase == "listening" => {
            view.push_event(AudioLogEvent {
                id: uuid::Uuid::new_v4().to_string(),
                created_at,
                source,
                stage,
            });
        }
        HearingResult::Observed {
            generation,
            observation,
        } if view.generation == generation => {
            view.latest_observation = Some(AudioObservationView::from_observation(&observation));
            view.push_event(AudioLogEvent {
                id: observation.id,
                created_at: observation.created_at,
                source: observation.source,
                stage: AudioLogStage::Confirmed {
                    text: observation.text,
                },
            });
        }
        HearingResult::Stopping(generation) if view.generation == generation => {
            view.phase = "stopping".to_owned()
        }
        HearingResult::Stopped(generation)
            if view.generation == generation && view.phase == "stopping" =>
        {
            view.phase = "off".to_owned();
            view.warning_kind = None;
            view.message = None;
        }
        HearingResult::Restarted(generation) if view.generation == generation => {
            view.phase = "listening".to_owned()
        }
        HearingResult::Failed {
            generation,
            kind,
            message,
            recognition,
        } if snapshot.config.audio.enabled
            && generation.is_none_or(|expected| view.generation == expected) =>
        {
            view.phase = "error".to_owned();
            view.message = Some(localize_audio_message(&kind, &message, locale));
            match kind.as_str() {
                "permission-microphone" => view.microphone_permission = "denied".to_owned(),
                "permission-speech" => {
                    let permission = crate::speech::permission_name(
                        recognition.unwrap_or(SpeechPermissionKind::Denied),
                    );
                    view.recognition_permission = permission.clone();
                    snapshot.speech.recognition_permission = permission;
                }
                _ => {}
            }
            view.warning_kind = Some(kind);
        }
        _ => return false,
    }
    true
}
