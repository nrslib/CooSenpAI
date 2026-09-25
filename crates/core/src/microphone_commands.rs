use crate::hearing_context::HearingContext;
use crate::state::{AudioObservation, AudioObservationSource, ObservationError};
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub(crate) struct MicrophoneCommandBatch {
    pub(crate) epoch: u64,
    pub(crate) ids: Vec<String>,
    pub(crate) selected_ids: Vec<String>,
}

#[derive(Default)]
pub(crate) struct MicrophoneCommands {
    enabled: bool,
    epoch: u64,
    last_generation: u64,
    pending: VecDeque<AudioObservation>,
}

impl MicrophoneCommands {
    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        if self.enabled != enabled {
            self.enabled = enabled;
            self.epoch = self.epoch.saturating_add(1);
            self.pending.clear();
        }
    }

    pub(crate) fn reset_session(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
        self.last_generation = 0;
        self.pending.clear();
    }

    pub(crate) fn record(&mut self, context: &HearingContext) -> Result<(), ObservationError> {
        if context.source != AudioObservationSource::Microphone
            || !context.confirmed
            || context.generation <= self.last_generation
        {
            return Ok(());
        }
        self.last_generation = context.generation;
        if self.enabled {
            let audio = context.confirmed_audio(chrono::Utc::now())?;
            if self.pending.len() == crate::hearing_context::MAX_PENDING_AUDIO_COUNT {
                self.pending.pop_front();
            }
            self.pending.push_back(audio);
        }
        Ok(())
    }

    pub(crate) fn candidates(
        &mut self,
        audio: &[AudioObservation],
    ) -> Option<MicrophoneCommandBatch> {
        if !self.enabled {
            return None;
        }
        self.pending.retain(|live| {
            let microphone_present = audio.iter().any(|record| {
                record.source == AudioObservationSource::Microphone
                    && record.id == live.id
                    && record.text == live.text
            });
            let playback_present = audio.iter().any(|record| {
                record.source == AudioObservationSource::Speaker && record.text == live.text
            });
            !(microphone_present && playback_present)
        });
        let ids: Vec<_> = audio
            .iter()
            .filter(|record| {
                record.source == AudioObservationSource::Microphone
                    && self
                        .pending
                        .iter()
                        .any(|live| live.id == record.id && live.text == record.text)
            })
            .map(|record| record.id.clone())
            .collect();
        (!ids.is_empty()).then_some(MicrophoneCommandBatch {
            epoch: self.epoch,
            ids,
            selected_ids: Vec::new(),
        })
    }

    pub(crate) fn take(&mut self, batch: &MicrophoneCommandBatch) -> Vec<AudioObservation> {
        if !self.enabled || batch.epoch != self.epoch {
            return Vec::new();
        }
        let mut commands = Vec::new();
        for id in &batch.ids {
            if let Some(index) = self.pending.iter().position(|record| &record.id == id) {
                let command = self.pending.remove(index).expect("candidate index");
                if batch.selected_ids.contains(id) {
                    commands.push(command);
                }
            }
        }
        commands
    }
}
