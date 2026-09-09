use coosenpai_core::config::{Config, ConfigError, ConfigPaths, ConfigValidationIssue};
use coosenpai_core::conversation_archive::{
    archive_conversation, archive_conversation_after_recovery, current_conversation_generation,
};
use coosenpai_core::locale::{text, Locale, TextKey};
use coosenpai_core::logging::FileLogger;
use coosenpai_core::onboarding::{OnboardingState, OnboardingStore};
use coosenpai_core::persistence::PersistenceError;
use coosenpai_core::runtime::{RuntimeActor, RuntimeErrorKind, RuntimeHandle, RuntimeLastError};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;
use tokio_util::sync::CancellationToken;

pub(super) fn resource_directory(app: &tauri::AppHandle) -> anyhow::Result<PathBuf> {
    match app.path().resource_dir() {
        Ok(path) => Ok(path),
        Err(_) => std::env::current_exe()?
            .parent()
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow::anyhow!("実行ファイルのディレクトリを取得できません")),
    }
}

pub(super) fn startup_config(
    loaded: Result<Config, ConfigError>,
) -> (Config, Option<RuntimeLastError>) {
    let (config, issues, message) = match loaded {
        Ok(config) => (config, Vec::new(), None),
        Err(ConfigError::Validation(issues)) => {
            let message = format_issues(&issues);
            (Config::default(), issues, Some(message))
        }
        Err(ConfigError::UnsupportedVersion(version)) => {
            let issue = ConfigValidationIssue {
                path: "configVersion".to_owned(),
                message: text(TextKey::StartupConfigUnsupportedVersion, Locale::Ja)
                    .replace("{version}", &version.to_string()),
            };
            let message = format_issues(std::slice::from_ref(&issue));
            (Config::default(), vec![issue], Some(message))
        }
        Err(error) => (Config::default(), Vec::new(), Some(error.format_for_user())),
    };
    let error = message.map(|message| runtime_error(message, issues));
    (config, error)
}

pub(super) fn onboarding_runtime_error_for_locale(
    state: &OnboardingState,
    locale: Locale,
) -> Option<RuntimeLastError> {
    if state.needs_setup() {
        Some(runtime_error(
            text(TextKey::SetupWaiting, locale).to_owned(),
            Vec::new(),
        ))
    } else {
        None
    }
}

pub(super) async fn startup_runtime(
    config: &Config,
    runtime_error: Option<RuntimeLastError>,
    onboarding: &crate::snapshot::OnboardingView,
    tutorial: &mut crate::tutorial::TutorialController,
    factory: Arc<crate::factory::DesktopRuntimeFactory>,
    logger: Arc<FileLogger>,
    cancellation: CancellationToken,
) -> Result<RuntimeHandle, crate::factory::DesktopFactoryError> {
    let tutorial_agents = if runtime_error.is_none()
        && onboarding.tutorial_active
        && !onboarding.setup_required
    {
        let (agents, provider) = factory
            .build_tutorial_candidate(config, super::tutorial_state::tutorial_placeholders(config))
            .await?;
        if !onboarding.finish_pending {
            tutorial.attach_provider(provider);
        }
        Some(agents)
    } else {
        None
    };
    Ok(match (runtime_error, tutorial_agents) {
        (Some(error), _) => RuntimeActor::spawn_degraded_with_logger_and_cancellation(
            config.clone(),
            logger,
            cancellation,
            error,
        ),
        (None, Some(agents)) => RuntimeActor::spawn_agents_with_factory_logger_and_cancellation(
            config.clone(),
            agents,
            factory,
            logger,
            cancellation,
        ),
        (None, None) => RuntimeActor::spawn_with_factory_logger_and_cancellation(
            config.clone(),
            None,
            None,
            factory,
            logger,
            cancellation,
        ),
    })
}

#[cfg(test)]
pub(super) fn startup_tutorial(
    store: OnboardingStore,
) -> (
    crate::tutorial::TutorialController,
    Option<RuntimeLastError>,
) {
    startup_tutorial_for_locale(store, Locale::Ja)
}

pub(super) fn startup_tutorial_for_locale(
    store: OnboardingStore,
    locale: Locale,
) -> (
    crate::tutorial::TutorialController,
    Option<RuntimeLastError>,
) {
    match crate::tutorial::TutorialController::load(store.clone()) {
        Ok(tutorial) => (tutorial, None),
        Err(error) => (
            crate::tutorial::TutorialController::from_state(store, OnboardingState::default()),
            Some(persistence_runtime_error(
                text(TextKey::StartupTutorialStateReadFailed, locale)
                    .replace("{error}", &error.to_string()),
            )),
        ),
    }
}

pub(super) fn setup_runtime_error_for_locale(locale: Locale) -> RuntimeLastError {
    runtime_error(text(TextKey::SetupWaiting, locale).to_owned(), Vec::new())
}

