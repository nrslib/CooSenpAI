use crate::command_guard::{CommandSource, DesktopCommand, OnboardingPhase};
use crate::commands::IpcResult;
use crate::presentation::PresentationEvent;
use crate::snapshot::AppSnapshot;
use crate::state::DesktopState;
use crate::ui_events::PresenterId;
use coosenpai_core::locale::{text, Locale, TextKey};
use std::sync::Arc;

pub(crate) type WindowEvent = PresentationEvent<WindowRequest, WindowContent>;

#[derive(Debug)]
pub(crate) enum WindowRequest {
    Main,
    BubbleClick(crate::bubbles::BubbleClickTarget),
    Settings { section: Option<&'static str> },
    Details,
    ModelPicker,
    PreparedMain(Arc<MainContent>),
}

#[derive(Debug, Clone)]
pub(crate) struct MainContent {
    pub(crate) snapshot: Arc<AppSnapshot>,
    pub(crate) personas: Vec<crate::factory::PersonaOption>,
    pub(crate) advance_tutorial: bool,
    pub(crate) bubble_click: Option<crate::bubbles::BubbleClickTarget>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct SettingsResources {
    pub(crate) models: IpcResult<Vec<crate::factory::ProviderModelOptions>>,
    pub(crate) keys: IpcResult<coosenpai_core::provider_api_keys::ProviderApiKeyStatus>,
}

#[derive(Debug)]
pub(crate) enum WindowContent {
    Main(Arc<MainContent>),
    Settings {
        main: Arc<MainContent>,
        resources: SettingsResources,
        section: Option<&'static str>,
    },
    Details {
        snapshot: Arc<AppSnapshot>,
        history: IpcResult<coosenpai_core::dataflow_log::DataFlowLog>,
    },
    ModelPicker {
        snapshot: Arc<AppSnapshot>,
        catalog: crate::model_catalog::ModelCatalogView,
    },
}

impl WindowContent {
    pub(crate) fn view(&self) -> PresenterId {
        match self {
            Self::Main(_) => PresenterId::Chat,
            Self::Settings { .. } => PresenterId::Settings,
            Self::Details { .. } => PresenterId::Details,
            Self::ModelPicker { .. } => PresenterId::ModelPicker,
        }
    }
}

pub(crate) async fn load(
    state: &Arc<DesktopState>,
    request: WindowRequest,
) -> Result<Option<WindowContent>, String> {
    let content = match request {
        WindowRequest::Main | WindowRequest::BubbleClick(_) => {
            let phase = state.onboarding_policy_phase().await;
            if phase == OnboardingPhase::Setup {
                return Ok(None);
            }
            let advance_tutorial = matches!(
                phase,
                OnboardingPhase::Tutorial {
                    step: coosenpai_core::onboarding::TutorialStep::Chat,
                    ..
                }
            );
            WindowContent::Main(main_content(state, advance_tutorial).await?)
        }
        WindowRequest::PreparedMain(content) => WindowContent::Main(content),
        WindowRequest::Settings { section } => {
            prepare_settings(state).await?;
            state.refresh_speech_input_devices().await;
            let models =
                crate::commands_provider_models::provider_models_for_state("main", state).await?;
            let locale = Locale::from_config(&state.runtime_config().ui.language);
            let keys = match state.factory.provider_api_key_status() {
                Ok(value) => IpcResult::success(value),
                Err(error) => IpcResult::failure(error.format_for_locale(locale)),
            };
            WindowContent::Settings {
                main: main_content(state, false).await?,
                resources: SettingsResources { models, keys },
                section,
            }
        }
        WindowRequest::Details => details_content(state).await,
        WindowRequest::ModelPicker => model_content(state).await,
    };
    Ok(Some(content))
}

pub(crate) async fn refresh(
    state: &Arc<DesktopState>,
    view: PresenterId,
) -> Result<WindowContent, String> {
    match view {
        PresenterId::Chat => Ok(WindowContent::Main(main_content(state, false).await?)),
        PresenterId::Details => Ok(details_content(state).await),
        PresenterId::ModelPicker => Ok(model_content(state).await),
        _ => Err(format!("未対応のView初期化: {view:?}")),
    }
}

async fn main_content(
    state: &DesktopState,
    advance_tutorial: bool,
) -> Result<Arc<MainContent>, String> {
    let personas = crate::factory::persona_options(&state.paths)?;
    Ok(Arc::new(MainContent {
        snapshot: Arc::new(state.snapshot().await),
        personas,
        advance_tutorial,
        bubble_click: None,
    }))
}

async fn details_content(state: &DesktopState) -> WindowContent {
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    let history = match coosenpai_core::dataflow_log::read_dataflow_log(&state.paths, 200) {
        Ok(history) => IpcResult::success(history),
        Err(error) => IpcResult::failure(
            text(TextKey::DetailsDataflowLogReadFailed, locale)
                .replace("{error}", &error.to_string()),
        ),
    };
    WindowContent::Details {
        snapshot: Arc::new(state.snapshot().await),
        history,
    }
}

async fn model_content(state: &Arc<DesktopState>) -> WindowContent {
    let catalog = crate::model_catalog::catalog_for_state(state).await;
    WindowContent::ModelPicker {
        snapshot: Arc::new(state.snapshot().await),
        catalog,
    }
}

async fn prepare_settings(state: &Arc<DesktopState>) -> Result<(), String> {
    let handler_state = state.clone();
    let allowed = state
        .dispatch(
            CommandSource::IpcMain,
            DesktopCommand::SettingsOpen,
            move |context| async move {
                handler_state
                    .command_tutorial_settings_opened(&context)
                    .await
                    .map_err(|error| {
                        crate::command_guard::DispatchError::handler(error.format_for_locale(
                            Locale::from_config(&handler_state.runtime_config().ui.language),
                        ))
                    })
            },
        )
        .await
        .map_err(|error| {
            error.format_for_locale(Locale::from_config(&state.runtime_config().ui.language))
        })?;
    if !allowed {
        return Err(text(
            TextKey::TutorialPersonaWaiting,
            Locale::from_config(&state.runtime_config().ui.language),
        )
        .to_owned());
    }
    Ok(())
}

