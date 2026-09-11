use super::*;
use crate::commands_config::{invalidates_running_operations, work_config_is_only_difference};
use crate::config_update::ConfigUpdateOutcome;
use coosenpai_core::config::audio_config_is_only_difference;
use coosenpai_core::locale::{text, Locale, TextKey};

impl DesktopState {
    pub(super) async fn update_config_with_raw<F>(
        self: &Arc<Self>,
        permit: &crate::command_guard::CommandContext,
        update: F,
        staged_avatar: Option<crate::avatar::StagedAvatar>,
        expected_revision: Option<u64>,
    ) -> Result<ConfigUpdateOutcome, ConfigCommitError>
    where
        F: FnOnce(Config) -> Result<Config, coosenpai_core::config::ConfigError>,
    {
        if permit.command() == crate::command_guard::DesktopCommand::ConfigAudioUpdate {
            return self
                .update_audio_config_with_raw(update, expected_revision)
                .await;
        }
        // 通常設定の大きな Future を音声操作や呼び出し元のスタックへ持ち回らない。
        Box::pin(self.update_config_transaction(permit, update, staged_avatar, expected_revision))
            .await
    }

    async fn update_config_transaction<F>(
        self: &Arc<Self>,
        permit: &crate::command_guard::CommandContext,
        update: F,
        mut staged_avatar: Option<crate::avatar::StagedAvatar>,
        expected_revision: Option<u64>,
    ) -> Result<ConfigUpdateOutcome, ConfigCommitError>
    where
        F: FnOnce(Config) -> Result<Config, coosenpai_core::config::ConfigError>,
    {
        let transaction = self.config_update.begin().await;
        transaction.ensure_expected_revision(expected_revision)?;
        let shortcut_version = transaction.base_revision.saturating_add(1);
        let previous = self.runtime.config();
        let avatar_updated = staged_avatar.is_some();
        let normal_mode =
            self.onboarding_policy_phase().await == crate::command_guard::OnboardingPhase::Normal;
        let guarded_update = move |current: Config| {
            let next = update(current.clone())?;
            crate::commands_config::validate_work_config_change(&current, &next, normal_mode)?;
            Ok(next)
        };
        let prepared = match prepare_config_update(
            &self.paths,
            &self.runtime,
            &previous,
            guarded_update,
            expected_revision,
        ) {
            Ok(persisted) => persisted,
            Err(error @ coosenpai_core::config::ConfigError::RevisionConflict { actual, .. }) => {
                self.config_update.observe_config_revision(actual);
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        };
        let persisted_before = prepared.previous.clone();
        let requested = prepared.requested.clone();
        let staged = prepared.staged.clone();
        let mut provider_start_gate = prepared.provider_start_gate;
        let persona_notice = persona_change_notice(
            &persisted_before,
            &staged,
            Locale::from_config(&staged.ui.language),
        );
        let invalidates_operations = invalidates_running_operations(&persisted_before, &staged);
        let work_mode_only = work_config_is_only_difference(&persisted_before, &staged);
        let watch_enabled_is_only_difference =
            watch_enabled_is_only_difference(&persisted_before, &staged);
        let keymap_only = only_keymap_difference(&persisted_before, &requested);
        let audio_only = audio_config_is_only_difference(&persisted_before, &requested);
        let requires_factory =
            !work_mode_only && !watch_enabled_is_only_difference && !keymap_only && !audio_only;
        // この permit だけは、構築中も別の音声 intent を保存・反映できる。
        let provider_update =
            permit.command() == crate::command_guard::DesktopCommand::ConfigProviderUpdate;
        let prepared_runtime = if requires_factory && !provider_update {
            Some(
                self.prepare_config_for_current_mode(&staged, persona_notice.clone())
                    .await?,
            )
        } else {
            None
        };
        let _voice_start = if previous.voice_output != requested.voice_output {
            let gate = self.voice_output.start_gate.lock().await;
            self.voice_output.stop().await;
            Some(gate)
        } else {
            None
        };
        let shortcut_result = crate::capture::sync_shortcuts(
            self,
            crate::capture::ShortcutBindings::from_config(&requested),
            shortcut_version,
        )
        .await;
        let (config, mut issues) = match shortcut_result {
            Ok(()) => (requested.clone(), Vec::new()),
            Err(message) => (staged.clone(), vec![keymap_issue(message)]),
        };
        let saved = match save_config_candidate(
            &self.paths,
            &persisted_before,
            &config,
            &mut staged_avatar,
        ) {
            Ok(saved) => saved,
            Err(error) => {
                restore_shortcuts_after_failed_commit(self, &persisted_before, shortcut_version)
                    .await;
                if let coosenpai_core::config::ConfigError::RevisionConflict { actual, .. } = error
                {
                    self.config_update.observe_config_revision(actual);
                }
                return Err(error.into());
            }
        };
        let config = saved.config;
        let rollback_base = saved.previous;
        if provider_update {
            self.work.approvals.set_mode(config.work.approval_mode);
            self.work.set_roots(config.work.allowed_roots.clone());
        }
        self.config_update.observe_config_revision(config.revision);
        self.publish_event(crate::snapshot_presenter::SnapshotEvent::ConfigSaved(
            config.clone(),
        ))
        .await;
        if let Some(error) = saved.avatar_cleanup_error.as_deref() {
            let _ = self.logger.write(
                "WARN",
                &format!("アバター旧ファイルの cleanup に失敗しました: {error}"),
            );
        }
        let watch_scope_changed = persisted_before.watch.fullscreen != config.watch.fullscreen
            || persisted_before.watch.apps != config.watch.apps;
        let language_changed = persisted_before.ui.language != config.ui.language;
        let bubble_stack_changed = persisted_before.bubble.max_stack != config.bubble.max_stack;
        let bubble_appearance_changed = bubble_appearance_changed(&persisted_before, &config);
        let watch_was_running = self.snapshot().await.observer_running;
        if watch_scope_changed {
            self.runtime.invalidate_watch_scope();
        }
        let config_update = if requires_factory {
            async {
                let prepared_runtime = match prepared_runtime {
                    Some(prepared_runtime) => prepared_runtime,
                    None => {
                        self.prepare_config_for_current_mode(&config, persona_notice)
                            .await?
                    }
                };
                if invalidates_operations {
                    self.runtime.quiesce_for_config_update().await?;
                }
                self.apply_prepared_config(config.clone(), invalidates_operations, prepared_runtime)
                    .await
            }
            .await
        } else if audio_only {
            self.runtime
                .update_audio_config(config.audio.clone(), config.revision)
                .await
                .map(|_| ())
                .map_err(ConfigCommitError::Runtime)
        } else {
            self.runtime
                .update_config_without_factory(config.clone())
                .await
                .map(|_| ())
                .map_err(ConfigCommitError::Runtime)
        };
        if let Err(error) = config_update {
            if !provider_update {
                rollback_after_runtime_failure(self, &rollback_base, &config, shortcut_version)
                    .await;
            } else if let Some(avatar) = staged_avatar.as_mut() {
                if let Err(error) = avatar.finalize() {
                    let _ = self.logger.write(
                        "WARN",
                        &format!("アバター旧ファイルの cleanup に失敗しました: {error}"),
                    );
                }
            }
            self.runtime
                .enter_degraded(config_commit_last_error_for_locale(
                    &error,
                    Locale::from_config(&persisted_before.ui.language),
                ))
                .await?;
            self.finish_config_degraded_state(&error).await;
            transaction.commit_config(self.config_update.current_revision())?;
            return Err(error);
        }
        let config = self.runtime.config();
        if Self::audio_session_needs_stop(&persisted_before, &config) {
            self.cancel_audio().await;
        }
        if !provider_update {
            self.work.approvals.set_mode(config.work.approval_mode);
            self.work.set_roots(config.work.allowed_roots.clone());
        }
        if let Some(gate) = provider_start_gate.take() {
            gate.release();
        }
        if let Some(avatar) = staged_avatar.as_mut() {
            if let Err(error) = avatar.finalize() {
                let _ = self.logger.write(
                    "WARN",
                    &format!("アバター旧ファイルの cleanup に失敗しました: {error}"),
                );
            }
        }
        transaction.commit_config(config.revision)?;
        if watch_enabled_is_only_difference || work_mode_only || audio_only {
            self.activate_runtime();
            self.publish_event(crate::snapshot_presenter::SnapshotEvent::ConfigSaved(
                config.clone(),
            ))
            .await;
            if avatar_updated {
                self.refresh_avatar_image().await;
            }
            if avatar_updated || language_changed {
                crate::bubbles::sync_window(self).await.map_err(|error| {
                    ConfigCommitError::Runtime(RuntimeError::Factory(error.to_string()))
                })?;
            }
            return Ok(ConfigUpdateOutcome { config, issues });
        }
        record_companion_model_history(
            &self.paths,
            &persisted_before,
            &config,
            self.logger.as_ref(),
        )
        .await;
        if persisted_before.app.launch_at_login != config.app.launch_at_login {
            if let Err(message) = self.sync_launch_at_login(config.app.launch_at_login) {
                issues.push(coosenpai_core::config::ConfigValidationIssue {
                    path: "app.launchAtLogin".to_owned(),
                    message: text(
                        TextKey::LaunchAtLoginSyncFailed,
                        Locale::from_config(&config.ui.language),
                    )
                    .replace("{error}", &message),
                });
            }
        }
        self.publish_event(crate::snapshot_presenter::SnapshotEvent::ConfigLoaded(
            config.clone(),
        ))
        .await;
        if avatar_updated {
            self.refresh_avatar_image().await;
        }
        if !config.ui.thought_bubble {
            self.clear_pending_thought_bubble().await;
        }
        if !config.ui.thought_bubble {
            crate::bubbles::mutate_checked(
                &self.ui,
                crate::bubbles::BubbleMutation::ClearThoughtBubbles,
            )
            .await
            .map_err(|error| ConfigCommitError::Runtime(RuntimeError::Factory(error)))?;
        }
        if bubble_stack_changed {
            crate::bubbles::mutate_checked(
                &self.ui,
                crate::bubbles::BubbleMutation::SetMaxStack(config.bubble.max_stack),
            )
            .await
            .map_err(|error| ConfigCommitError::Runtime(RuntimeError::Factory(error)))?;
        }
        if avatar_updated || language_changed || bubble_appearance_changed {
            crate::bubbles::sync_window(self).await.map_err(|error| {
                ConfigCommitError::Runtime(RuntimeError::Factory(error.to_string()))
            })?;
        }
        self.refresh_debug().await;
        self.activate_runtime();
        if watch_scope_changed && watch_was_running {
            self.stop_watch_internal(false).await;
            self.start_watch(permit).await.map_err(|error| {
                ConfigCommitError::Runtime(RuntimeError::Factory(error.to_string()))
            })?;
        }
        Ok(ConfigUpdateOutcome { config, issues })
    }

    async fn update_audio_config_with_raw<F>(
        self: &Arc<Self>,
        update: F,
        expected_revision: Option<u64>,
    ) -> Result<ConfigUpdateOutcome, ConfigCommitError>
    where
        F: FnOnce(Config) -> Result<Config, coosenpai_core::config::ConfigError>,
    {
        let _audio = self.config_update.audio.lock().await;
        let recovery = self.runtime.config();
        let (config, stop_session) = coosenpai_core::config::patch_config_before_save_if_revision(
            &self.paths,
            Some(&recovery),
            expected_revision,
            |current| {
                let next = update(current.clone())?;
                if !audio_config_is_only_difference(&current, &next) {
                    return Err(coosenpai_core::config::ConfigError::Validation(vec![
                        coosenpai_core::config::ConfigValidationIssue {
                            path: "audio".to_owned(),
                            message: "音声専用の更新で他の設定は変更できません".to_owned(),
                        },
                    ]));
                }
                Ok(next)
            },
            |previous, next| Ok(Self::audio_session_needs_stop(previous, next)),
        )?;
        self.config_update.observe_config_revision(config.revision);
        if stop_session {
            self.cancel_audio().await;
        }
        let applied = self
            .runtime
            .update_audio_config(config.audio.clone(), config.revision)
            .await;
        self.publish_event(crate::snapshot_presenter::SnapshotEvent::ConfigSaved(
            config.clone(),
        ))
        .await;
        applied?;
        self.activate_runtime();
        Ok(ConfigUpdateOutcome {
            config,
            issues: Vec::new(),
        })
    }

    pub(super) async fn reload_persona_raw(&self) -> Result<(), ConfigCommitError> {
        let transaction = self.config_update.begin().await;
        if !self.runtime_active.load(Ordering::Acquire) {
            return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                text(
                    TextKey::WatchConfigInvalid,
                    Locale::from_config(&self.runtime_config().ui.language),
                )
                .to_owned(),
            )));
        }
        let config = self.runtime.config();
        let tutorial_provider = self.tutorial.lock().await.provider();
        if let Some(provider) = tutorial_provider {
            let agents = self.factory.build_tutorial_agents(&config, provider)?;
            self.runtime.replace_config(config, agents).await?;
        } else {
            let companion = self.factory.build_companion_candidate(&config).await?;
            self.runtime.replace_companion(companion).await?;
        }
        self.publish_event(crate::snapshot_presenter::SnapshotEvent::CompanionStopped)
            .await;
        transaction.commit()?;
        Ok(())
    }

    pub(super) async fn switch_persona_raw(
        self: &Arc<Self>,
        persona: String,
    ) -> Result<Config, ConfigCommitError> {
        let transaction = self.config_update.begin().await;
        if !self.runtime_active.load(Ordering::Acquire) {
            return Err(ConfigCommitError::Runtime(RuntimeError::Factory(
                text(
                    TextKey::WatchConfigInvalid,
                    Locale::from_config(&self.runtime_config().ui.language),
                )
                .to_owned(),
            )));
        }
        let recovery = self.runtime.config();
        let mut previous = String::new();
        let config =
            coosenpai_core::config::patch_config(&self.paths, Some(&recovery), |mut config| {
                previous.clone_from(&config.companion.persona);
                config.companion.persona = persona.clone();
                Ok(config)
            })?;
        if previous == persona {
            transaction.commit_config(config.revision)?;
            self.publish_event(crate::snapshot_presenter::SnapshotEvent::ConfigLoaded(
                config.clone(),
            ))
            .await;
            return Ok(config);
        }
        let locale = Locale::from_config(&config.ui.language);
        let tutorial_provider = self.tutorial.lock().await.provider();
        let replace_result = if let Some(provider) = tutorial_provider {
            let agents = self.factory.build_tutorial_agents(&config, provider)?;
            self.runtime.replace_config(config.clone(), agents).await
        } else {
            let notice = text(TextKey::PersonaChangedNotice, locale)
                .replace("{previous}", &previous)
                .replace("{next}", &persona);
            let companion = self
                .factory
                .build_companion_candidate_with_notice(&config, Some(notice))
                .await?;
            self.runtime
                .replace_companion_with_config(config.clone(), companion)
                .await
        };
        if let Err(error) = replace_result {
            let error = ConfigCommitError::Runtime(error);
            self.runtime
                .enter_degraded(config_commit_last_error_for_locale(
                    &error,
                    Locale::from_config(&config.ui.language),
                ))
                .await?;
            self.finish_config_degraded_state(&error).await;
            return Err(error);
        }
        transaction.commit_config(config.revision)?;
        self.publish_event(
            crate::snapshot_presenter::SnapshotEvent::CompanionReconfigured(config.clone()),
        )
        .await;
        Ok(config)
    }

    pub(super) async fn switch_persona_during_setup_raw(
        self: &Arc<Self>,
        persona: String,
    ) -> Result<Config, ConfigCommitError> {
        let transaction = self.config_update.begin().await;
        let recovery = self.runtime.config();
        let mut previous = String::new();
        let config =
            coosenpai_core::config::patch_config(&self.paths, Some(&recovery), |mut config| {
                previous.clone_from(&config.companion.persona);
                config.companion.persona = persona.clone();
                Ok(config)
            })?;
        if previous == persona {
            transaction.commit_config(config.revision)?;
            self.publish_event(crate::snapshot_presenter::SnapshotEvent::ConfigLoaded(
                config.clone(),
            ))
            .await;
            return Ok(config);
        }
        self.runtime.update_config(config.clone()).await?;
        transaction.commit_config(config.revision)?;
        self.publish_event(
            crate::snapshot_presenter::SnapshotEvent::CompanionReconfigured(config.clone()),
        )
        .await;
        Ok(config)
    }

    async fn finish_config_degraded_state(&self, error: &ConfigCommitError) {
        let locale = Locale::from_config(&self.runtime_config().ui.language);
        let last_error = config_commit_last_error_for_locale(error, locale);
        self.deactivate_runtime().await;
        self.stop_watch_internal(true).await;
        self.publish_event(crate::snapshot_presenter::SnapshotEvent::CompanionFailed(
            last_error,
        ))
        .await;
    }

}

