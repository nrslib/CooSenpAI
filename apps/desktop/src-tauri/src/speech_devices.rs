use super::*;

impl SpeechController {
    pub(super) fn resolve_input_device(&self, configured: &str) -> (String, Option<String>) {
        if configured == "default" {
            return ("default".to_owned(), None);
        }
        match self.input_devices.input_devices() {
            Ok(devices) if devices.iter().any(|device| device.id == configured) => {
                (configured.to_owned(), None)
            }
            Ok(_) => (
                "default".to_owned(),
                Some("選択したマイクが見つからないため、システム既定を使います".to_owned()),
            ),
            Err(_) => (
                "default".to_owned(),
                Some("マイク一覧を取得できないため、システム既定を使います".to_owned()),
            ),
        }
    }

    pub fn input_devices(&self) -> Vec<SpeechInputDevice> {
        self.input_devices.input_devices().unwrap_or_default()
    }

    pub async fn refresh_input_devices(&self, state: &DesktopState) {
        match self.input_devices.input_devices() {
            Ok(devices) => {
                state
                    .publish(|snapshot| snapshot.speech.input_devices = devices)
                    .await;
            }
            Err(error) => {
                let original = error.to_string();
                let message = super::support::present_speech_error(
                    state,
                    Some("input-device-list"),
                    &original,
                )
                .message
                .to_owned();
                state
                    .publish(|snapshot| {
                        apply_warning(&mut snapshot.speech, "input-device-list", message)
                    })
                    .await;
            }
        }
    }
}
