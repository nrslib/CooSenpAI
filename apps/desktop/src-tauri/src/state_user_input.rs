use super::DesktopState;
use coosenpai_core::locale::Locale;
use coosenpai_core::runtime::{RuntimeError, RuntimeHandle};
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
        let result = enqueue_user_message(
            &self.runtime,
            message,
            caused_by,
            attachment,
            tutorial_response_key,
        )
        .await
        .map_err(|error| {
            error.format_for_locale(Locale::from_config(&self.runtime_config().ui.language))
        })?;
        self.refresh_conversation().await;
        Ok(result)
    }
}

pub(super) async fn enqueue_user_message(
    runtime: &RuntimeHandle,
    message: String,
    caused_by: Vec<ObservationRecord>,
    attachment: UserMessageAttachment,
    tutorial_response_key: Option<&str>,
) -> Result<String, RuntimeError> {
    let tutorial_response_key = tutorial_response_key.map(str::to_owned);
    match attachment {
        UserMessageAttachment::None => {
            runtime
                .enqueue_user_message_with_attachment_and_tutorial_response(
                    message,
                    caused_by,
                    None,
                    tutorial_response_key,
                )
                .await
        }
        UserMessageAttachment::Image(path) => {
            runtime
                .enqueue_user_message_with_attachment_and_tutorial_response(
                    message,
                    caused_by,
                    Some(path),
                    tutorial_response_key,
                )
                .await
        }
        UserMessageAttachment::Text(text) => {
            runtime
                .enqueue_user_message_with_text_attachment_and_tutorial_response(
                    message,
                    caused_by,
                    text,
                    tutorial_response_key,
                )
                .await
        }
    }
}
