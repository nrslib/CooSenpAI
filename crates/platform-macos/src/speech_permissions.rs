use crate::audio_permissions::{current_microphone, request_microphone};
use crate::hearing_permissions::MacHearingPermissions;
use async_trait::async_trait;
use coosenpai_core::ports::{
    PortError, SpeechPermissionKind, SpeechPermissionPort, SpeechPermissions,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Default)]
pub struct MacSpeechPermissions;

#[async_trait]
impl SpeechPermissionPort for MacSpeechPermissions {
    async fn request_recognition(
        &self,
        cancellation: CancellationToken,
    ) -> Result<SpeechPermissionKind, PortError> {
        if !objc2::available!(macos = 26.0) {
            return MacHearingPermissions
                .request_recognition(cancellation)
                .await;
        }
        if cancellation.is_cancelled() {
            return Err(PortError::Unavailable(
                "音声入力の準備を取り消しました".to_owned(),
            ));
        }
        // SpeechAnalyzer に追加の音声認識 TCC は不要。IPC の既存値で利用可能を表す。
        Ok(SpeechPermissionKind::Granted)
    }

    fn current(&self) -> Result<SpeechPermissions, PortError> {
        if !objc2::available!(macos = 26.0) {
            return MacHearingPermissions.current();
        }
        Ok(SpeechPermissions {
            microphone: current_microphone()?,
            recognition: SpeechPermissionKind::Granted,
        })
    }

    async fn request(
        &self,
        cancellation: CancellationToken,
    ) -> Result<SpeechPermissions, PortError> {
        if !objc2::available!(macos = 26.0) {
            return MacHearingPermissions.request(cancellation).await;
        }
        Ok(SpeechPermissions {
            microphone: request_microphone(cancellation).await?,
            recognition: SpeechPermissionKind::Granted,
        })
    }
}

