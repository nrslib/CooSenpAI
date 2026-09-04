use super::*;
use std::sync::atomic::Ordering;

impl RuntimeHandle {
    pub fn watch_scope_generation(&self) -> u64 {
        self.watch_scope_generation.load(Ordering::Acquire)
    }

    pub fn invalidate_watch_scope(&self) {
        let _guard = self
            .watch_scope_commit_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.watch_scope_generation.fetch_add(1, Ordering::AcqRel);
        self.cancel_operations_for_config_update();
    }

    pub(super) fn advance_watch_scope_generation(&self, next: &Config) {
        let _guard = self
            .watch_scope_commit_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if watch_scope_changed(&self.config(), next) {
            self.watch_scope_generation.fetch_add(1, Ordering::AcqRel);
        }
    }
}

impl RuntimeActor {
    pub(super) fn advance_watch_scope_generation(&self, next: &Config) {
        let _guard = self
            .watch_scope_commit_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if watch_scope_changed(&self.config, next) {
            self.watch_scope_generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    pub(super) fn accepts_watch_scope(&self, frames: &[ObservationFrameInput]) -> bool {
        let generation = self.watch_scope_generation.load(Ordering::Acquire);
        frames
            .iter()
            .all(|frame| frame.scope_generation == generation)
    }
}

fn watch_scope_changed(current: &Config, next: &Config) -> bool {
    current.watch.fullscreen != next.watch.fullscreen || current.watch.apps != next.watch.apps
}
