use crate::state::AudioObservationSource;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

#[path = "microphone_commands.rs"]
pub(crate) mod microphone_commands;
use microphone_commands::{MicrophoneCommandBatch, MicrophoneCommands};

pub const MAX_HEARING_TEXT_BYTES: usize = 4096;
pub const MAX_PENDING_AUDIO_COUNT: usize = 64;
pub const MAX_PENDING_AUDIO_BYTES: usize = 64 * 1024;
pub const AUDIO_OBSERVATION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HearingContext {
    pub session_id: String,
    pub source: AudioObservationSource,
    pub generation: u64,
    pub sequence: u64,
    pub text: String,
    pub confirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<crate::ports::HearingSpeakerMetadata>,
}

impl HearingContext {
    pub fn confirmed_audio(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::state::AudioObservation, crate::state::ObservationError> {
        use sha2::{Digest, Sha256};
        if !self.confirmed {
            return Err(crate::state::ObservationError::Invalid);
        }
        // Speech helper の Final は partial と同じ入力境界を通らない場合がある。
        // journal の契約上限に達しても一件の Final で session 全体を落とさず、
        // UTF-8 の文字境界で安全に切り詰めて保存する。
        let text = crate::state::truncate(self.text.trim(), crate::state::AUDIO_TEXT_MAX_CHARS);
        let mut record =
            crate::state::AudioObservation::from_confirmed_text(self.source, &text, now)?;
        let identity = format!("{}:{:?}:{}", self.session_id, self.source, self.generation);
        let digest = Sha256::digest(identity.as_bytes());
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&digest[..16]);
        record.id = uuid::Uuid::from_bytes(bytes).to_string();
        if let Some(speaker) = &self.speaker {
            record.apply_speaker_identification(
                &speaker.segment_id,
                speaker.audio_start_ms,
                speaker.audio_end_ms,
                speaker.speaker_id.as_deref(),
                speaker.speaker_registry_id.as_deref(),
                speaker.speaker_status,
                speaker.speaker_segments.clone(),
            )?;
        }
        Ok(record)
    }
}

/// 再開時の backlog も、一回の観察を一分以内・全文32 KiB以内の発話集合にする。
pub fn observation_batch(
    audio: Vec<crate::state::AudioObservation>,
) -> Result<Vec<crate::state::AudioObservation>, crate::state::ObservationError> {
    let Some(first) = audio.first() else {
        return Ok(Vec::new());
    };
    let end = chrono::DateTime::parse_from_rfc3339(&first.created_at)
        .map_err(|_| crate::state::ObservationError::Invalid)?
        + chrono::Duration::seconds(AUDIO_OBSERVATION_INTERVAL.as_secs() as i64);
    let mut bytes = 0;
    let mut batch = Vec::new();
    for record in audio {
        let time = chrono::DateTime::parse_from_rfc3339(&record.created_at)
            .map_err(|_| crate::state::ObservationError::Invalid)?;
        if time >= end || bytes + record.text.len() > 32 * 1024 {
            break;
        }
        bytes += record.text.len();
        batch.push(record);
    }
    Ok(batch)
}

pub fn bound_pending_audio(
    audio: Vec<crate::state::AudioObservation>,
) -> Vec<crate::state::AudioObservation> {
    let mut result = Vec::new();
    let mut bytes = 0usize;
    for record in audio {
        if result.len() >= MAX_PENDING_AUDIO_COUNT
            || bytes.saturating_add(record.text.len()) > MAX_PENDING_AUDIO_BYTES
        {
            break;
        }
        bytes = bytes.saturating_add(record.text.len());
        result.push(record);
    }
    result
}

pub(crate) fn deserialize_pending_audio<'de, D>(
    deserializer: D,
) -> Result<Vec<crate::state::AudioObservation>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    let mut ids = std::collections::HashSet::new();
    values
        .into_iter()
        .map(|value| {
            match crate::state::parse_observation(value, crate::state::DEFAULT_OBSERVATION_LIMITS)
                .map_err(serde::de::Error::custom)?
            {
                crate::state::ObservationRecord::Audio(record) if ids.insert(record.id.clone()) => {
                    Ok(record)
                }
                _ => Err(serde::de::Error::custom(
                    "未観察発話の形式またはIDが不正です",
                )),
            }
        })
        .collect()
}

pub(crate) fn deserialize_contexts<'de, D>(deserializer: D) -> Result<Vec<HearingContext>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let contexts = Vec::<HearingContext>::deserialize(deserializer)?;
    validate_contexts(&contexts).map_err(serde::de::Error::custom)?;
    Ok(contexts)
}

pub(crate) fn validate_contexts(contexts: &[HearingContext]) -> Result<(), &'static str> {
    if contexts.len() > 2
        || contexts.iter().enumerate().any(|(index, context)| {
            uuid::Uuid::parse_str(&context.session_id).is_err()
                || context.generation == 0
                || context.sequence == 0
                || context.text.len() > MAX_HEARING_TEXT_BYTES
                || context
                    .speaker
                    .as_ref()
                    .is_some_and(|speaker| !speaker.is_valid_for(context.source))
                || contexts[..index]
                    .iter()
                    .any(|previous| previous.source == context.source)
        })
    {
        return Err("音声文脈の世代、更新順序、件数またはサイズが不正です");
    }
    Ok(())
}

pub(crate) struct HearingContextBuffer {
    session: Option<(u64, String, CancellationToken)>,
    latest: Vec<HearingContext>,
    pending_audio: Vec<crate::state::AudioObservation>,
    persistent: bool,
    microphone_commands: MicrophoneCommands,
}

impl Default for HearingContextBuffer {
    fn default() -> Self {
        Self {
            session: None,
            latest: Vec::new(),
            pending_audio: Vec::new(),
            persistent: true,
            microphone_commands: MicrophoneCommands::default(),
        }
    }
}

