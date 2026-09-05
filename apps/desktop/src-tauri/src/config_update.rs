use crate::factory::DesktopFactoryError;
use coosenpai_core::config::{Config, ConfigValidationIssue};
use coosenpai_core::runtime::RuntimeError;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex;

pub(crate) struct ConfigUpdateOutcome {
    pub config: Config,
    pub issues: Vec<ConfigValidationIssue>,
}

#[derive(Default)]
pub(crate) struct ConfigUpdateCoordinator {
    pub(crate) serial: Mutex<()>,
    revision: AtomicU64,
    config_revision: AtomicU64,
}

pub(crate) struct ConfigUpdateTransaction<'a> {
    coordinator: &'a ConfigUpdateCoordinator,
    _guard: tokio::sync::MutexGuard<'a, ()>,
    pub(crate) base_revision: u64,
    pub(crate) base_config_revision: u64,
}

impl ConfigUpdateCoordinator {
    pub(crate) fn new(config_revision: u64) -> Self {
        Self {
            serial: Mutex::new(()),
            revision: AtomicU64::new(0),
            config_revision: AtomicU64::new(config_revision),
        }
    }

    pub(crate) async fn begin(&self) -> ConfigUpdateTransaction<'_> {
        let guard = self.serial.lock().await;
        ConfigUpdateTransaction {
            coordinator: self,
            _guard: guard,
            base_revision: self.revision.load(Ordering::Acquire),
            base_config_revision: self.config_revision.load(Ordering::Acquire),
        }
    }

    pub(crate) fn current_revision(&self) -> u64 {
        self.config_revision.load(Ordering::Acquire)
    }

    pub(crate) fn observe_config_revision(&self, revision: u64) {
        self.config_revision.store(revision, Ordering::Release);
    }
}

impl ConfigUpdateTransaction<'_> {
    pub(crate) fn ensure_expected_revision(
        &self,
        expected: Option<u64>,
    ) -> Result<(), ConfigCommitError> {
        if let Some(expected) = expected {
            if expected != self.base_config_revision {
                return Err(ConfigCommitError::Storage(
                    coosenpai_core::config::ConfigError::RevisionConflict {
                        expected,
                        actual: self.base_config_revision,
                    },
                ));
            }
        }
        Ok(())
    }

    fn commit_generation(&self) -> Result<(), ConfigCommitError> {
        self.coordinator
            .revision
            .compare_exchange(
                self.base_revision,
                self.base_revision.saturating_add(1),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map(|_| ())
            .map_err(|_| {
                ConfigCommitError::Runtime(RuntimeError::Factory(
                    "設定の revision が競合しました".to_owned(),
                ))
            })
    }

    pub(crate) fn commit(self) -> Result<(), ConfigCommitError> {
        self.commit_generation()
    }

    pub(crate) fn commit_config(self, config_revision: u64) -> Result<(), ConfigCommitError> {
        self.commit_generation()?;
        self.coordinator
            .config_revision
            .store(config_revision, Ordering::Release);
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigCommitError {
    #[error(transparent)]
    Factory(#[from] DesktopFactoryError),
    #[error(transparent)]
    Storage(#[from] coosenpai_core::config::ConfigError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

impl ConfigCommitError {
    pub fn format_for_user(&self) -> String {
        match self {
            Self::Factory(error) => error.to_string(),
            Self::Storage(error) => error.format_for_user(),
            Self::Runtime(RuntimeError::Config(error)) => error.format_for_user(),
            Self::Runtime(error) => error.to_string(),
        }
    }

    pub fn issues(&self) -> Vec<ConfigValidationIssue> {
        match self {
            Self::Factory(error) => vec![error.issue.clone()],
            Self::Storage(coosenpai_core::config::ConfigError::Validation(issues))
            | Self::Runtime(RuntimeError::Config(
                coosenpai_core::config::ConfigError::Validation(issues),
            )) => issues.clone(),
            _ => Vec::new(),
        }
    }
}
