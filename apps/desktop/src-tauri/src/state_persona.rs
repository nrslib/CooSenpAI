use super::*;
use crate::commands_config::invalidates_running_operations;
use crate::config_update::ConfigUpdateOutcome;

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
        let persisted =
            persist_config_update(&self.paths, &self.runtime, &previous, update, staged_avatar)?;
        let requested = persisted.requested.ok_or_else(|| {
            ConfigCommitError::Runtime(RuntimeError::Factory(
                "設定候補を構築できませんでした".to_owned(),
            ))
        })?;
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
        let bubble_stack_changed = persisted_before.bubble.max_stack != requested.bubble.max_stack;
        let bubble_appearance_changed = bubble_appearance_changed(&persisted_before, &requested);
        let persona_notice = persona_change_notice(&persisted_before, &staged);
        let invalidates_operations = invalidates_running_operations(&persisted_before, &staged);
        let watch_enabled_is_only_difference = watch_enabled_is_only_difference(&previous, &staged);
        if Self::audio_session_needs_stop(&persisted_before, &requested) {
            self.cancel_audio_and_wait().await;
        }
        if watch_scope_changed {
            self.runtime.invalidate_watch_scope();
        }
        let watch_was_running = self.snapshot().await.observer_running;
        let config_update = if watch_enabled_is_only_difference {
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
                .enter_degraded(config_commit_last_error(&error))
                .await?;
            self.finish_config_degraded_state(&error).await;
            return Err(error);
        }
        if watch_enabled_is_only_difference {
            transaction.commit()?;
            self.activate_runtime();
            self.publish(|snapshot| snapshot.apply_config(staged.clone()))
                .await;
            if avatar_updated {
                self.refresh_avatar_image().await;
                let _ = crate::bubbles::sync_window(self).await;
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
                match coosenpai_core::config::patch_config(&self.paths, None, move |mut current| {
                    apply_keymap_changes(&mut current, &keymap_base, &keymap_candidate);
                    Ok(current)
                }) {
                    Ok(config) => (config, Vec::new()),
                    Err(error) => {
                        let restore = crate::capture::sync_shortcuts(
                            self,
                            crate::capture::ShortcutBindings::from_config(&staged),
                            shortcut_version,
                        )
                        .await
                        .err();
                        let message = restore.map_or_else(
                            || error.format_for_user(),
                            |restore| format!("{}; {restore}", error.format_for_user()),
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
                    .enter_degraded(config_commit_last_error(&error))
                    .await?;
                self.finish_config_degraded_state(&error).await;
                return Err(error);
            }
        }
        transaction.commit()?;
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
                    message: format!("ログイン時起動を変更できませんでした: {message}"),
                });
            }
        }
        self.publish(|snapshot| snapshot.apply_config(config.clone()))
            .await;
        if avatar_updated {
            self.refresh_avatar_image().await;
        }
        crate::windows::sync_shortcut_menu(&self.app, &config);
        if !config.ui.thought_bubble {
            self.clear_pending_thought_bubble().await;
        }
        let thought_bubbles_cleared =
            !config.ui.thought_bubble && self.bubbles.lock().await.clear_thought_bubbles();
        let bubble_stack_changed_on_screen = bubble_stack_changed
            && self
                .bubbles
                .lock()
                .await
                .set_max_stack(config.bubble.max_stack);
        if avatar_updated
            || thought_bubbles_cleared
            || bubble_appearance_changed
            || bubble_stack_changed_on_screen
        {
            let _ = crate::bubbles::sync_window(self).await;
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
                "設定を修正して保存してください".to_owned(),
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
        self.publish(|snapshot| {
            snapshot.companion.phase = crate::snapshot::CompanionViewPhase::Idle;
        })
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
                "設定を修正して保存してください".to_owned(),
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
            return Ok(config);
        }
        let tutorial_provider = self.tutorial.lock().await.provider();
        let replace_result = if let Some(provider) = tutorial_provider {
            let agents = self.factory.build_tutorial_agents(&config, provider)?;
            self.runtime.replace_config(config.clone(), agents).await
        } else {
            let notice = format!("ペルソナが {previous} から {persona} に切り替わった");
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
                .enter_degraded(config_commit_last_error(&error))
                .await?;
            self.finish_config_degraded_state(&error).await;
            return Err(error);
        }
        transaction.commit()?;
        self.publish(|snapshot| {
            snapshot.apply_config(config.clone());
            snapshot.companion.phase = crate::snapshot::CompanionViewPhase::Idle;
        })
        .await;
        crate::windows::sync_persona(&self.app, &persona);
        Ok(config)
    }

    async fn finish_config_degraded_state(&self, error: &ConfigCommitError) {
        let last_error = config_commit_last_error(error);
        self.deactivate_runtime().await;
        self.stop_watch_internal(true).await;
        self.publish(|snapshot| {
            snapshot.last_error = Some(last_error);
            snapshot.companion.ready = false;
            snapshot.companion.phase = crate::snapshot::CompanionViewPhase::Error;
        })
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
) -> Result<PersistedConfigUpdate, coosenpai_core::config::ConfigError>
where
    F: FnOnce(Config) -> Result<Config, coosenpai_core::config::ConfigError>,
{
    let mut persisted_before = recovery.clone();
    let mut requested = None;
    let mut avatar_cleanup_errors = Vec::new();
    let (staged, provider_start_gate) = coosenpai_core::config::patch_config_before_save(
        paths,
        Some(recovery),
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
            if let Err(error) = crate::avatar::cleanup_stale_backups(paths) {
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

fn watch_enabled_is_only_difference(current: &Config, next: &Config) -> bool {
    let mut current_without_intent = current.clone();
    let mut next_without_intent = next.clone();
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

fn persona_change_notice(previous: &Config, next: &Config) -> Option<String> {
    (previous.companion.persona != next.companion.persona).then(|| {
        format!(
            "ペルソナが {} から {} に切り替わった",
            previous.companion.persona, next.companion.persona
        )
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

pub(crate) fn config_commit_last_error(error: &ConfigCommitError) -> RuntimeLastError {
    RuntimeLastError {
        kind: RuntimeErrorKind::Config,
        occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: Some(error.format_for_user()),
        issues: error.issues(),
        attachment_ocr: None,
    }
}