async fn record_companion_model_history(
    paths: &ConfigPaths,
    previous: &Config,
    next: &Config,
    logger: &dyn RuntimeLogger,
) {
    if previous.companion.provider == next.companion.provider
        && previous.companion.model == next.companion.model
    {
        return;
    }
    if let Err(error) = crate::model_catalog::record_companion_selection(
        paths,
        &next.companion.provider,
        &next.companion.model,
    )
    .await
    {
        let _ = logger.write(
            "DEBUG",
            &format!("companion のモデル使用履歴を保存できませんでした: reason={error}"),
        );
    }
}

fn keymap_issue(message: String) -> coosenpai_core::config::ConfigValidationIssue {
    coosenpai_core::config::ConfigValidationIssue {
        path: "keymap".to_owned(),
        message,
    }
}

struct PreparedConfigUpdate {
    previous: Config,
    staged: Config,
    requested: Config,
    provider_start_gate: Option<coosenpai_core::runtime::ProviderStartGate>,
}

struct SavedConfig {
    config: Config,
    previous: Config,
    avatar_cleanup_error: Option<String>,
}

fn prepare_config_update<F>(
    paths: &ConfigPaths,
    runtime: &RuntimeHandle,
    recovery: &Config,
    update: F,
    expected_revision: Option<u64>,
) -> Result<PreparedConfigUpdate, coosenpai_core::config::ConfigError>
where
    F: FnOnce(Config) -> Result<Config, coosenpai_core::config::ConfigError>,
{
    let (previous, requested) = coosenpai_core::config::prepare_config_update(
        paths,
        Some(recovery),
        expected_revision,
        update,
    )?;
    let staged = stage_without_keymap(&requested, &previous);
    let provider_start_gate = invalidates_running_operations(&previous, &staged)
        .then(|| runtime.block_provider_starts_for_config_update());
    Ok(PreparedConfigUpdate {
        previous,
        staged,
        requested,
        provider_start_gate,
    })
}

