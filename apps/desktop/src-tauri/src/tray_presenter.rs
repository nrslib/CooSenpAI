use crate::snapshot::AppSnapshot;
use crate::ui_events::{Handling, UiEffect, UiEvent};
use coosenpai_core::config::Config;
use coosenpai_core::locale::{text, Locale, TextKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayIcon {
    Paused,
    Watching,
    Recording,
}

#[derive(Debug, Clone)]
pub(crate) struct TrayView {
    pub start_enabled: bool,
    pub stop_enabled: bool,
    pub reset_enabled: bool,
    pub titles: [String; 7],
    pub shortcuts: Vec<String>,
    pub tooltip: String,
    pub icon: TrayIcon,
}

#[derive(Default)]
pub(crate) struct TrayPresenter {
    recording: bool,
    mounted: bool,
    revision: u64,
}

impl TrayPresenter {
    pub(crate) fn handle(&mut self, event: UiEvent) -> Handling {
        let snapshot = match event {
            UiEvent::TrayReady(snapshot) => {
                self.mounted = true;
                snapshot
            }
            UiEvent::SnapshotUpdated(snapshot) => snapshot,
            event => return Handling::Bubble(event),
        };
        if !self.mounted || snapshot.revision < self.revision {
            return Handling::Handled(Vec::new());
        }
        self.revision = snapshot.revision;
        Handling::Handled(vec![UiEffect::TrayRender(Box::new(
            self.present(&snapshot),
        ))])
    }

    fn present(&mut self, snapshot: &AppSnapshot) -> TrayView {
        match snapshot.speech.phase.as_str() {
            "starting" => self.recording = true,
            "idle" | "confirming" => self.recording = false,
            _ => {}
        }
        let locale = Locale::from_config(&snapshot.config.ui.language);
        let (start_enabled, stop_enabled, reset_enabled) = tray_availability(
            snapshot.onboarding.setup_required,
            snapshot.watch_intent_active,
        );
        let (icon, label) = if self.recording {
            (TrayIcon::Recording, TextKey::TrayRecording)
        } else if snapshot.watch_intent_active {
            (TrayIcon::Watching, TextKey::TrayWatching)
        } else {
            (TrayIcon::Paused, TextKey::TrayPaused)
        };
        TrayView {
            start_enabled,
            stop_enabled,
            reset_enabled: reset_enabled && !snapshot.onboarding.tutorial_active,
            titles: [
                TextKey::WindowOpen,
                TextKey::WindowWatchStart,
                TextKey::WindowWatchStop,
                TextKey::WindowSettings,
                TextKey::WindowResetConversation,
                TextKey::WindowShortcuts,
                TextKey::WindowQuit,
            ]
            .map(|key| text(key, locale).to_owned()),
            shortcuts: shortcut_menu_labels_for_locale(&snapshot.config, locale),
            tooltip: format!(
                "{}: {}",
                snapshot.companion_display_name,
                text(label, locale)
            ),
            icon,
        }
    }
}

pub(crate) fn tray_watch_actions(intent_active: bool) -> (bool, bool) {
    (!intent_active, intent_active)
}

pub(crate) fn tray_availability(setup_required: bool, intent_active: bool) -> (bool, bool, bool) {
    let (start, stop) = tray_watch_actions(intent_active);
    (
        !setup_required && start,
        !setup_required && stop,
        !setup_required,
    )
}

pub(crate) fn shortcut_menu_labels_for_locale(config: &Config, locale: Locale) -> Vec<String> {
    [
        (
            TextKey::ShortcutSendText,
            config.keymap.send_text.as_deref(),
        ),
        (
            TextKey::ShortcutCaptureRegion,
            config.keymap.capture_region.as_deref(),
        ),
        (
            TextKey::ShortcutMicrophone,
            config.keymap.microphone.as_deref(),
        ),
        (
            TextKey::ShortcutTogglePanel,
            config.keymap.toggle_panel.as_deref(),
        ),
        (
            TextKey::ShortcutToggleAvatar,
            config.keymap.toggle_avatar.as_deref(),
        ),
        (
            TextKey::ShortcutToggleWatch,
            config.keymap.toggle_watch.as_deref(),
        ),
        (
            TextKey::ShortcutCopyLastReply,
            config.keymap.copy_last_reply.as_deref(),
        ),
    ]
    .into_iter()
    .map(|(label, shortcut)| {
        format!(
            "{}: {}",
            text(label, locale),
            shortcut.unwrap_or(text(TextKey::ShortcutUnset, locale))
        )
    })
    .collect()
}
