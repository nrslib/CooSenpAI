use super::commands::{
    authorize_window, validate_id_for_locale, CommandOrigin, IpcResult, MemoryCatalog,
    MemoryConfirmPayload, MemoryConfirmUpdatePayload, MemoryConsolidatePayload,
    MemoryDeletePayload, MemoryRejectPayload, MemoryRejectUpdatePayload, TauriIpcResult,
};
use crate::command_guard::{CommandSource, DesktopCommand};
use crate::commands::dispatch_result;
use crate::state::DesktopState;
use coosenpai_core::config::ConfigPaths;
use coosenpai_core::locale::{localize_error_message, text, Locale, TextKey};
use coosenpai_core::memory::{FactStore, MemoryStore};
use std::sync::Arc;
use tauri::{State, WebviewWindow};

#[tauri::command]
pub async fn memory_list(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
) -> TauriIpcResult<MemoryCatalog> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    Ok(match load_memory_catalog(&state.paths, locale) {
        Ok(value) => IpcResult::success(value),
        Err(error) => IpcResult::failure(error),
    })
}

#[tauri::command]
pub async fn memory_confirm(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: MemoryConfirmPayload,
) -> TauriIpcResult<MemoryCatalog> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    validate_id_for_locale(&payload.candidate_id, locale)?;
    validate_id_for_locale(&payload.confirmation_id, locale)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::MemoryConfirm,
        move |context| async move {
            let config = handler_state.runtime_config();
            let store = FactStore::new(handler_state.paths.clone());
            if let Err(error) = store.confirm(
                &payload.candidate_id,
                &payload.confirmation_id,
                &timestamp(),
                &config.memory,
            ) {
                return IpcResult::failure(localize_error_message(
                    &error.to_string(),
                    TextKey::MemoryOperationFailed,
                    locale,
                ));
            }
            if let Err(error) = handler_state
                .command_sync_resolved_fact_prompt(&context, &payload.candidate_id)
                .await
            {
                return IpcResult::failure(error.format_for_locale(locale));
            }
            memory_catalog_result(&handler_state.paths, locale)
        },
    )
    .await)
}

#[tauri::command]
pub async fn memory_reject(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: MemoryRejectPayload,
) -> TauriIpcResult<MemoryCatalog> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    validate_id_for_locale(&payload.candidate_id, locale)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::MemoryReject,
        move |context| async move {
            if let Err(error) =
                FactStore::new(handler_state.paths.clone()).reject(&payload.candidate_id)
            {
                return IpcResult::failure(localize_error_message(
                    &error.to_string(),
                    TextKey::MemoryOperationFailed,
                    locale,
                ));
            }
            if let Err(error) = handler_state
                .command_sync_resolved_fact_prompt(&context, &payload.candidate_id)
                .await
            {
                return IpcResult::failure(error.format_for_locale(locale));
            }
            memory_catalog_result(&handler_state.paths, locale)
        },
    )
    .await)
}

#[tauri::command]
pub async fn memory_confirm_update(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: MemoryConfirmUpdatePayload,
) -> TauriIpcResult<MemoryCatalog> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    validate_id_for_locale(&payload.update_id, locale)?;
    validate_id_for_locale(&payload.confirmation_id, locale)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::MemoryConfirmUpdate,
        move |_context| async move {
            let config = handler_state.runtime_config();
            if let Err(error) = FactStore::new(handler_state.paths.clone()).confirm_update(
                &payload.update_id,
                &payload.confirmation_id,
                &timestamp(),
                &config.memory,
            ) {
                return IpcResult::failure(localize_error_message(
                    &error.to_string(),
                    TextKey::MemoryOperationFailed,
                    locale,
                ));
            }
            memory_catalog_result(&handler_state.paths, locale)
        },
    )
    .await)
}

