use super::*;

impl SpeechController {

    pub(super) fn resolve_input_device_for_locale(
        &self,
        configured: &str,
        locale: coosenpai_core::locale::Locale,
    ) -> (String, Option<String>) {
        if configured == "default" {
            return ("default".to_owned(), None);
        }
        match self.input_devices.input_devices() {
            Ok(devices) if devices.iter().any(|device| device.id == configured) => {
                (configured.to_owned(), None)
            }
            Ok(_) => (
                "default".to_owned(),
                Some(
                    coosenpai_core::locale::text(
                        coosenpai_core::locale::TextKey::SpeechInputDeviceFallback,
                        locale,
                    )
                    .to_owned(),
                ),
            ),
            Err(_) => (
                "default".to_owned(),
                Some(
                    coosenpai_core::locale::text(
                        coosenpai_core::locale::TextKey::SpeechInputDeviceListFallback,
                        locale,
                    )
                    .to_owned(),
                ),
            ),
        }
    }

    pub fn input_devices(&self) -> Vec<SpeechInputDevice> {
        self.input_devices.input_devices().unwrap_or_default()
    }

    pub async fn refresh_input_devices(&self, state: &DesktopState) {
        let result = self
            .input_devices
            .input_devices()
            .map_err(|error| error.to_string());
        state
            .publish_event(crate::snapshot_presenter::SnapshotEvent::SpeechDevicesLoaded(result))
            .await;
    }
}