impl HearingContextBuffer {
    pub(crate) fn set_persistent(&mut self, persistent: bool) {
        if self.persistent == persistent {
            return;
        }
        if let Some((_, _, cancellation)) = self.session.take() {
            cancellation.cancel();
        }
        self.latest.clear();
        self.pending_audio.clear();
        self.microphone_commands.reset_session();
        self.persistent = persistent;
    }

    pub(crate) fn is_persistent(&self) -> bool {
        self.persistent
    }

    pub(crate) fn accepts_session(&self, id: &str) -> bool {
        self.session
            .as_ref()
            .is_some_and(|(_, current, cancellation)| current == id && !cancellation.is_cancelled())
    }

    pub(crate) fn begin(
        &mut self,
        generation: u64,
        cancellation: CancellationToken,
    ) -> Option<String> {
        if generation == 0
            || cancellation.is_cancelled()
            || self
                .session
                .as_ref()
                .is_some_and(|(previous, _, _)| *previous >= generation)
        {
            return None;
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.session = Some((generation, id.clone(), cancellation));
        self.latest.clear();
        self.microphone_commands.reset_session();
        Some(id)
    }

    pub(crate) fn set_microphone_commands_enabled(&mut self, enabled: bool) {
        self.microphone_commands
            .set_enabled(enabled && self.persistent);
    }

    pub(crate) fn register_microphone_final(
        &mut self,
        context: &HearingContext,
    ) -> Result<(), crate::state::ObservationError> {
        if self.accepts_session(&context.session_id) {
            self.microphone_commands.record(context)?;
        }
        Ok(())
    }

    pub(crate) fn microphone_command_candidates(
        &mut self,
        audio: &[crate::state::AudioObservation],
    ) -> Option<MicrophoneCommandBatch> {
        if self
            .session
            .as_ref()
            .is_none_or(|(_, _, cancellation)| cancellation.is_cancelled())
        {
            return None;
        }
        self.microphone_commands.candidates(audio)
    }

    pub(crate) fn take_microphone_commands(
        &mut self,
        batch: &MicrophoneCommandBatch,
    ) -> Vec<crate::state::AudioObservation> {
        if self
            .session
            .as_ref()
            .is_none_or(|(_, _, cancellation)| cancellation.is_cancelled())
        {
            return Vec::new();
        }
        self.microphone_commands.take(batch)
    }

    pub(crate) fn prepare_update(&self, mut context: HearingContext) -> Option<HearingContext> {
        let (_, session_id, cancellation) = self.session.as_ref()?;
        if cancellation.is_cancelled()
            || context.session_id != *session_id
            || context.generation == 0
            || context.sequence == 0
            || self.latest.iter().any(|previous| {
                previous.source == context.source
                    && (context.generation < previous.generation
                        || (context.generation == previous.generation
                            && (context.sequence <= previous.sequence || previous.confirmed)))
            })
        {
            return None;
        }
        let mut end = context.text.len().min(MAX_HEARING_TEXT_BYTES);
        while !context.text.is_char_boundary(end) {
            end -= 1;
        }
        context.text.truncate(end);
        Some(context)
    }

    pub(crate) fn record(
        &mut self,
        context: HearingContext,
        audio: Option<crate::state::AudioObservation>,
    ) -> bool {
        if let Some(audio) = audio {
            if self.pending_audio.len() >= MAX_PENDING_AUDIO_COUNT
                || self
                    .pending_audio
                    .iter()
                    .map(|record| record.text.len())
                    .sum::<usize>()
                    + audio.text.len()
                    > MAX_PENDING_AUDIO_BYTES
            {
                return false;
            }
            self.pending_audio.push(audio);
        }
        self.latest
            .retain(|previous| previous.source != context.source);
        self.latest.push(context);
        true
    }

    pub(crate) fn acknowledge_saved_audio(&mut self, id: &str) {
        let observation_id = crate::state::audio_segment_observation_id(id);
        self.pending_audio
            .retain(|record| record.id != observation_id);
    }

    pub(crate) fn pending_audio_after_discard_for_day_with_ids(
        &self,
        date: chrono::NaiveDate,
        audio_ids: &std::collections::HashSet<String>,
    ) -> Result<Vec<crate::state::AudioObservation>, crate::persistence::PersistenceError> {
        let parse_timestamp = |value: &str| {
            chrono::DateTime::parse_from_rfc3339(value).map_err(|_| {
                crate::persistence::PersistenceError::Invalid(
                    "保留中の音声時刻が不正です".to_owned(),
                )
            })
        };
        let mut retained = Vec::with_capacity(self.pending_audio.len());
        for record in &self.pending_audio {
            let parsed = parse_timestamp(&record.created_at)?;
            parse_timestamp(&record.window_start)?;
            parse_timestamp(&record.window_end)?;
            let observation_id = crate::state::audio_segment_observation_id(&record.id);
            if observation_id.is_empty() {
                return Err(crate::persistence::PersistenceError::Invalid(
                    "保留中の音声 ID が不正です".to_owned(),
                ));
            }
            if !audio_ids.contains(observation_id)
                && crate::config::local_date_at(parsed.with_timezone(&chrono::Utc))
                    != date.format("%Y-%m-%d").to_string()
            {
                retained.push(record.clone());
            }
        }
        Ok(retained)
    }

    pub(crate) fn replace_pending_audio(
        &mut self,
        pending_audio: Vec<crate::state::AudioObservation>,
    ) {
        self.pending_audio = pending_audio;
    }

    pub(crate) fn pending_audio(&self) -> &[crate::state::AudioObservation] {
        &self.pending_audio
    }

}
