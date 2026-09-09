use crate::bubbles::{self, BubbleRecord};
use crate::state::DesktopState;
use async_trait::async_trait;
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::RuntimeLogger;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub(crate) enum NoticeTarget {
    Status,
    Bubble,
}

const NOTICE_DURATION_MS: u64 = 3_000;
const COPY_NOTICE_DURATION_MS: u64 = 2_000;

#[async_trait]
trait NoticeWindowPort: Send + Sync {
    async fn show_status(&self, message: &str);
    async fn show_bubble(&self, message: &str, duration_ms: u64) -> Result<(), String>;
}

struct DesktopNoticeWindowPort(Arc<DesktopState>);

#[async_trait]
impl NoticeWindowPort for DesktopNoticeWindowPort {
    async fn show_status(&self, message: &str) {
        crate::capture::publish_transient_shortcut_error(self.0.clone(), message.to_owned()).await;
    }

    async fn show_bubble(&self, message: &str, duration_ms: u64) -> Result<(), String> {
        let config = self.0.runtime_config();
        let conversation_generation = self.0.bubbles.lock().await.conversation_generation();
        bubbles::show(
            self.0.clone(),
            BubbleRecord {
                id: format!("notice-{}", uuid::Uuid::new_v4()),
                created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                message: message.to_owned(),
                message_kind: "notice".to_owned(),
                notification_priority: "info".to_owned(),
                caused_by: None,
                display_name: self.0.runtime_snapshot().companion_display_name,
                persona: config.companion.persona,
                avatar_color: config.ui.avatar_color,
                conversation_generation,
                persistent: false,
                interaction: None,
            },
            duration_ms,
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }
}

pub(crate) async fn show_empty_clipboard(state: Arc<DesktopState>, target: NoticeTarget) {
    let port = DesktopNoticeWindowPort(state.clone());
    let config = state.runtime_config();
    let locale = Locale::from_config(&config.ui.language);
    let shortcut = crate::state::tutorial_state::shortcut_label(config.keymap.send_text.as_deref());
    let message = empty_clipboard_message_for_locale(&shortcut, locale);
    if let Err(error) = present_empty_clipboard(&port, &message, target).await {
        let _ = state.logger.write(
            "WARN",
            &format!("クリップボード通知の表示に失敗しました: error-type=bubble ({error})"),
        );
    }
}

pub(crate) async fn show_copy_completed(state: Arc<DesktopState>) {
    let port = DesktopNoticeWindowPort(state.clone());
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    if let Err(error) = port
        .show_bubble(
            text(TextKey::CopyCompleted, locale),
            COPY_NOTICE_DURATION_MS,
        )
        .await
    {
        let _ = state.logger.write(
            "WARN",
            &format!("コピー完了通知の表示に失敗しました: error-type=bubble ({error})"),
        );
    }
}

fn empty_clipboard_message_for_locale(shortcut: &str, locale: Locale) -> String {
    text(TextKey::EmptyClipboard, locale).replace("{shortcut}", shortcut)
}

async fn present_empty_clipboard(
    port: &dyn NoticeWindowPort,
    message: &str,
    target: NoticeTarget,
) -> Result<(), String> {
    match target {
        NoticeTarget::Status => {
            port.show_status(message).await;
            Ok(())
        }
        NoticeTarget::Bubble => port.show_bubble(message, NOTICE_DURATION_MS).await,
    }
}

