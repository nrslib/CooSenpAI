use crate::commands::{authorize, CommandOrigin, IpcResult, TauriIpcResult};
use crate::factory::ProviderModelOptions;
use crate::model_catalog::ModelCatalogView;
use crate::state::DesktopState;
use coosenpai_core::locale::Locale;

pub(crate) async fn provider_models_for_state(
    origin: &str,
    state: &DesktopState,
) -> TauriIpcResult<Vec<ProviderModelOptions>> {
    authorize(origin, CommandOrigin::Main)?;
    let tutorial_active = state.tutorial_is_active().await;
    let mut values = match state
        .factory
        .provider_model_options(&state.runtime_config(), tutorial_active)
        .await
    {
        Ok(values) => values,
        Err(error) => {
            return Ok(IpcResult::failure(error.format_for_locale(
                Locale::from_config(&state.runtime_config().ui.language),
            )));
        }
    };
    if !tutorial_active {
        apply_model_catalog(
            &mut values,
            &crate::model_catalog::catalog_for_state(state).await,
        );
    }
    Ok(IpcResult::success(values))
}

fn apply_model_catalog(values: &mut [ProviderModelOptions], catalog: &ModelCatalogView) {
    for value in values {
        if let Some(provider) = catalog
            .providers
            .iter()
            .find(|provider| provider.provider == value.provider.as_str())
        {
            value.efforts = provider.efforts.clone();
            value.model_efforts = provider.model_efforts.clone();
        }
    }
}