fn save_config_candidate(
    paths: &ConfigPaths,
    previous: &Config,
    candidate: &Config,
    staged_avatar: &mut Option<crate::avatar::StagedAvatar>,
) -> Result<SavedConfig, coosenpai_core::config::ConfigError> {
    let mut cleanup_errors = Vec::new();
    if let Err(error) = crate::avatar::cleanup_stale_files(paths) {
        cleanup_errors.push(format!("保存前: {error}"));
    }
    if let Some(avatar) = staged_avatar.as_mut() {
        avatar
            .install()
            .map_err(coosenpai_core::config::ConfigError::Io)?;
    }
    let (config, previous) = save_config_preserving_audio(paths, previous, candidate)?;
    Ok(SavedConfig {
        config,
        previous,
        avatar_cleanup_error: (!cleanup_errors.is_empty()).then(|| cleanup_errors.join("; ")),
    })
}

fn save_config_preserving_audio(
    paths: &ConfigPaths,
    previous: &Config,
    candidate: &Config,
) -> Result<(Config, Config), coosenpai_core::config::ConfigError> {
    coosenpai_core::config::patch_config_before_save_if_revision(
        paths,
        Some(previous),
        None,
        |current| {
            if current.revision < previous.revision
                || !audio_config_is_only_difference(previous, &current)
            {
                return Err(coosenpai_core::config::ConfigError::RevisionConflict {
                    expected: previous.revision,
                    actual: current.revision,
                });
            }
            let mut config = candidate.clone();
            if current.revision > previous.revision {
                config.audio = current.audio;
            }
            Ok(config)
        },
        |current, _| Ok(current.clone()),
    )
}

