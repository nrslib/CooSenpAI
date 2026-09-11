use crate::config::ConfigPaths;
use crate::hearing_context::HearingContextBuffer;
use crate::persistence::PersistenceError;
use crate::state::AudioObservation;
use std::sync::{Arc, Mutex, MutexGuard};

/// helper が受理された session に保存と観察を束縛する。
#[derive(Clone)]
pub struct HearingAudioIngestion {
    session_id: String,
    buffer: Arc<Mutex<HearingContextBuffer>>,
}

#[derive(Debug)]
pub struct HearingAudioRecord {
    pub observation: AudioObservation,
    pub transcript_path: Option<String>,
}

impl HearingAudioIngestion {
    pub(crate) fn new(session_id: String, buffer: Arc<Mutex<HearingContextBuffer>>) -> Self {
        Self { session_id, buffer }
    }

    pub fn record(
        &self,
        paths: &ConfigPaths,
        retention_days: u64,
        observation: AudioObservation,
    ) -> Result<Option<HearingAudioRecord>, PersistenceError> {
        let mut buffer = self.lock()?;
        if !buffer.accepts_session(&self.session_id) {
            return Ok(None);
        }
        // モード切替も同じロックを使い、判定後から書き込みまでの競合を防ぐ。
        let observation = if buffer.is_persistent() {
            let observation = persist_audio(paths, retention_days, observation)?;
            buffer.acknowledge_saved_audio(&observation.id);
            observation
        } else {
            observation
        };
        let transcript_path = if buffer.is_persistent() {
            let time = chrono::DateTime::parse_from_rfc3339(&observation.created_at)
                .map_err(|_| PersistenceError::Invalid("発話時刻が不正です".to_owned()))?;
            Some(
                paths
                    .transcripts
                    .join(format!(
                        "{}.jsonl",
                        crate::config::local_date_at(time.with_timezone(&chrono::Utc))
                    ))
                    .to_string_lossy()
                    .into_owned(),
            )
        } else {
            None
        };
        Ok(Some(HearingAudioRecord {
            observation,
            transcript_path,
        }))
    }

    pub fn pending_audio(
        &self,
        paths: &ConfigPaths,
        retention_days: u64,
    ) -> Result<Vec<AudioObservation>, PersistenceError> {
        let mut buffer = self.lock()?;
        if !buffer.accepts_session(&self.session_id) {
            return Ok(Vec::new());
        }
        let audio = if buffer.is_persistent() {
            for record in buffer.pending_audio().to_vec() {
                let saved = persist_audio(paths, retention_days, record)?;
                buffer.acknowledge_saved_audio(&saved.id);
            }
            crate::observer::read_unobserved_audio(&paths.observations)?
        } else {
            buffer.pending_audio().to_vec()
        };
        crate::hearing_context::observation_batch(audio)
            .map_err(|_| PersistenceError::Invalid("発話時刻が不正です".to_owned()))
    }

    fn lock(&self) -> Result<MutexGuard<'_, HearingContextBuffer>, PersistenceError> {
        self.buffer
            .lock()
            .map_err(|_| PersistenceError::Invalid("音声文脈のロックが壊れています".to_owned()))
    }

}

fn persist_audio(
    paths: &ConfigPaths,
    retention_days: u64,
    observation: AudioObservation,
) -> Result<AudioObservation, PersistenceError> {
    let mut existing = crate::observer::read_audio_by_ids(
        &paths.observations,
        &std::iter::once(observation.id.clone()).collect(),
    )?;
    // Final の受付と ingestion は別タスクなので、再試行では最初の保存時刻を保つ。
    let observation = match existing.pop() {
        Some(saved) if saved.source == observation.source && saved.text == observation.text => {
            saved
        }
        Some(_) => {
            return Err(PersistenceError::Invalid(
                "同じ発話 ID の本文または入力源が異なります".to_owned(),
            ))
        }
        None => observation,
    };
    crate::observer::record_audio_observation(
        paths,
        retention_days,
        &observation,
        chrono::Utc::now(),
    )?;
    Ok(observation)
}
