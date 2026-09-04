use async_trait::async_trait;
use block2::RcBlock;
use coosenpai_core::ports::{
    PortError, SpeechPermissionKind, SpeechPermissionPort, SpeechPermissions,
};
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_speech::{SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Default)]
pub struct MacSpeechPermissions;

#[async_trait]
impl SpeechPermissionPort for MacSpeechPermissions {
    fn current(&self) -> Result<SpeechPermissions, PortError> {
        Ok(SpeechPermissions {
            microphone: current_microphone()?,
            recognition: current_recognition(),
        })
    }

    async fn request(
        &self,
        cancellation: CancellationToken,
    ) -> Result<SpeechPermissions, PortError> {
        let microphone = request_microphone(cancellation.child_token()).await?;
        if microphone != SpeechPermissionKind::Granted {
            return Ok(SpeechPermissions {
                microphone,
                recognition: current_recognition(),
            });
        }
        let recognition = request_recognition(cancellation).await?;
        Ok(SpeechPermissions {
            microphone,
            recognition,
        })
    }
}

async fn request_microphone(
    cancellation: CancellationToken,
) -> Result<SpeechPermissionKind, PortError> {
    let current = current_microphone()?;
    if current != SpeechPermissionKind::NotDetermined {
        return Ok(current);
    }
    let media_type = audio_media_type()?;
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));
    {
        let block = RcBlock::new(move |_granted: objc2::runtime::Bool| {
            if let Ok(mut sender) = tx.lock() {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(());
                }
            }
        });
        // SAFETY: `media_type` is Apple's AVMediaTypeAudio constant and AVFoundation copies the
        // block before this scope ends.
        unsafe {
            AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &block);
        }
    }
    tokio::select! {
        _ = cancellation.cancelled() => Err(PortError::Unavailable("マイク権限の確認を取り消しました".to_owned())),
        result = rx => {
            result.map_err(|_| PortError::Unavailable("マイク権限の結果を取得できませんでした".to_owned()))?;
            current_microphone()
        }
    }
}

async fn request_recognition(
    cancellation: CancellationToken,
) -> Result<SpeechPermissionKind, PortError> {
    let current = current_recognition();
    if current != SpeechPermissionKind::NotDetermined {
        return Ok(current);
    }
    let (tx, rx) = oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));
    {
        let block = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
            if let Ok(mut sender) = tx.lock() {
                if let Some(sender) = sender.take() {
                    let _ = sender.send(map_recognition(status));
                }
            }
        });
        // SAFETY: Speech.framework copies the block before this scope ends.
        unsafe {
            SFSpeechRecognizer::requestAuthorization(&block);
        }
    }
    tokio::select! {
        _ = cancellation.cancelled() => Err(PortError::Unavailable("音声認識権限の確認を取り消しました".to_owned())),
        result = rx => result.map_err(|_| PortError::Unavailable("音声認識権限の結果を取得できませんでした".to_owned())),
    }
}

fn current_microphone() -> Result<SpeechPermissionKind, PortError> {
    let media_type = audio_media_type()?;
    // SAFETY: `media_type` is Apple's AVMediaTypeAudio constant, the only accepted audio token.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
    Ok(map_microphone(status))
}

fn current_recognition() -> SpeechPermissionKind {
    // SAFETY: This class method has no arguments and returns the process authorization status.
    map_recognition(unsafe { SFSpeechRecognizer::authorizationStatus() })
}

fn audio_media_type() -> Result<&'static objc2_av_foundation::AVMediaType, PortError> {
    // SAFETY: Framework constants are initialized by dyld before use and live for the process.
    unsafe { AVMediaTypeAudio }.ok_or_else(|| {
        PortError::Unavailable("AVFoundation の音声 media type がありません".to_owned())
    })
}

fn map_microphone(status: AVAuthorizationStatus) -> SpeechPermissionKind {
    match status {
        AVAuthorizationStatus::NotDetermined => SpeechPermissionKind::NotDetermined,
        AVAuthorizationStatus::Authorized => SpeechPermissionKind::Granted,
        AVAuthorizationStatus::Denied => SpeechPermissionKind::Denied,
        AVAuthorizationStatus::Restricted => SpeechPermissionKind::Restricted,
        _ => SpeechPermissionKind::Unavailable,
    }
}

fn map_recognition(status: SFSpeechRecognizerAuthorizationStatus) -> SpeechPermissionKind {
    match status {
        SFSpeechRecognizerAuthorizationStatus::NotDetermined => SpeechPermissionKind::NotDetermined,
        SFSpeechRecognizerAuthorizationStatus::Authorized => SpeechPermissionKind::Granted,
        SFSpeechRecognizerAuthorizationStatus::Denied => SpeechPermissionKind::Denied,
        SFSpeechRecognizerAuthorizationStatus::Restricted => SpeechPermissionKind::Restricted,
        _ => SpeechPermissionKind::Unavailable,
    }
}

