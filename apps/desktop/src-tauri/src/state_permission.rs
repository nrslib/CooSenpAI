use super::*;
use coosenpai_core::ports::{
    ScreenCapturePermission, ScreenCapturePermissionKind, SpeechPermissionKind,
    SpeechPermissionPort, SpeechPermissions,
};
use std::time::{Duration, Instant};

const SCREEN_PERMISSION_CACHE_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy)]
pub(super) struct ScreenPermissionCache {
    pub permission: ScreenCapturePermission,
    checked_at: Option<Instant>,
}

impl ScreenPermissionCache {
    pub(super) fn new(permission: ScreenCapturePermission, checked_at: Instant) -> Self {
        Self {
            permission,
            checked_at: Some(checked_at),
        }
    }

    fn age(&self, now: Instant) -> Option<Duration> {
        self.checked_at
            .map(|checked_at| now.saturating_duration_since(checked_at))
    }

}

#[derive(Clone, Copy)]
enum ScreenPermissionCacheUpdate {
    Keep,
    CheckedAt(Instant),
    Unconfirmed,
}

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

    pub(crate) async fn request_screen_permission_for_watch(&self) -> ScreenCapturePermission {
        let now = Instant::now();
        let (permission, source) = {
            let cache = self.screen_permission.lock().await;
            resolve_screen_permission(
                cache.permission,
                cache.age(now),
                crate::platform::request_screen_capture_permission,
                crate::platform::screen_capture_permission,
            )
        };
        self.publish_screen_permission(
            permission,
            if source == "cache" {
                ScreenPermissionCacheUpdate::Keep
            } else {
                ScreenPermissionCacheUpdate::CheckedAt(now)
            },
        )
        .await;
        let presentation = permission.presentation();
        let _ = self.logger.write(
            "INFO",
            &format!("画面収録権限: {} source={source}", presentation.status),
        );
        permission
    }

    pub(crate) async fn request_screen_permission_for_audio(&self) -> ScreenCapturePermission {
        #[cfg(test)]
        if let Some(permission) = *self.screen_permission_override.lock().await {
            self.publish_screen_permission(
                permission,
                ScreenPermissionCacheUpdate::CheckedAt(Instant::now()),
            )
            .await;
            return permission;
        }
        let current = self.screen_permission.lock().await.permission;
        let permission = if current.requestable {
            crate::platform::request_screen_capture_permission()
        } else {
            crate::platform::screen_capture_permission()
        };
        self.publish_screen_permission(
            permission,
            ScreenPermissionCacheUpdate::CheckedAt(Instant::now()),
        )
        .await;
        let presentation = permission.presentation();
        let _ = self.logger.write(
            "INFO",
            &format!("耳の画面収録権限: {}", presentation.status),
        );
        permission
    }

    pub(crate) async fn record_screen_capture_result(&self, succeeded: bool) {
        let permission = if succeeded {
            self.screen_permission
                .lock()
                .await
                .permission
                .with_capture_result(true)
        } else {
            crate::platform::screen_capture_permission().with_capture_result(false)
        };
        self.publish_screen_permission(
            permission,
            ScreenPermissionCacheUpdate::CheckedAt(Instant::now()),
        )
        .await;
    }

    pub(crate) async fn invalidate_screen_permission_cache(&self) {
        let permission = crate::platform::screen_capture_permission().with_capture_result(false);
        self.publish_screen_permission(permission, ScreenPermissionCacheUpdate::Unconfirmed)
            .await;
    }

    async fn publish_screen_permission(
        &self,
        permission: ScreenCapturePermission,
        cache_update: ScreenPermissionCacheUpdate,
    ) {
        let mut current = self.screen_permission.lock().await;
        let changed = current.permission != permission;
        current.permission = permission;
        match cache_update {
            ScreenPermissionCacheUpdate::Keep => {}
            ScreenPermissionCacheUpdate::CheckedAt(checked_at) => {
                current.checked_at = Some(checked_at);
            }
            ScreenPermissionCacheUpdate::Unconfirmed => current.checked_at = None,
        }
        drop(current);
        if !changed {
            return;
        }
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

fn should_refresh_screen_permission(
    permission: ScreenCapturePermission,
    cache_age: Option<Duration>,
) -> bool {
    permission.kind != ScreenCapturePermissionKind::Granted
        || permission.requires_restart()
        || cache_age.is_none_or(|age| age >= SCREEN_PERMISSION_CACHE_TTL)
}

fn resolve_screen_permission(
    permission: ScreenCapturePermission,
    cache_age: Option<Duration>,
    request: impl FnOnce() -> ScreenCapturePermission,
    preflight: impl FnOnce() -> ScreenCapturePermission,
) -> (ScreenCapturePermission, &'static str) {
    if !should_refresh_screen_permission(permission, cache_age) {
        return (permission, "cache");
    }
    let use_preflight = cache_age.is_none() || !permission.requestable;
    let permission = if use_preflight {
        preflight()
    } else {
        request()
    };
    let source = if use_preflight {
        "preflight"
    } else {
        "request"
    };
    (permission, source)
}

