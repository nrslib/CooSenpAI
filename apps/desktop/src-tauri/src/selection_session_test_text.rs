use super::RecordingPort;
use coosenpai_core::ports::{
    ClipboardReader, PortError, RuntimeLogger, SelectedTextCopyOutcome, SelectedTextCopyPort,
};
use tokio_util::sync::CancellationToken;

#[derive(Default, Clone, Copy)]
pub(crate) enum CopyBehavior {
    #[default]
    Sent,
    PermissionDenied,
    ReleaseTimeout,
}

#[async_trait::async_trait]
impl SelectedTextCopyPort for RecordingPort {
    async fn synthesize_copy(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<SelectedTextCopyOutcome, PortError> {
        if cancellation.is_cancelled() {
            return Ok(SelectedTextCopyOutcome::Cancelled);
        }
        self.record("text:copy-attempt");
        let mut observed = self.observation.lock().unwrap();
        match observed.copy_behavior {
            CopyBehavior::PermissionDenied => Ok(SelectedTextCopyOutcome::PermissionDenied),
            CopyBehavior::ReleaseTimeout => Ok(SelectedTextCopyOutcome::ReleaseTimeout),
            CopyBehavior::Sent => {
                let before = observed.count;
                if let Some(text) = observed.copied_text.take() {
                    observed.text = Some(text);
                    observed.count += 1;
                }
                self.record(format!("text:posted:{before}"));
                Ok(SelectedTextCopyOutcome::Sent {
                    change_count_before_post: before,
                })
            }
        }
    }
}

impl ClipboardReader for RecordingPort {
    fn read_text(&self) -> Result<Option<String>, PortError> {
        self.record("text:read");
        Ok(self.observation.lock().unwrap().text.clone())
    }
    fn change_count(&self) -> Result<i64, PortError> {
        self.record("text:count");
        Ok(self.observation.lock().unwrap().count)
    }
}

impl RuntimeLogger for RecordingPort {
    fn write(&self, _level: &str, message: &str) -> Result<(), std::io::Error> {
        self.record(message);
        Ok(())
    }
}