#[tauri::command]
pub async fn memory_reject_update(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: MemoryRejectUpdatePayload,
) -> TauriIpcResult<MemoryCatalog> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    validate_id_for_locale(&payload.update_id, locale)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::MemoryRejectUpdate,
        move |_context| async move {
            if let Err(error) =
                FactStore::new(handler_state.paths.clone()).reject_update(&payload.update_id)
            {
                return IpcResult::failure(localize_error_message(
                    &error.to_string(),
                    TextKey::MemoryOperationFailed,
                    locale,
                ));
            }
            memory_catalog_result(&handler_state.paths, locale)
        },
    )
    .await)
}

#[tauri::command]
pub async fn memory_delete(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: MemoryDeletePayload,
) -> TauriIpcResult<MemoryCatalog> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    validate_id_for_locale(&payload.fact_id, locale)?;
    validate_id_for_locale(&payload.confirmation_id, locale)?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::MemoryDelete,
        move |_context| async move {
            if let Err(error) = FactStore::new(handler_state.paths.clone()).delete(
                &payload.fact_id,
                &payload.confirmation_id,
                &timestamp(),
            ) {
                return IpcResult::failure(localize_error_message(
                    &error.to_string(),
                    TextKey::MemoryOperationFailed,
                    locale,
                ));
            }
            memory_catalog_result(&handler_state.paths, locale)
        },
    )
    .await)
}

#[tauri::command]
pub async fn memory_consolidate(
    window: WebviewWindow,
    state: State<'_, Arc<DesktopState>>,
    payload: MemoryConsolidatePayload,
) -> TauriIpcResult<MemoryCatalog> {
    authorize_window(&window, CommandOrigin::Main)?;
    let locale = Locale::from_config(&state.runtime_config().ui.language);
    coosenpai_core::memory::memory_job_kind_for_period(&payload.period)
        .map_err(|_| text(TextKey::MemoryPeriodInvalid, locale).to_owned())?;
    let state = state.inner().clone();
    let handler_state = state.clone();
    Ok(dispatch_result(
        state,
        CommandSource::IpcMain,
        DesktopCommand::MemoryConsolidate,
        move |_context| async move {
            if let Err(error) = handler_state
                .core_runtime()
                .consolidate_memory(payload.period)
                .await
            {
                return IpcResult::failure(localize_error_message(
                    &error.to_string(),
                    TextKey::MemoryOperationFailed,
                    locale,
                ));
            }
            memory_catalog_result(&handler_state.paths, locale)
        },
    )
    .await)
}

fn memory_catalog_result(paths: &ConfigPaths, locale: Locale) -> IpcResult<MemoryCatalog> {
    match load_memory_catalog(paths, locale) {
        Ok(value) => IpcResult::success(value),
        Err(error) => IpcResult::failure(error),
    }
}

fn load_memory_catalog(paths: &ConfigPaths, locale: Locale) -> Result<MemoryCatalog, String> {
    let facts = FactStore::new(paths.clone());
    let store = MemoryStore::new(paths.clone());
    let mut active = facts
        .active_facts()
        .map_err(|error| {
            localize_error_message(&error.to_string(), TextKey::MemoryOperationFailed, locale)
        })?
        .into_values()
        .collect::<Vec<_>>();
    active.sort_by(|left, right| right.confirmed_at.cmp(&left.confirmed_at));
    let candidates = facts.load_candidates().map_err(|error| {
        localize_error_message(&error.to_string(), TextKey::MemoryOperationFailed, locale)
    })?;
    Ok(MemoryCatalog {
        facts: active,
        candidates: candidates.candidates,
        updates: candidates.updates,
        daily: store.daily_summaries().map_err(|error| {
            localize_error_message(&error.to_string(), TextKey::MemoryOperationFailed, locale)
        })?,
        weekly: store.weekly_summaries().map_err(|error| {
            localize_error_message(&error.to_string(), TextKey::MemoryOperationFailed, locale)
        })?,
    })
}

fn timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

