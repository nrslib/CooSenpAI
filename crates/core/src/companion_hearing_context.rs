use super::user::UserMessagePreparer;
use super::*;
use crate::hearing_context::HearingContext;

impl CompanionAgent {
    pub(crate) fn set_hearing_context(
        &mut self,
        context: Arc<std::sync::Mutex<crate::hearing_context::HearingContextBuffer>>,
    ) {
        self.hearing_context = context;
    }
}

impl UserMessagePreparer {
    pub(crate) fn hearing_audio_ingestion(
        &self,
        session_id: String,
    ) -> crate::hearing_ingestion::HearingAudioIngestion {
        crate::hearing_ingestion::HearingAudioIngestion::new(
            session_id,
            self.hearing_context.clone(),
        )
    }

    pub(crate) fn acknowledge_saved_hearing_audio(&self, id: &str) -> Result<(), CompanionError> {
        self.hearing_context
            .lock()
            .map_err(|_| PersistenceError::Invalid("音声文脈のロックが壊れています".to_owned()))?
            .acknowledge_saved_audio(id);
        Ok(())
    }

    pub(crate) fn begin_hearing_context(
        &self,
        generation: u64,
        cancellation: CancellationToken,
    ) -> Result<String, CompanionError> {
        let mut buffer = self
            .hearing_context
            .lock()
            .map_err(|_| PersistenceError::Invalid("音声文脈のロックが壊れています".to_owned()))?;
        buffer.begin(generation, cancellation).ok_or_else(|| {
            PersistenceError::Invalid("音声文脈の開始世代が古いか、取消済みです".to_owned()).into()
        })
    }

    pub(crate) fn update_hearing_context(
        &self,
        context: HearingContext,
    ) -> Result<bool, CompanionError> {
        let mut buffer = self
            .hearing_context
            .lock()
            .map_err(|_| PersistenceError::Invalid("音声文脈のロックが壊れています".to_owned()))?;
        let original = context.clone();
        let Some(context) = buffer.prepare_update(context) else {
            return Ok(false);
        };
        let audio = if original.confirmed && !original.text.trim().is_empty() {
            Some(
                original
                    .confirmed_audio(chrono::Utc::now())
                    .map_err(|_| PersistenceError::Invalid("確定した音声が不正です".to_owned()))?,
            )
        } else {
            None
        };
        if !buffer.record(context, audio) {
            return Err(CompanionError::AudioPendingOverflow);
        }
        Ok(true)
    }
}