pub(super) fn factory_runtime_error_for_locale(
    issue: ConfigValidationIssue,
    locale: Locale,
) -> RuntimeLastError {
    let issue = issue.localized(locale);
    runtime_error(format!("{}: {}", issue.path, issue.message), vec![issue])
}

pub(super) fn conversation_generation_for_locale(
    paths: &ConfigPaths,
    locale: Locale,
) -> (u64, Option<RuntimeLastError>) {
    match current_conversation_generation(paths) {
        Ok(generation) => (generation, None),
        Err(error) => (
            0,
            Some(RuntimeLastError {
                kind: RuntimeErrorKind::Persistence,
                occurred_at: chrono::Utc::now()
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                message: Some(
                    text(TextKey::StartupConversationGenerationReadFailed, locale)
                        .replace("{error}", &error.to_string()),
                ),
                issues: Vec::new(),
                attachment_ocr: None,
            }),
        ),
    }
}

pub(super) fn should_initialize_conversation_on_startup(
    config_is_valid: bool,
    onboarding_is_valid: bool,
    generation_is_valid: bool,
    setup_required: bool,
    tutorial_active: bool,
) -> bool {
    config_is_valid
        && onboarding_is_valid
        && generation_is_valid
        && !setup_required
        && !tutorial_active
}

pub(super) fn initialize_conversation_on_startup(
    paths: &ConfigPaths,
    retention_days: u64,
    should_initialize: bool,
) -> Result<u64, PersistenceError> {
    if should_initialize {
        archive_conversation(paths, retention_days, chrono::Utc::now())?;
    }
    current_conversation_generation(paths)
}

pub(super) fn initialize_conversation_before_runtime(
    paths: &ConfigPaths,
    config: &Config,
    current_generation: u64,
    generation_error: bool,
    should_initialize: bool,
) -> (u64, Option<RuntimeLastError>) {
    if generation_error {
        return (current_generation, None);
    }
    let result = if should_initialize {
        initialize_conversation_on_startup_after_recovery(paths, config)
    } else {
        initialize_conversation_on_startup(paths, config.retention.conversation_days, false)
    };
    match result {
        Ok(generation) => (generation, None),
        Err(error) => (
            current_generation,
            Some(conversation_initialization_error_for_locale(
                error,
                Locale::from_config(&config.ui.language),
            )),
        ),
    }
}

pub(super) fn initialize_conversation_on_startup_after_recovery(
    paths: &ConfigPaths,
    config: &Config,
) -> Result<u64, PersistenceError> {
    let locale = Locale::from_config(&config.ui.language);
    let mut companion = coosenpai_core::companion::CompanionAgent::for_storage_recovery(
        config.companion.clone(),
        paths,
        config.retention.conversation_days,
    )
    .map_err(|error| {
        PersistenceError::Invalid(
            text(TextKey::StartupConversationRecoveryPrepareFailed, locale)
                .replace("{error}", &error.to_string()),
        )
    })?;
    archive_conversation_after_recovery(
        paths,
        config.retention.conversation_days,
        chrono::Utc::now(),
        || {
            companion
                .recover_persisted_state_before_conversation_archive()
                .map_err(|error| {
                    PersistenceError::Invalid(
                        text(TextKey::StartupConversationRecoveryFailed, locale)
                            .replace("{error}", &error.to_string()),
                    )
                })
        },
    )?;
    current_conversation_generation(paths)
}

pub(super) fn conversation_initialization_error_for_locale(
    error: PersistenceError,
    locale: Locale,
) -> RuntimeLastError {
    persistence_runtime_error(
        text(TextKey::StartupConversationInitializationFailed, locale)
            .replace("{error}", &error.to_string()),
    )
}

pub(super) fn persistence_runtime_error(message: String) -> RuntimeLastError {
    RuntimeLastError {
        kind: RuntimeErrorKind::Persistence,
        occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: Some(message),
        issues: Vec::new(),
        attachment_ocr: None,
    }
}

fn format_issues(issues: &[ConfigValidationIssue]) -> String {
    issues
        .iter()
        .map(|issue| format!("{}: {}", issue.path, issue.message))
        .collect::<Vec<_>>()
        .join("\n")
}

fn runtime_error(message: String, issues: Vec<ConfigValidationIssue>) -> RuntimeLastError {
    runtime_error_with_kind(RuntimeErrorKind::Config, message, issues)
}

fn runtime_error_with_kind(
    kind: RuntimeErrorKind,
    message: String,
    issues: Vec<ConfigValidationIssue>,
) -> RuntimeLastError {
    RuntimeLastError {
        kind,
        occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        message: Some(message),
        issues,
        attachment_ocr: None,
    }
}
