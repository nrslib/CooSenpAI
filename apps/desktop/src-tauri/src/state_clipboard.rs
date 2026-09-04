use crate::command_guard::{CommandContext, CommandSource, DesktopCommand, DispatchError};
use crate::state::DesktopState;
use coosenpai_core::ports::ClipboardWriter;
use coosenpai_core::state::{ConversationEntry, ConversationRole};
use std::sync::Arc;

impl DesktopState {
    pub(crate) async fn command_copy_last_reply(
        self: &Arc<Self>,
        _context: &CommandContext,
    ) -> Result<(), String> {
        let snapshot = self.snapshot().await;
        if !copy_latest_reply(&snapshot.conversation, self.clipboard_writer.as_ref())? {
            return Err("コピーできる返事がありません".to_owned());
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
        .map_err(|error| format!("返事をコピーできませんでした: {error}"))?;
    Ok(true)
}

