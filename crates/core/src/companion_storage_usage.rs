use super::*;
use crate::usage::{CompanionCallKind, CompanionUsage, UsageError};

impl CompanionStorage {
    pub(crate) fn load_companion_usage(&self, date: &str) -> Result<CompanionUsage, UsageError> {
        self.with_usage_recovery(|| crate::usage::load_companion(&self.usage_path, date))
    }

    pub(crate) fn record_companion_attempt(
        &self,
        date: &str,
        kind: CompanionCallKind,
    ) -> Result<CompanionUsage, UsageError> {
        self.with_usage_recovery(|| {
            crate::usage::record_companion_attempt(&self.usage_path, date, kind)
        })
    }

    pub(crate) fn try_record_proactive_emit(
        &self,
        date: &str,
        limit: Option<u32>,
        emit_id: &str,
    ) -> Result<Option<CompanionUsage>, UsageError> {
        self.with_usage_recovery(|| {
            crate::usage::try_record_proactive_emit(&self.usage_path, date, limit, emit_id)
        })
    }

    pub(crate) fn forget_proactive_emit_id(&self, emit_id: &str) -> Result<(), UsageError> {
        self.with_usage_recovery(|| {
            crate::usage::forget_proactive_emit_id(&self.usage_path, emit_id)
        })
    }

    fn with_usage_recovery<T>(
        &self,
        operation: impl Fn() -> Result<T, UsageError>,
    ) -> Result<T, UsageError> {
        match operation() {
            Err(UsageError::Json(_)) => {
                if crate::usage::quarantine_invalid_companion(&self.usage_path)? {
                    if let Ok(logger) = FileLogger::new(self.log_path.clone()) {
                        let _ = logger.write(
                            "WARN",
                            "壊れた companion usage を隔離しました: error-type=usage-json",
                        );
                    }
                }
                operation()
            }
            result => result,
        }
    }
}
