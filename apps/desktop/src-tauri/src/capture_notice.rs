use crate::bubbles::{self, BubbleRecord};
use crate::state::DesktopState;
use async_trait::async_trait;
use coosenpai_core::ports::RuntimeLogger;
use std::sync::Arc;
use tauri::Manager;

const NOTICE_DURATION_MS: u64 = 3_000;
const COPY_COMPLETED_MESSAGE: &str = "コピーしました";
const COPY_NOTICE_DURATION_MS: u64 = 2_000;

#[async_trait]
trait NoticeWindowPort: Send + Sync {
    fn main_is_foreground(&self) -> bool;
    async fn show_status(&self, message: &str);
    async fn show_bubble(&self, message: &str, duration_ms: u64) -> Result<(), String>;
}

struct DesktopNoticeWindowPort(Arc<DesktopState>);

#[async_trait]
impl NoticeWindowPort for DesktopNoticeWindowPort {
    fn main_is_foreground(&self) -> bool {
        self.0.app.get_webview_window("main").is_some_and(|window| {
            window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
        })
    }

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
                open_url: None,
                interaction: None,
            },
            duration_ms,
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
    }
}

pub(crate) async fn show_empty_clipboard(state: Arc<DesktopState>) {
    let port = DesktopNoticeWindowPort(state.clone());
    let config = state.runtime_config();
    let shortcut = crate::state::tutorial_state::shortcut_label(config.keymap.send_text.as_deref());
    let message = empty_clipboard_message(&shortcut);
    if let Err(error) = present_empty_clipboard(&port, &message).await {
        let _ = state.logger.write(
            "WARN",
            &format!("クリップボード通知の表示に失敗しました: error-type=bubble ({error})"),
        );
    }
}

pub(crate) async fn show_copy_completed(state: Arc<DesktopState>) {
    let port = DesktopNoticeWindowPort(state.clone());
    if let Err(error) = port
        .show_bubble(COPY_COMPLETED_MESSAGE, COPY_NOTICE_DURATION_MS)
        .await
    {
        let _ = state.logger.write(
            "WARN",
            &format!("コピー完了通知の表示に失敗しました: error-type=bubble ({error})"),
        );
    }
}

fn empty_clipboard_message(shortcut: &str) -> String {
    format!("文章を選んで {shortcut} を押してください")
}

async fn present_empty_clipboard(port: &dyn NoticeWindowPort, message: &str) -> Result<(), String> {
    present_notice(port, message).await
}

async fn present_notice(port: &dyn NoticeWindowPort, message: &str) -> Result<(), String> {
    if port.main_is_foreground() {
        port.show_status(message).await;
        Ok(())
    } else {
        port.show_bubble(message, NOTICE_DURATION_MS).await
    }
}

