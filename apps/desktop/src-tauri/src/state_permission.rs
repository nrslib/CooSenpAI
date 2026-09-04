use super::*;
use coosenpai_core::ports::{SpeechPermissionKind, SpeechPermissionPort, SpeechPermissions};

pub(crate) fn current_speech_permissions(logger: &dyn RuntimeLogger) -> SpeechPermissions {
    match crate::platform::MacSpeechPermissions.current() {
        Ok(permissions) => permissions,
        Err(error) => {
            let _ = logger.write("WARN", &format!("音声権限の状態を取得できません: {error}"));
            SpeechPermissions {
                microphone: SpeechPermissionKind::Unavailable,
                recognition: SpeechPermissionKind::Unavailable,
            }
        }
    }
}

impl DesktopState {
    pub(crate) async fn request_speech_permissions(&self) {
        match crate::platform::MacSpeechPermissions
            .request(self.cancellation.child_token())
            .await
        {
            Ok(permissions) => {
                self.publish(|snapshot| {
                    snapshot.speech.microphone_permission =
                        crate::speech::permission_name(permissions.microphone);
                    snapshot.speech.recognition_permission =
                        crate::speech::permission_name(permissions.recognition);
                })
                .await;
            }
            Err(error) => {
                let _ = self
                    .logger
                    .write("WARN", &format!("音声権限を要求できません: {error}"));
            }
        }
    }

    pub(crate) async fn request_screen_permission_for_watch(
        &self,
    ) -> coosenpai_core::ports::ScreenCapturePermission {
        let current = *self.screen_permission.lock().await;
        let permission = if current.requestable {
            crate::platform::request_screen_capture_permission()
        } else {
            crate::platform::screen_capture_permission()
        };
        self.publish_screen_permission(permission).await;
        let presentation = permission.presentation();
        let _ = self
            .logger
            .write("INFO", &format!("画面収録権限: {}", presentation.status));
        permission
    }

    pub(crate) async fn request_screen_permission_for_audio(
        &self,
    ) -> coosenpai_core::ports::ScreenCapturePermission {
        #[cfg(test)]
        if let Some(permission) = *self.screen_permission_override.lock().await {
            self.publish_screen_permission(permission).await;
            return permission;
        }
        let current = *self.screen_permission.lock().await;
        let permission = if current.requestable {
            crate::platform::request_screen_capture_permission()
        } else {
            crate::platform::screen_capture_permission()
        };
        self.publish_screen_permission(permission).await;
        let presentation = permission.presentation();
        let _ = self.logger.write(
            "INFO",
            &format!("耳の画面収録権限: {}", presentation.status),
        );
        permission
    }

    pub(crate) async fn record_screen_capture_result(&self, succeeded: bool) {
        let permission = if succeeded {
            (*self.screen_permission.lock().await).with_capture_result(true)
        } else {
            crate::platform::screen_capture_permission().with_capture_result(false)
        };
        self.publish_screen_permission(permission).await;
    }

    async fn publish_screen_permission(
        &self,
        permission: coosenpai_core::ports::ScreenCapturePermission,
    ) {
        let mut current = self.screen_permission.lock().await;
        if *current == permission {
            return;
        }
        *current = permission;
        drop(current);
        let presentation = permission.presentation();
        self.publish(|snapshot| {
            snapshot.screen_recording_status = presentation.status.to_owned();
            snapshot.screen_recording_message = presentation.message.map(str::to_owned);
            snapshot.screen_recording_restart_required = permission.requires_restart();
            snapshot.audio.screen_capture_permission = presentation.status.to_owned();
        })
        .await;
    }
}