#[cfg(test)]
struct PersistedConfigUpdate {
    staged: Config,
    provider_start_gate: Option<coosenpai_core::runtime::ProviderStartGate>,
}

#[cfg(test)]
fn persist_config_update<F>(
    paths: &ConfigPaths,
    runtime: &RuntimeHandle,
    recovery: &Config,
    update: F,
    mut staged_avatar: Option<crate::avatar::StagedAvatar>,
    expected_revision: Option<u64>,
) -> Result<PersistedConfigUpdate, coosenpai_core::config::ConfigError>
where
    F: FnOnce(Config) -> Result<Config, coosenpai_core::config::ConfigError>,
{
    let prepared = prepare_config_update(paths, runtime, recovery, update, expected_revision)?;
    let saved = save_config_candidate(
        paths,
        &prepared.previous,
        &prepared.staged,
        &mut staged_avatar,
    )?;
    if let Some(avatar) = staged_avatar.as_mut() {
        avatar
            .finalize()
            .map_err(coosenpai_core::config::ConfigError::Io)?;
    }
    Ok(PersistedConfigUpdate {
        staged: saved.config,
        provider_start_gate: prepared.provider_start_gate,
    })
}

async fn restore_shortcuts_after_failed_commit(
    state: &DesktopState,
    previous: &Config,
    shortcut_version: u64,
) {
    if let Err(error) = crate::capture::sync_shortcuts(
        state,
        crate::capture::ShortcutBindings::from_config(previous),
        shortcut_version,
    )
    .await
    {
        let _ = state.logger.write(
            "WARN",
            &format!("設定保存失敗後のショートカット復元に失敗しました: {error}"),
        );
    }
}

