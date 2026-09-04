use super::DesktopState;
use coosenpai_core::state::ObservationRecord;
use std::path::PathBuf;

pub(crate) enum UserMessageAttachment {
    None,
    Image(PathBuf),
    Text(String),
}

impl DesktopState {
    pub(super) async fn enqueue_user_message_raw(
        &self,
        message: String,
        caused_by: Vec<ObservationRecord>,
        attachment: UserMessageAttachment,
        tutorial_response_key: Option<&str>,
    ) -> Result<String, String> {
        let tutorial_response_key = tutorial_response_key.map(str::to_owned);
        let result = match attachment {
            UserMessageAttachment::None => {
                self.runtime
                    .enqueue_user_message_with_attachment_and_tutorial_response(
                        message,
                        caused_by,
                        None,
                        tutorial_response_key,
                    )
                    .await
            }
            UserMessageAttachment::Image(path) => {
                self.runtime
                    .enqueue_user_message_with_attachment_and_tutorial_response(
                        message,
                        caused_by,
                        Some(path),
                        tutorial_response_key,
                    )
                    .await
            }
            UserMessageAttachment::Text(text) => {
                self.runtime
                    .enqueue_user_message_with_text_attachment_and_tutorial_response(
                        message,
                        caused_by,
                        text,
                        tutorial_response_key,
                    )
                    .await
            }
        }
        .map_err(|error| error.to_string())?;
        self.refresh_conversation().await;
        Ok(result)
    }
}
