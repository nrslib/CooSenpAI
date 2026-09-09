use super::*;
use crate::commands_config::{invalidates_running_operations, work_config_is_only_difference};
use crate::config_update::ConfigUpdateOutcome;
use coosenpai_core::locale::{localize_shortcut_message, text, Locale, TextKey};

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
        let persisted = match persist_config_update(
            &self.paths,
            &self.runtime,
            &previous,
            guarded_update,
            staged_avatar,
            expected_revision,
        ) {
            Ok(persisted) => persisted,
            Err(error @ coosenpai_core::config::ConfigError::RevisionConflict { actual, .. }) => {
                self.config_update.observe_config_revision(actual);
                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        };
        let requested = persisted.requested.ok_or_else(|| {
            ConfigCommitError::Runtime(RuntimeError::Factory(
                text(
                    TextKey::ConfigCandidateBuildFailed,
                    Locale::from_config(&previous.ui.language),
                )
                .to_owned(),
            ))
        })?;
        self.work.approvals.set_mode(requested.work.approval_mode);
        self.work.set_roots(requested.work.allowed_roots.clone());
        let persisted_before = persisted.previous;
        let staged = persisted.staged;
        let provider_start_gate = persisted.provider_start_gate;
        if let Some(error) = persisted.avatar_cleanup_error.as_deref() {
            let _ = self.logger.write(
                "WARN",
                &format!("アバター旧ファイルの cleanup に失敗しました: {error}"),
            );
        }
        let watch_scope_changed = persisted_before.watch.fullscreen != requested.watch.fullscreen
            || persisted_before.watch.apps != requested.watch.apps;
        let language_changed = persisted_before.ui.language != staged.ui.language;
        let bubble_stack_changed = persisted_before.bubble.max_stack != requested.bubble.max_stack;
        let bubble_appearance_changed = bubble_appearance_changed(&persisted_before, &requested);
        let persona_notice = persona_change_notice(
            &persisted_before,
            &staged,
            Locale::from_config(&staged.ui.language),
        );
        let invalidates_operations = invalidates_running_operations(&persisted_before, &staged);
        let watch_enabled_is_only_difference = watch_enabled_is_only_difference(&previous, &staged);
        let _voice_start = if previous.voice_output != requested.voice_output {
            let gate = self.voice_output.start_gate.lock().await;
            self.voice_output.stop().await;
            Some(gate)
        } else {
            None
        };
        if Self::audio_session_needs_stop(&persisted_before, &requested) {
            self.cancel_audio().await;
        }
        if watch_scope_changed {
            self.runtime.invalidate_watch_scope();
        }
        let watch_was_running = self.snapshot().await.observer_running;
        let work_mode_only = work_config_is_only_difference(&previous, &staged);
        let config_update = if work_mode_only {
            self.runtime
                .update_work_config(staged.work.clone())
                .await
                .map(|_| ())
                .map_err(ConfigCommitError::Runtime)
        } else if watch_enabled_is_only_difference {
            self.runtime
                .update_watch_enabled(staged.watch.enabled)
                .await
                .map(|_| ())
                .map_err(ConfigCommitError::Runtime)
        } else {
            self.replace_config_for_current_mode_with_notice(
                staged.clone(),
                invalidates_operations,
                persona_notice,
            )
            .await
        };
        if let Err(error) = config_update {
            self.runtime
                .enter_degraded(config_commit_last_error_for_locale(
                    &error,
                    Locale::from_config(&staged.ui.language),
                ))
                .await?;
            self.finish_config_degraded_state(&error).await;
            return Err(error);
        }
        if watch_enabled_is_only_difference || work_mode_only {
            transaction.commit_config(staged.revision)?;
            self.activate_runtime();
            self.publish_event(crate::snapshot_presenter::SnapshotEvent::ConfigLoaded(
                staged.clone(),
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
            return Ok(ConfigUpdateOutcome {
                config: staged,
                issues: Vec::new(),
            });
        }
        if let Some(gate) = provider_start_gate {
            gate.release();
        }
        let shortcut_result = crate::capture::sync_shortcuts(
            self,
            crate::capture::ShortcutBindings::from_config(&requested),
            shortcut_version,
        )
        .await;
        let (config, mut issues) = match shortcut_result {
            Ok(()) if requested.keymap != staged.keymap => {
                let keymap_base = persisted_before.clone();
                let keymap_candidate = requested.clone();
                match persist_keymap_patch(
                    &self.paths,
                    staged.revision,
                    &keymap_base,
                    &keymap_candidate,
                ) {
                    Ok(config) => (config, Vec::new()),
                    Err(error) => {
                        let restore = crate::capture::sync_shortcuts(
                            self,
                            crate::capture::ShortcutBindings::from_config(&staged),
                            shortcut_version,
                        )
                        .await
                        .err();
                        if let coosenpai_core::config::ConfigError::RevisionConflict {
                            actual,
                            ..
                        } = &error
                        {
                            if let Some(restore) = restore.as_deref() {
                                let _ = self.logger.write(
                                    "WARN",
                                    &format!(
                                        "設定競合後のショートカット復元に失敗しました: {restore}"
                                    ),
                                );
                            }
                            self.config_update.observe_config_revision(*actual);
                            return Err(error.into());
                        }
                        let locale = Locale::from_config(&staged.ui.language);
                        let message = restore.map_or_else(
                            || error.format_for_locale(locale),
                            |restore| {
                                format!(
                                    "{}; {}",
                                    error.format_for_locale(locale),
                                    localize_shortcut_message(&restore, locale)
                                )
                            },
                        );
                        (staged.clone(), vec![keymap_issue(message)])
                    }
                }
            }
            Ok(()) => (staged.clone(), Vec::new()),
            Err(message) => (staged.clone(), vec![keymap_issue(message)]),
        };
        if config != staged {
            if let Err(error) = self
                .replace_config_for_current_mode(config.clone(), false)
                .await
            {
                self.runtime
                    .enter_degraded(config_commit_last_error_for_locale(
                        &error,
                        Locale::from_config(&config.ui.language),
                    ))
                    .await?;
                self.finish_config_degraded_state(&error).await;
                return Err(error);
            }
        }
        transaction.commit_config(config.revision)?;
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

struct PersistedConfigUpdate {
    previous: Config,
    staged: Config,
    requested: Option<Config>,
    provider_start_gate: Option<coosenpai_core::runtime::ProviderStartGate>,
    avatar_cleanup_error: Option<String>,
}

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
    let mut persisted_before = recovery.clone();
    let mut requested = None;
    let mut avatar_cleanup_errors = Vec::new();
    let (staged, provider_start_gate) =
        coosenpai_core::config::patch_config_before_save_if_revision(
            paths,
            Some(recovery),
            expected_revision,
            |current| {
                let keymap_base = current.clone();
                let audio_was_enabled = current.audio.enabled;
                let mut config = update(current)?;
                coosenpai_core::config::normalize_audio_sources_on_enable(
                    audio_was_enabled,
                    &mut config,
                );
                let staged = stage_without_keymap(&config, &keymap_base);
                requested = Some(config);
                Ok(staged)
            },
            |current, staged| {
                if let Err(error) = crate::avatar::cleanup_stale_files(paths) {
                    avatar_cleanup_errors.push(format!("保存前: {error}"));
                }
                if let Some(avatar) = staged_avatar.as_mut() {
                    avatar
                        .install()
                        .map_err(coosenpai_core::config::ConfigError::Io)?;
                }
                persisted_before = current.clone();
                Ok(invalidates_running_operations(current, staged)
                    .then(|| runtime.block_provider_starts_for_config_update()))
            },
        )?;
    if let Some(error) = staged_avatar
        .as_mut()
        .and_then(|avatar| avatar.finalize().err())
    {
        avatar_cleanup_errors.push(format!("確定後: {error}"));
    }
    Ok(PersistedConfigUpdate {
        previous: persisted_before,
        staged,
        requested,
        provider_start_gate,
        avatar_cleanup_error: (!avatar_cleanup_errors.is_empty())
            .then(|| avatar_cleanup_errors.join("; ")),
    })
}

fn stage_without_keymap(requested: &Config, previous: &Config) -> Config {
    let mut staged = requested.clone();
    staged.keymap = previous.keymap.clone();
    staged
}

fn persist_keymap_patch(
    paths: &ConfigPaths,
    expected_revision: u64,
    keymap_base: &Config,
    keymap_candidate: &Config,
) -> Result<Config, coosenpai_core::config::ConfigError> {
    coosenpai_core::config::patch_config_before_save_if_revision(
        paths,
        None,
        Some(expected_revision),
        |mut current| {
            apply_keymap_changes(&mut current, keymap_base, keymap_candidate);
            Ok(current)
        },
        |_, _| Ok(()),
    )
    .map(|(config, ())| config)
}

fn watch_enabled_is_only_difference(current: &Config, next: &Config) -> bool {
    let mut current_without_intent = current.clone();
    let mut next_without_intent = next.clone();
    current_without_intent.revision = next_without_intent.revision;
    current_without_intent.watch.enabled = false;
    next_without_intent.watch.enabled = false;
    current_without_intent == next_without_intent
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

fn apply_keymap_changes(current: &mut Config, previous: &Config, requested: &Config) {
    macro_rules! apply {
        ($field:ident) => {
            if previous.keymap.$field != requested.keymap.$field {
                current.keymap.$field = requested.keymap.$field.clone();
            }
        };
    }
    apply!(capture_region);
    apply!(microphone);
    apply!(toggle_panel);
    apply!(toggle_avatar);
    apply!(toggle_watch);
    apply!(send_text);
    apply!(copy_last_reply);
    apply!(send_key);
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
    }
}