async fn rollback_after_runtime_failure(
    state: &DesktopState,
    previous: &Config,
    committed: &Config,
    shortcut_version: u64,
) {
    restore_shortcuts_after_failed_commit(state, previous, shortcut_version).await;
    match save_config_preserving_audio(&state.paths, committed, previous) {
        Ok((rollback, _)) => {
            state
                .config_update
                .observe_config_revision(rollback.revision);
            if let Err(error) = state
                .runtime
                .update_config_without_factory(rollback.clone())
                .await
            {
                let _ = state.logger.write(
                    "WARN",
                    &format!("runtime の設定復元に失敗しました: {error}"),
                );
            }
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::ConfigSaved(
                    rollback,
                ))
                .await;
        }
        Err(error) => {
            let _ = state.logger.write(
                "WARN",
                &format!("設定ファイルの rollback に失敗しました: {error}"),
            );
        }
    }
}

fn stage_without_keymap(requested: &Config, previous: &Config) -> Config {
    let mut staged = requested.clone();
    staged.keymap = previous.keymap.clone();
    staged
}

fn watch_enabled_is_only_difference(current: &Config, next: &Config) -> bool {
    let mut current_without_intent = current.clone();
    let mut next_without_intent = next.clone();
    current_without_intent.revision = next_without_intent.revision;
    current_without_intent.watch.enabled = false;
    next_without_intent.watch.enabled = false;
    current_without_intent == next_without_intent
}

