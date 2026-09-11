use super::startup;
use crate::factory::DesktopRuntimeFactory;
use crate::snapshot::OnboardingView;
use crate::tutorial::TutorialController;
use coosenpai_core::config::{load_config, Config, ConfigPaths};
use coosenpai_core::locale::Locale;
use coosenpai_core::onboarding::OnboardingStore;
use coosenpai_core::ports::RuntimeLogger;
use coosenpai_core::runtime::RuntimeLastError;

pub(super) struct StartupContext {
    pub config: Config,
    pub tutorial: TutorialController,
    pub onboarding: OnboardingView,
    pub conversation_generation: u64,
    pub ready: bool,
    pub error: Option<RuntimeLastError>,
}

impl StartupContext {
    pub fn load(paths: &ConfigPaths, factory: &DesktopRuntimeFactory) -> Self {
        let (config, config_error) = startup::startup_config(load_config(paths));
        let locale = Locale::from_config(&config.ui.language);
        let (mut tutorial, onboarding_persistence_error) = startup::startup_tutorial_for_locale(
            OnboardingStore::new(paths.onboarding.clone()),
            locale,
        );
        let onboarding = OnboardingView::from_state(tutorial.state());
        let setup_provider_error = if onboarding.setup_required {
            match factory.tutorial_provider_for_locale(
                super::tutorial_state::tutorial_placeholders(&config),
                locale,
            ) {
                Ok(provider) => {
                    tutorial.attach_setup_provider(provider);
                    None
                }
                Err(error) => Some(startup::factory_runtime_error_for_locale(
                    error.issue,
                    locale,
                )),
            }
        } else {
            None
        };
        let (current_generation, generation_error) =
            startup::conversation_generation_for_locale(paths, locale);
        let should_initialize = startup::should_initialize_conversation_on_startup(
            config_error.is_none(),
            onboarding_persistence_error.is_none(),
            generation_error.is_none(),
            onboarding.setup_required,
            onboarding.tutorial_active,
        );
        let (conversation_generation, conversation_error) =
            startup::initialize_conversation_before_runtime(
                paths,
                &config,
                current_generation,
                generation_error.is_some(),
                should_initialize,
            );
        let onboarding_error =
            startup::onboarding_runtime_error_for_locale(tutorial.state(), locale);
        let ready = config_error.is_none()
            && generation_error.is_none()
            && onboarding_persistence_error.is_none()
            && conversation_error.is_none();
        let error = config_error
            .or(generation_error)
            .or(conversation_error)
            .or(onboarding_persistence_error)
            .or(setup_provider_error)
            .or(onboarding_error);
        Self {
            config,
            tutorial,
            onboarding,
            conversation_generation,
            ready,
            error,
        }
    }

    pub fn is_runtime_active(&self) -> bool {
        self.error.is_none()
    }

    pub fn log_status(&self, logger: &coosenpai_core::logging::FileLogger) -> std::io::Result<()> {
        logger.write(
            "INFO",
            &format!(
                "desktop 起動状態: runtime-active={} observer-provider={} companion-provider={} setup-required={} tutorial-active={}",
                self.is_runtime_active(),
                self.config.observer.vision.provider,
                self.config.companion.provider,
                self.onboarding.setup_required,
                self.onboarding.tutorial_active,
            ),
        )?;
        if let Some(error) = &self.error {
            logger.write(
                "ERROR",
                &format!(
                    "desktop 起動エラー: kind={:?} message={:?}",
                    error.kind, error.message
                ),
            )?;
        }
        Ok(())
    }
}

