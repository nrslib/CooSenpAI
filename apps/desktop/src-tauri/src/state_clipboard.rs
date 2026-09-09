use crate::command_guard::{CommandContext, CommandSource, DesktopCommand, DispatchError};
use crate::state::DesktopState;
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::ports::ClipboardWriter;
use coosenpai_core::state::{ConversationEntry, ConversationRole};
use std::sync::Arc;

impl DesktopState {
    pub(crate) async fn command_copy_last_reply(
        self: &Arc<Self>,
        _context: &CommandContext,
    ) -> Result<(), String> {
        let locale = Locale::from_config(&self.runtime_config().ui.language);
        let snapshot = self.snapshot().await;
        let copied = copy_latest_reply(&snapshot.conversation, self.clipboard_writer.as_ref())
            .map_err(|error| {
                text(TextKey::CopyLastReplyFailed, locale).replace("{error}", &error)
            })?;
        if !copied {
            return Err(text(TextKey::CopyLastReplyEmpty, locale).to_owned());
        }
        crate::capture_notice::show_copy_completed(self.clone()).await;
        Ok(())
    }
}

pub(crate) async fn dispatch_copy_last_reply_shortcut(
    state: Arc<DesktopState>,
) -> Result<(), DispatchError> {
    let handler_state = state.clone();
    state
        .dispatch(
            CommandSource::GlobalShortcut,
            DesktopCommand::CopyLastReply,
            move |context| async move {
                handler_state
                    .command_copy_last_reply(&context)
                    .await
                    .map_err(DispatchError::handler)
            },
        )
        .await
}

fn copy_latest_reply(
    conversation: &[ConversationEntry],
    clipboard: &dyn ClipboardWriter,
) -> Result<bool, String> {
    let Some(reply) = conversation
        .iter()
        .rev()
        .find(|entry| entry.role == ConversationRole::Companion)
    else {
        return Ok(false);
    };
    clipboard
        .write_text(&reply.message)
        .map_err(|error| error.to_string())?;
    Ok(true)
}

