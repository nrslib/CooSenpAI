use anyhow::Result;
use chrono::Utc;
use coosenpai_core::config::{load_config, Config, ConfigError, ConfigPaths};
use coosenpai_core::ports::RuntimeLogger;
use coosenpai_core::runtime::{RuntimeErrorKind, RuntimeHandle, RuntimeLastError};
use coosenpai_core::watch_coordinator::RetryBackoff;
use std::time::Instant;

pub(super) struct ConfigReload {
    degraded: bool,
    failed_config: Option<Config>,
    retry: RetryBackoff,
}

impl ConfigReload {
    pub(super) fn new() -> Self {
        Self {
            degraded: false,
            failed_config: None,
            retry: RetryBackoff::default(),
        }
    }

    pub(super) fn is_degraded(&self) -> bool {
        self.degraded
    }

    pub(super) async fn refresh(
        &mut self,
        paths: &ConfigPaths,
        runtime: &RuntimeHandle,
        logger: &dyn RuntimeLogger,
    ) -> Result<()> {
        match load_config(paths) {
            Ok(config)
                if self.failed_config.as_ref() == Some(&config)
                    && !self.retry.is_due(Instant::now()) => {}
            Ok(config) if self.degraded || config != runtime.config() => {
                if self.failed_config.as_ref() != Some(&config) {
                    self.retry.reset();
                }
                match runtime.update_config(config.clone()).await {
                    Ok(_) => self.recovered(),
                    Err(_) => {
                        self.degraded = true;
                        self.failed_config = Some(config);
                        self.retry.defer(Instant::now());
                        let _ = logger
                            .write("WARN", "設定更新に失敗しました: error-type=config-update");
                    }
                }
            }
            Ok(_) => {}
            Err(error) if !self.degraded => {
                runtime.enter_degraded(config_read_error(error)).await?;
                self.degraded = true;
                self.failed_config = None;
                self.retry.reset();
                let _ = logger.write("WARN", "設定の再読込に失敗しました: error-type=config-read");
            }
            Err(_) => {}
        }
        Ok(())
    }

    fn recovered(&mut self) {
        self.degraded = false;
        self.failed_config = None;
        self.retry.reset();
    }
}

fn config_read_error(error: ConfigError) -> RuntimeLastError {
    let message = error.format_for_user();
    let issues = match error {
        ConfigError::Validation(issues) => issues,
        _ => Vec::new(),
    };
    RuntimeLastError {
        kind: RuntimeErrorKind::Config,
        occurred_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: Some(message),
        issues,
        attachment_ocr: None,
    }
}