fn only_keymap_difference(current: &Config, next: &Config) -> bool {
    let mut current_without_keymap = current.clone();
    current_without_keymap.keymap = next.keymap.clone();
    current_without_keymap.revision = next.revision;
    current_without_keymap == *next
}

fn bubble_appearance_changed(previous: &Config, next: &Config) -> bool {
    previous.ui.theme != next.ui.theme
        || previous.ui.font != next.ui.font
        || previous.ui.avatar_color != next.ui.avatar_color
        || previous.ui.avatar_path != next.ui.avatar_path
        || previous.bubble.position != next.bubble.position
        || previous.bubble.display != next.bubble.display
}

fn persona_change_notice(previous: &Config, next: &Config, locale: Locale) -> Option<String> {
    (previous.companion.persona != next.companion.persona).then(|| {
        text(TextKey::PersonaChangedNotice, locale)
            .replace("{previous}", &previous.companion.persona)
            .replace("{next}", &next.companion.persona)
    })
}

#[cfg(test)]
pub(crate) async fn commit_config_or_degrade(
    factory: &DesktopRuntimeFactory,
    runtime: &RuntimeHandle,
    config: &Config,
) -> Result<(), ConfigCommitError> {
    let commit = async {
        let agents = factory.build_candidate(config).await?;
        runtime.replace_config(config.clone(), agents).await?;
        Ok::<(), ConfigCommitError>(())
    };
    if let Err(error) = commit.await {
        runtime
            .enter_degraded(config_commit_last_error(&error))
            .await?;
        return Err(error);
    }
    Ok(())
}

pub(crate) fn config_commit_last_error_for_locale(
    error: &ConfigCommitError,
    locale: Locale,
) -> RuntimeLastError {
    RuntimeLastError {
        kind: RuntimeErrorKind::Config,
        occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: Some(error.format_for_locale(locale)),
        issues: error.issues_for_locale(locale),
        attachment_ocr: None,
        user_response: None,
    }
}

