use crate::commands::{authorize, CommandOrigin, IpcResult, TauriIpcResult};
use crate::factory::ProviderModelOptions;
use crate::state::DesktopState;
use coosenpai_core::locale::Locale;

pub(crate) async fn provider_models_for_state(
    origin: &str,
    state: &DesktopState,
) -> TauriIpcResult<Vec<ProviderModelOptions>> {
    authorize(origin, CommandOrigin::Main)?;
    let tutorial_active = state.tutorial_is_active().await;
    Ok(
        match state
            .factory
            .provider_model_options(&state.runtime_config(), tutorial_active)
            .await
        {
            Ok(values) => IpcResult::success(values),
            Err(error) => IpcResult::failure(
                error.format_for_locale(Locale::from_config(&state.runtime_config().ui.language)),
            ),
        },
    )
}

