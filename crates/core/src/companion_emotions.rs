use crate::companion_storage::{CompanionStorage, CursorSnapshot};
use crate::emotion::{EmotionDelta, EmotionState};
use crate::persistence::PersistenceError;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub(super) struct EmotionSnapshot {
    pub(super) state: EmotionState,
    pub(super) epoch: u64,
    enabled: bool,
}

impl Default for EmotionSnapshot {
    fn default() -> Self {
        Self {
            state: EmotionState::default(),
            epoch: 0,
            enabled: true,
        }
    }
}

impl From<&CursorSnapshot> for EmotionSnapshot {
    fn from(cursor: &CursorSnapshot) -> Self {
        Self {
            state: cursor.companion_emotions,
            epoch: cursor.emotion_epoch,
            enabled: cursor.emotion_updates_enabled,
        }
    }
}

fn next_epoch(epoch: u64) -> Result<u64, PersistenceError> {
    epoch
        .checked_add(1)
        .ok_or_else(|| PersistenceError::Invalid("emotion epoch が上限に達しました".to_owned()))
}

#[derive(Clone, Default)]
pub(crate) struct CompanionEmotions {
    current: Arc<Mutex<EmotionSnapshot>>,
}

impl CompanionEmotions {
    pub(crate) fn state(&self) -> EmotionState {
        self.current
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .state
    }

    pub(super) fn load(
        &self,
        storage: Option<&CompanionStorage>,
    ) -> Result<EmotionSnapshot, PersistenceError> {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(storage) = storage {
            let cursor = storage.load_cursor()?;
            *current = EmotionSnapshot::from(&cursor);
        }
        Ok(*current)
    }

    pub(super) fn configure(
        &self,
        storage: Option<&CompanionStorage>,
        enabled: bool,
    ) -> Result<EmotionSnapshot, PersistenceError> {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(storage) = storage {
            let cursor = storage.load_cursor()?;
            let updated = if cursor.emotion_updates_enabled == enabled {
                EmotionSnapshot::from(&cursor)
            } else {
                storage.update_cursor(|cursor| {
                    if cursor.emotion_updates_enabled != enabled {
                        cursor.emotion_epoch = next_epoch(cursor.emotion_epoch)?;
                        cursor.emotion_updates_enabled = enabled;
                    }
                    Ok(EmotionSnapshot::from(&*cursor))
                })?
            };
            *current = updated;
        } else if current.enabled != enabled {
            current.epoch = next_epoch(current.epoch)?;
            current.enabled = enabled;
        }
        Ok(*current)
    }

    pub(super) fn update_cursor<R>(
        &self,
        storage: &CompanionStorage,
        update: impl FnOnce(&mut CursorSnapshot) -> Result<R, PersistenceError>,
    ) -> Result<R, PersistenceError> {
        // cursor の保存と表示用 state の更新を reset/commit 間でも同じ順序にする。
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (result, updated) = storage.update_cursor(|cursor| {
            let result = update(cursor)?;
            Ok((result, EmotionSnapshot::from(&*cursor)))
        })?;
        *current = updated;
        Ok(result)
    }

    pub(super) fn apply_volatile(&self, epoch: u64, delta: Option<EmotionDelta>) {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if current.enabled && current.epoch == epoch {
            if let Some(delta) = delta {
                current.state = current.state.apply(delta);
            }
        }
    }

    pub(super) fn reset(&self, storage: Option<&CompanionStorage>) -> Result<(), PersistenceError> {
        if let Some(storage) = storage {
            self.update_cursor(storage, |cursor| {
                cursor.emotion_epoch = next_epoch(cursor.emotion_epoch)?;
                cursor.companion_emotions = EmotionState::default();
                Ok(())
            })
        } else {
            let mut current = self
                .current
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            *current = EmotionSnapshot {
                state: EmotionState::default(),
                epoch: next_epoch(current.epoch)?,
                enabled: current.enabled,
            };
            Ok(())
        }
    }
}

impl super::CompanionAgent {
    pub(crate) fn synchronize_emotions(&self) -> Result<(), super::CompanionError> {
        self.emotions
            .configure(self.storage.as_ref(), self.config.emotions_enabled)?;
        Ok(())
    }

    pub fn emotions(&self) -> Result<EmotionState, super::CompanionError> {
        Ok(self.emotions.load(self.storage.as_ref())?.state)
    }
}

impl super::user::UserMessagePreparer {
    pub(crate) fn reset_companion_emotions(&self) -> Result<(), super::CompanionError> {
        Ok(self.emotions.reset(self.storage.as_ref())?)
    }

    pub(crate) fn companion_emotions(&self) -> EmotionState {
        self.emotions.state()
    }
}
