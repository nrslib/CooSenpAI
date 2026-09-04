use crate::commands::{authorize, CommandOrigin, IpcResult, TauriIpcResult};
use crate::factory::ProviderModelOptions;
use crate::state::DesktopState;

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
            Err(error) => IpcResult::failure(error.to_string()),
        },
    )
}

