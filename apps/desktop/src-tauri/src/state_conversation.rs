use super::*;
use coosenpai_core::conversation_archive::{
    current_conversation_generation, select_conversation_generation,
};
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::persistence::PersistenceError;

impl DesktopState {
    pub(crate) async fn switch_conversation_generation(
        self: &Arc<Self>,
        generation: u64,
    ) -> Result<(), RuntimeError> {
        let locale = Locale::from_config(&self.runtime_config().ui.language);
        let transaction = self.config_update.begin().await;
        let conversation_sync = self.conversation_sync.lock().await;
        let paths = self.paths.clone();
        let original_generation = current_generation(&paths, locale).await?;
        if let Err(error) = self.runtime.quiesce_for_conversation_reset().await {
            drop(conversation_sync);
            return Err(error);
        }
        self.deactivate_runtime().await;
        let config = self.runtime.config();
        self.clear_thought_bubble_for_conversation_switch().await;
        let transition = async {
            select_generation(&paths, generation, locale).await?;
            self.rebuild_runtime_after_conversation_reset(&config).await
        }
        .await;
        if let Err(error) = transition {
            let error = self
                .recover_conversation_switch_failure(
                    &paths,
                    original_generation,
                    &config,
                    locale,
                    error,
                )
                .await;
            drop(conversation_sync);
            return Err(error);
        }
        if let Err(error) = transaction
            .commit_while_holding()
            .map_err(|error| RuntimeError::Factory(error.to_string()))
        {
            let error = self
                .recover_conversation_switch_failure(
                    &paths,
                    original_generation,
                    &config,
                    locale,
                    error,
                )
                .await;
            drop(conversation_sync);
            return Err(error);
        }
        let changed = self
            .bubbles
            .lock()
            .await
            .switch_conversation_generation(generation);
        if changed {
            let _ = bubbles::sync_window(self).await;
        }
        self.activate_runtime();
        drop(conversation_sync);
        self.refresh_conversation().await;
        Ok(())
    }

    async fn recover_conversation_switch_failure(
        self: &Arc<Self>,
        paths: &ConfigPaths,
        original_generation: u64,
        config: &Config,
        locale: Locale,
        error: RuntimeError,
    ) -> RuntimeError {
        let recovery = async {
            self.runtime.quiesce_for_conversation_reset().await?;
            self.deactivate_runtime().await;
            select_generation(paths, original_generation, locale).await?;
            self.rebuild_runtime_after_conversation_reset(config)
                .await?;
            self.activate_runtime();
            Ok::<(), RuntimeError>(())
        }
        .await;
        switch_failure_result(error, recovery, locale)
    }
}

fn switch_failure_result(
    error: RuntimeError,
    recovery: Result<(), RuntimeError>,
    locale: Locale,
) -> RuntimeError {
    match recovery {
        Ok(()) => error,
        Err(recovery_error) => RuntimeError::Factory(
            text(TextKey::ConversationSwitchRecoveryFailed, locale)
                .replace("{error}", &error.format_for_locale(locale))
                .replace(
                    "{recovery_error}",
                    &recovery_error.format_for_locale(locale),
                ),
        ),
    }
}

async fn current_generation(paths: &ConfigPaths, locale: Locale) -> Result<u64, RuntimeError> {
    let paths = paths.clone();
    tokio::task::spawn_blocking(move || current_conversation_generation(&paths))
        .await
        .map_err(|error| RuntimeError::Factory(error.to_string()))?
        .map_err(|error| conversation_persistence_error(error, locale))
}

async fn select_generation(
    paths: &ConfigPaths,
    generation: u64,
    locale: Locale,
) -> Result<(), RuntimeError> {
    let paths = paths.clone();
    tokio::task::spawn_blocking(move || select_conversation_generation(&paths, generation))
        .await
        .map_err(|error| RuntimeError::Factory(error.to_string()))?
        .map_err(|error| conversation_persistence_error(error, locale))
}

fn conversation_persistence_error(error: PersistenceError, locale: Locale) -> RuntimeError {
    if locale == Locale::Ja {
        return RuntimeError::Factory(error.to_string());
    }
    if let PersistenceError::Invalid(detail) = &error {
        if let Some(generation) = detail.strip_prefix("選択する会話世代が存在しません: ")
        {
            return RuntimeError::Factory(
                text(TextKey::ConversationGenerationNotFound, locale)
                    .replace("{generation}", generation),
            );
        }
    }
    let detail = match error {
        PersistenceError::Io(_) => text(TextKey::ConversationDataReadWriteFailed, locale),
        PersistenceError::Json(_) | PersistenceError::Invalid(_) => {
            text(TextKey::ConversationDataInvalid, locale)
        }
        PersistenceError::AlreadyLocked => text(TextKey::ConversationDataLocked, locale),
    };
    RuntimeError::Factory(
        text(TextKey::ConversationGenerationOperationFailed, locale).replace("{error}", detail),
    )
}

