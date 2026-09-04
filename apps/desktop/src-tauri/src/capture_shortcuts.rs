use super::{
    clear_shortcut_error_if_current, clear_speech_shortcut_error_if_current,
    current_shortcut_error_token, publish_shortcut_error, publish_speech_shortcut_error,
    replace_shortcuts, DesktopState, ShortcutRegistrar, ShortcutReplacement,
};
use tauri::AppHandle;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutAction {
    CaptureRegion,
    SendText,
    Microphone,
    SpeechCancel,
    TogglePanel,
    ToggleWatch,
    CopyLastReply,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShortcutBindings(pub(crate) Vec<(String, ShortcutAction)>);

#[derive(Default)]
pub struct ShortcutCoordinator {
    state: std::sync::Mutex<ShortcutCoordinatorState>,
}

#[derive(Default)]
struct ShortcutCoordinatorState {
    active: ShortcutBindings,
    configured: ShortcutBindings,
    config_version: u64,
    speech_cancel_generation: Option<u64>,
    speech_lifecycle_revision: u64,
}

impl ShortcutCoordinator {
    pub fn action(&self, shortcut: &str) -> Option<ShortcutAction> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.active.action(shortcut))
    }

    pub(super) fn replace_config(
        &self,
        registrar: &dyn ShortcutRegistrar,
        configured: ShortcutBindings,
        version: u64,
    ) -> ShortcutReplacement {
        let mut state = self.lock();
        if version < state.config_version {
            return ShortcutReplacement {
                active: state.active.clone(),
                error: None,
                accepted: false,
            };
        }
        let desired = configured
            .clone()
            .with_speech_cancel(state.speech_cancel_generation.is_some());
        let outcome = replace_shortcuts(registrar, &state.active, &desired);
        state.active = outcome.active.clone();
        if outcome.error.is_none() {
            state.configured = configured;
            state.config_version = version;
        }
        outcome
    }

    pub(super) fn replace_speech_cancel(
        &self,
        registrar: &dyn ShortcutRegistrar,
        generation: Option<u64>,
        lifecycle_revision: u64,
    ) -> ShortcutReplacement {
        let mut state = self.lock();
        if lifecycle_revision < state.speech_lifecycle_revision {
            return ShortcutReplacement {
                active: state.active.clone(),
                error: None,
                accepted: false,
            };
        }
        let desired = state
            .configured
            .clone()
            .with_speech_cancel(generation.is_some());
        let outcome = replace_shortcuts(registrar, &state.active, &desired);
        state.active = outcome.active.clone();
        state.speech_lifecycle_revision = lifecycle_revision;
        if outcome.error.is_none() {
            state.speech_cancel_generation = generation;
        }
        outcome
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ShortcutCoordinatorState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub(super) fn accepts_speech_revision(&self, revision: u64) -> bool {
        self.lock().speech_lifecycle_revision == revision
    }
}

impl ShortcutBindings {
    pub fn from_config(config: &coosenpai_core::config::Config) -> Self {
        Self(
            [
                (
                    config.keymap.capture_region.clone(),
                    ShortcutAction::CaptureRegion,
                ),
                (config.keymap.microphone.clone(), ShortcutAction::Microphone),
                (config.keymap.send_text.clone(), ShortcutAction::SendText),
                (
                    config.keymap.copy_last_reply.clone(),
                    ShortcutAction::CopyLastReply,
                ),
                (
                    config.keymap.toggle_panel.clone(),
                    ShortcutAction::TogglePanel,
                ),
                (
                    config.keymap.toggle_watch.clone(),
                    ShortcutAction::ToggleWatch,
                ),
            ]
            .into_iter()
            .filter_map(|(shortcut, action)| shortcut.map(|shortcut| (shortcut, action)))
            .collect(),
        )
    }

    pub fn action(&self, shortcut: &str) -> Option<ShortcutAction> {
        let incoming = shortcut
            .parse::<tauri_plugin_global_shortcut::Shortcut>()
            .ok()?;
        self.0.iter().find_map(|(value, action)| {
            value
                .parse::<tauri_plugin_global_shortcut::Shortcut>()
                .ok()
                .filter(|registered| registered.id() == incoming.id())
                .map(|_| *action)
        })
    }

    pub fn validate_unique(&self) -> Result<(), String> {
        let mut identities = std::collections::HashSet::new();
        for (shortcut, _) in &self.0 {
            shortcut
                .parse::<tauri_plugin_global_shortcut::Shortcut>()
                .map_err(|_| format!("ショートカット {shortcut} を解釈できません。"))?;
            let identity = coosenpai_core::config::shortcut_identity(shortcut)
                .ok_or_else(|| format!("ショートカット {shortcut} を解釈できません。"))?;
            if !identities.insert(identity) {
                return Err(format!(
                    "ショートカット {shortcut} が別の操作と重複しています。"
                ));
            }
        }
        Ok(())
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = &(String, ShortcutAction)> {
        self.0.iter()
    }

    pub(super) fn with_speech_cancel(mut self, enabled: bool) -> Self {
        self.0
            .retain(|(_, action)| *action != ShortcutAction::SpeechCancel);
        if enabled {
            self.0
                .push(("Escape".to_owned(), ShortcutAction::SpeechCancel));
        }
        self
    }
}

struct TauriShortcutRegistrar<'a>(&'a AppHandle);

impl ShortcutRegistrar for TauriShortcutRegistrar<'_> {
    fn register(&self, shortcut: &str) -> Result<(), ()> {
        self.0.global_shortcut().register(shortcut).map_err(|_| ())
    }

    fn unregister(&self, shortcut: &str) {
        let _ = self.0.global_shortcut().unregister(shortcut);
    }
}

pub async fn sync_shortcuts(
    state: &DesktopState,
    next: ShortcutBindings,
    config_version: u64,
) -> Result<(), String> {
    let previous_error = current_shortcut_error_token(state).await;
    if let Err(error) = next.validate_unique() {
        publish_shortcut_error(state, error.clone()).await;
        return Err(error);
    }
    let outcome = state.shortcut_coordinator.replace_config(
        &TauriShortcutRegistrar(&state.app),
        next,
        config_version,
    );
    if !outcome.accepted {
        return Ok(());
    }
    match outcome.error {
        None => {
            clear_shortcut_error_if_current(state, previous_error).await;
            Ok(())
        }
        Some(message) => {
            publish_shortcut_error(state, message.clone()).await;
            Err(message)
        }
    }
}

pub async fn refresh_speech_cancel_shortcut(
    state: &DesktopState,
    speech: crate::speech_lifecycle::SpeechShortcutState,
) {
    let previous_error = current_shortcut_error_token(state).await;
    let outcome = state.shortcut_coordinator.replace_speech_cancel(
        &TauriShortcutRegistrar(&state.app),
        speech.cancel_generation,
        speech.revision,
    );
    if !outcome.accepted {
        return;
    }
    match outcome.error {
        None => {
            clear_speech_shortcut_error_if_current(state, speech.revision, previous_error).await;
        }
        Some(error) => {
            publish_speech_shortcut_error(state, speech.revision, error).await;
        }
    }
}
