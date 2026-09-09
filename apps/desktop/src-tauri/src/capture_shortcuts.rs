use super::{
    clear_shortcut_error_if_current, clear_speech_shortcut_error_if_current,
    current_shortcut_error_token, publish_shortcut_error, publish_speech_shortcut_error,
    replace_shortcuts, DesktopState, ShortcutRegistrar, ShortcutReplacement,
};
use coosenpai_core::locale::{localize_shortcut_message, text, Locale, TextKey};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::AppHandle;
use tauri_plugin_global_shortcut::GlobalShortcutExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutAction {
    CaptureRegion,
    SendText,
    Microphone,
    SpeechCancel,
    PopupCancel,
    TogglePanel,
    ToggleAvatar,
    ToggleWatch,
    CopyLastReply,
}

const SHORTCUT_ACTIONS: [ShortcutAction; 9] = [
    ShortcutAction::CaptureRegion,
    ShortcutAction::SendText,
    ShortcutAction::Microphone,
    ShortcutAction::SpeechCancel,
    ShortcutAction::PopupCancel,
    ShortcutAction::TogglePanel,
    ShortcutAction::ToggleAvatar,
    ShortcutAction::ToggleWatch,
    ShortcutAction::CopyLastReply,
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShortcutBindings(pub(crate) Vec<(String, ShortcutAction)>);

#[derive(Default)]
pub struct ShortcutCoordinator {
    state: std::sync::Mutex<ShortcutCoordinatorState>,
    routes: [AtomicU64; 9],
    popup_generation: AtomicU64,
}

#[derive(Default)]
struct ShortcutCoordinatorState {
    active: ShortcutBindings,
    configured: ShortcutBindings,
    config_version: u64,
    speech_cancel_generation: Option<u64>,
    speech_lifecycle_revision: u64,
    popup_cancel: Option<PopupCancelTarget>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PopupCancelTarget {
    pub generation: u64,
}

impl ShortcutCoordinator {
    pub fn action(&self, shortcut: &str) -> Option<ShortcutAction> {
        let id = shortcut
            .parse::<tauri_plugin_global_shortcut::Shortcut>()
            .ok()?
            .id() as u64
            + 1;
        SHORTCUT_ACTIONS
            .iter()
            .copied()
            .find(|action| self.routes[*action as usize].load(Ordering::Acquire) == id)
    }

    fn publish_routes(&self, active: &ShortcutBindings) {
        for action in SHORTCUT_ACTIONS {
            let id = active
                .0
                .iter()
                .find(|(_, registered)| *registered == action)
                .map(|(shortcut, _)| {
                    shortcut
                        .parse::<tauri_plugin_global_shortcut::Shortcut>()
                        .expect("登録済みショートカット")
                        .id() as u64
                        + 1
                })
                .unwrap_or(0);
            self.routes[action as usize].store(id, Ordering::Release);
        }
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
        // 表示中のEscを設定変更の登録し直しに巻き込まない。
        let previous_config = state.active.clone().with_cancel(false, false);
        let cancel = state
            .active
            .0
            .iter()
            .find(|(_, action)| {
                matches!(
                    action,
                    ShortcutAction::SpeechCancel | ShortcutAction::PopupCancel
                )
            })
            .cloned();
        let mut outcome = replace_shortcuts(registrar, &previous_config, &configured);
        if let Some(cancel) = cancel {
            outcome.active.0.push(cancel);
        }
        state.active = outcome.active.clone();
        self.publish_routes(&state.active);
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
        if state.popup_cancel.is_some() {
            state.speech_cancel_generation = generation;
            state.speech_lifecycle_revision = lifecycle_revision;
            return ShortcutReplacement {
                active: state.active.clone(),
                error: None,
                accepted: true,
            };
        }
        let desired = state
            .configured
            .clone()
            .with_cancel(generation.is_some(), state.popup_cancel.is_some());
        let outcome = replace_shortcuts(registrar, &state.active, &desired);
        state.active = outcome.active.clone();
        self.publish_routes(&state.active);
        state.speech_lifecycle_revision = lifecycle_revision;
        if outcome.error.is_none() {
            state.speech_cancel_generation = generation;
        }
        outcome
    }

    pub(crate) fn popup_cancel_target(&self) -> Option<PopupCancelTarget> {
        let generation = self.popup_generation.load(Ordering::Acquire);
        (generation != 0).then_some(PopupCancelTarget { generation })
    }

    pub(crate) fn install_popup_cancel(
        &self,
        registrar: &dyn ShortcutRegistrar,
        target: PopupCancelTarget,
    ) -> Result<(), String> {
        let mut state = self.lock();
        if state.active.action("Escape").is_none() {
            registrar
                .register("Escape")
                .map_err(|()| "送信ポップアップのEsc取消を登録できません".to_owned())?;
        }
        state.active = state
            .active
            .clone()
            .with_cancel(state.speech_cancel_generation.is_some(), true);
        state.popup_cancel = Some(target);
        self.popup_generation
            .store(target.generation, Ordering::Release);
        self.publish_routes(&state.active);
        Ok(())
    }

    pub(crate) fn remove_popup_cancel(
        &self,
        registrar: &dyn ShortcutRegistrar,
        generation: u64,
    ) -> Result<(), String> {
        let mut state = self.lock();
        if !state
            .popup_cancel
            .is_some_and(|target| target.generation == generation)
        {
            return Ok(());
        }
        if state.speech_cancel_generation.is_none() {
            registrar.unregister("Escape");
        }
        state.active = state
            .active
            .clone()
            .with_cancel(state.speech_cancel_generation.is_some(), false);
        state.popup_cancel = None;
        self.publish_routes(&state.active);
        self.popup_generation.store(0, Ordering::Release);
        Ok(())
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
                    config.keymap.toggle_avatar.clone(),
                    ShortcutAction::ToggleAvatar,
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

    fn validate_unique_for_locale(&self, locale: Locale) -> Result<(), String> {
        let mut identities = std::collections::HashSet::new();
        for (shortcut, _) in &self.0 {
            shortcut
                .parse::<tauri_plugin_global_shortcut::Shortcut>()
                .map_err(|_| {
                    text(TextKey::ShortcutInvalid, locale).replace("{shortcut}", shortcut)
                })?;
            let identity =
                coosenpai_core::config::shortcut_identity(shortcut).ok_or_else(|| {
                    text(TextKey::ShortcutInvalid, locale).replace("{shortcut}", shortcut)
                })?;
            if !identities.insert(identity) {
                return Err(
                    text(TextKey::ShortcutDuplicate, locale).replace("{shortcut}", shortcut)
                );
            }
        }
        Ok(())
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = &(String, ShortcutAction)> {
        self.0.iter()
    }

    fn with_cancel(mut self, speech: bool, popup: bool) -> Self {
        self.0.retain(|(_, action)| {
            !matches!(
                action,
                ShortcutAction::SpeechCancel | ShortcutAction::PopupCancel
            )
        });
        if popup || speech {
            self.0.push((
                "Escape".to_owned(),
                if popup {
                    ShortcutAction::PopupCancel
                } else {
                    ShortcutAction::SpeechCancel
                },
            ));
        }
        self
    }
}

pub(crate) struct TauriShortcutRegistrar<'a>(pub(crate) &'a AppHandle);

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
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let previous_error = current_shortcut_error_token(state).await;
    if let Err(error) = next.validate_unique_for_locale(locale) {
        let error = localize_shortcut_message(&error, locale);
        publish_shortcut_error(state, error.clone()).await;
        return Err(error);
    }
    let outcome = update_shortcuts_on_main_thread(state, move |coordinator, registrar| {
        coordinator.replace_config(registrar, next, config_version)
    })
    .await?;
    if !outcome.accepted {
        return Ok(());
    }
    match outcome.error {
        None => {
            clear_shortcut_error_if_current(state, previous_error).await;
            Ok(())
        }
        Some(message) => {
            let message = localize_shortcut_message(&message, locale);
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
    let outcome = match update_shortcuts_on_main_thread(state, move |coordinator, registrar| {
        coordinator.replace_speech_cancel(registrar, speech.cancel_generation, speech.revision)
    })
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            publish_speech_shortcut_error(state, speech.revision, error).await;
            return;
        }
    };
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

// OS登録も更新の排他もメインスレッドに揃え、native表示中の登録と相互待ちにしない。
async fn update_shortcuts_on_main_thread(
    state: &DesktopState,
    update: impl FnOnce(&ShortcutCoordinator, &dyn ShortcutRegistrar) -> ShortcutReplacement
        + Send
        + 'static,
) -> Result<ShortcutReplacement, String> {
    let coordinator = state.shortcut_coordinator.clone();
    let app = state.app.clone();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    state
        .app
        .run_on_main_thread(move || {
            let outcome = update(&coordinator, &TauriShortcutRegistrar(&app));
            let _ = sender.send(outcome);
        })
        .map_err(|error| error.to_string())?;
    receiver.await.map_err(|error| error.to_string())
}
