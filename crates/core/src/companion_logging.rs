use super::{CompanionAgent, CompanionError};
use crate::provider::{ProviderErrorKind, ProviderUsage};
use std::path::Path;

impl CompanionAgent {
    pub(super) fn log_proactive_limit_reached(&mut self) -> Result<(), CompanionError> {
        let Some(limit) = self.config.daily_proactive_limit else {
            return Ok(());
        };
        let date = crate::config::local_date_at(self.clock.now());
        let should_log = match &self.storage {
            Some(storage) => storage.try_record_limit_notice(&date)?,
            None => self.last_proactive_limit_log_date.as_deref() != Some(date.as_str()),
        };
        if !should_log {
            return Ok(());
        }
        if let Some(logger) = &self.logger {
            logger.write(
                "INFO",
                &format!("相棒の自発呼び出しが一日上限に達しました: limit={}", limit),
            )?;
        }
        if self.storage.is_none() {
            self.last_proactive_limit_log_date = Some(date);
        }
        Ok(())
    }

    fn provider_label(&self) -> &str {
        self.provider
            .provider_name()
            .map_or("unknown", |provider| provider.as_str())
    }

    pub(super) fn log_call_start(&self, mode: &str) -> Result<(), CompanionError> {
        if let Some(logger) = &self.logger {
            logger.write(
                "INFO",
                &format!(
                    "companion 呼び出し開始: provider={} mode={mode}",
                    self.provider_label()
                ),
            )?;
        }
        Ok(())
    }

    pub(super) fn log_call_end(&self, mode: &str, elapsed_ms: u128) -> Result<(), CompanionError> {
        if let Some(logger) = &self.logger {
            logger.write(
                "INFO",
                &format!(
                    "companion 呼び出し終了: provider={} mode={mode} elapsed-ms={elapsed_ms}",
                    self.provider_label(),
                ),
            )?;
        }
        Ok(())
    }

    pub(super) fn log_call_failure(
        &self,
        mode: &str,
        kind: ProviderErrorKind,
        detail: Option<&str>,
    ) {
        if let Some(logger) = &self.logger {
            let detail = detail.map_or(String::new(), |value| format!(" detail={value}"));
            let _ = logger.write(
                "WARN",
                &format!(
                    "companion 呼び出し失敗: provider={} mode={mode} error-type={}{}",
                    self.provider_label(),
                    kind.as_str(),
                    detail,
                ),
            );
        }
    }

    pub(super) fn log_session_rejection(&self, mode: &str, error: &CompanionError) {
        if let Some(logger) = &self.logger {
            let detail = match error {
                CompanionError::Provider(error) => error.message.clone(),
                _ => error.to_string(),
            }
            .replace(['\r', '\n'], " ");
            let _ = logger.write(
                "WARN",
                &format!(
                    "companion 応答を受理できませんでした: provider={} mode={mode} error-type=session-validation detail={detail}",
                    self.provider_label(),
                ),
            );
        }
    }

    pub(super) fn log_debug_failure(&self) {
        if let Some(logger) = &self.logger {
            let _ = logger.write(
                "WARN",
                "デバッグ記録に失敗しました: stage=companion error-type=debug-persistence",
            );
        }
    }

    pub(super) fn log_observation_image_skip(&self, path: &Path, error: &std::io::Error) {
        if let Some(logger) = &self.logger {
            let detail = error.to_string().replace(['\r', '\n'], " ");
            let _ = logger.write(
                "DEBUG",
                &format!(
                    "観察フレーム画像を添付から除外しました: path={} detail={detail}",
                    path.display()
                ),
            );
        }
    }

    pub(super) fn log_usage_persistence_failure(&self, stage: &str) {
        if let Some(logger) = &self.logger {
            let _ = logger.write(
                "WARN",
                &format!(
                    "companion usage の保存に失敗しましたがユーザー応答を続行します: stage={stage} error-type=usage-persistence"
                ),
            );
        }
    }

    pub(super) fn log_watch_call_measurement(
        &self,
        emitted: bool,
        counted_emit: bool,
        usage: Option<&ProviderUsage>,
    ) {
        let Some(logger) = &self.logger else {
            return;
        };
        let token = |value: Option<u64>| {
            value.map_or_else(|| "unreported".to_owned(), |value| value.to_string())
        };
        let _ = logger.write(
            "INFO",
            &format!(
                "見守り companion 実測: calls-today={} proactive-emits-today={} emitted={} counted={} input-tokens={} cached-input-tokens={} output-tokens={} total-tokens={}",
                self.total_calls_today,
                self.proactive_calls_today,
                emitted,
                counted_emit,
                token(usage.and_then(|value| value.input_tokens)),
                token(usage.and_then(|value| value.cached_input_tokens)),
                token(usage.and_then(|value| value.output_tokens)),
                token(usage.and_then(|value| value.total_tokens)),
            ),
        );
    }
}
