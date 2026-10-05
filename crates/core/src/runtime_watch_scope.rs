use super::*;
use std::sync::atomic::Ordering;

impl RuntimeHandle {
    pub fn watch_capture_config(&self) -> (Config, u64) {
        self.watch_capture_config_with(|| {})
    }

    fn watch_capture_config_with(&self, after_config: impl FnOnce()) -> (Config, u64) {
        let _guard = self
            .watch_scope_commit_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let config = self.config_rx.borrow().clone();
        after_config();
        (config, self.watch_scope_generation())
    }

    pub fn lock_current_watch_scope(
        &self,
        expected: u64,
    ) -> Result<std::sync::MutexGuard<'_, ()>, RuntimeError> {
        let guard = self
            .watch_scope_commit_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.watch_scope_generation() != expected {
            return Err(RuntimeError::StaleWatchScope);
        }
        Ok(guard)
    }

    pub fn watch_scope_generation(&self) -> u64 {
        self.watch_scope_generation.load(Ordering::Acquire)
    }

    pub fn invalidate_watch_scope(&self) {
        let _guard = self
            .watch_scope_commit_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        advance_scope_generation(
            &self.watch_scope_generation,
            &self.watch_scope_cancellation,
            self.operation_cancellation.shutdown_token(),
        );
        self.cancel_operations_for_config_update();
    }

    pub(super) fn advance_watch_scope_generation(&self, next: &Config) {
        let _guard = self
            .watch_scope_commit_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = self.config_rx.borrow();
        if current.watch_capture_scope_changed(next) {
            advance_scope_generation(
                &self.watch_scope_generation,
                &self.watch_scope_cancellation,
                self.operation_cancellation.shutdown_token(),
            );
        }
    }
}

impl RuntimeActor {
    pub(super) fn publish_config(&self, config_tx: &watch::Sender<Config>) {
        let _guard = self
            .watch_scope_commit_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scope_changed = config_tx.borrow().watch_capture_scope_changed(&self.config);
        if scope_changed {
            advance_scope_generation(
                &self.watch_scope_generation,
                &self.watch_scope_cancellation,
                self.operation_cancellation.shutdown_token(),
            );
        }
        config_tx.send_replace(self.config.clone());
    }

    pub(super) fn watch_scope_cancellation(&self) -> CancellationToken {
        self.watch_scope_cancellation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn accepts_watch_scope(&self, frames: &[ObservationFrameInput]) -> bool {
        let generation = self.watch_scope_generation.load(Ordering::Acquire);
        frames
            .iter()
            .all(|frame| frame.scope_generation == generation)
    }
}

fn advance_scope_generation(
    generation: &std::sync::atomic::AtomicU64,
    cancellation: &std::sync::Mutex<CancellationToken>,
    shutdown: CancellationToken,
) {
    let mut current = cancellation
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    current.cancel();
    *current = shutdown.child_token();
    generation.fetch_add(1, Ordering::AcqRel);
}
