use anyhow::{Context, Result};
use coosenpai_core::config::{Config, ConfigPaths};
use coosenpai_core::memory::{FactStore, MemoryService, MemoryStore};
use coosenpai_core::persistence::WatchLock;
use coosenpai_core::provider::ProviderClient;
use serde::Serialize;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryList {
    facts: Vec<coosenpai_core::memory::FactRecord>,
    candidates: coosenpai_core::memory::FactCandidatesSnapshot,
    daily: Vec<coosenpai_core::memory::DailySummary>,
    weekly: Vec<coosenpai_core::memory::WeeklySummary>,
}

pub(super) fn list(paths: &ConfigPaths) -> Result<()> {
    let fact_store = FactStore::new(paths.clone());
    let store = MemoryStore::new(paths.clone());
    let mut facts = fact_store.active_facts()?.into_values().collect::<Vec<_>>();
    facts.sort_by(|left, right| left.confirmed_at.cmp(&right.confirmed_at));
    println!(
        "{}",
        serde_json::to_string_pretty(&MemoryList {
            facts,
            candidates: fact_store.load_candidates()?,
            daily: store.daily_summaries()?,
            weekly: store.weekly_summaries()?,
        })?
    );
    Ok(())
}

pub(super) fn confirm(
    paths: &ConfigPaths,
    config: &Config,
    candidate_id: &str,
    confirmation_id: &str,
) -> Result<()> {
    let fact = FactStore::new(paths.clone()).confirm(
        candidate_id,
        confirmation_id,
        &now(),
        &config.memory,
    )?;
    println!("{}", serde_json::to_string_pretty(&fact)?);
    Ok(())
}

pub(super) fn reject(paths: &ConfigPaths, candidate_id: &str) -> Result<()> {
    FactStore::new(paths.clone()).reject(candidate_id)?;
    Ok(())
}

pub(super) fn confirm_update(
    paths: &ConfigPaths,
    config: &Config,
    update_id: &str,
    confirmation_id: &str,
) -> Result<()> {
    FactStore::new(paths.clone()).confirm_update(
        update_id,
        confirmation_id,
        &now(),
        &config.memory,
    )?;
    Ok(())
}

pub(super) fn delete(paths: &ConfigPaths, fact_id: &str, confirmation_id: &str) -> Result<()> {
    FactStore::new(paths.clone()).delete(fact_id, confirmation_id, &now())?;
    Ok(())
}

pub(super) async fn consolidate(
    paths: &ConfigPaths,
    config: &Config,
    period: &str,
    provider: Arc<dyn ProviderClient>,
) -> Result<()> {
    coosenpai_core::memory::memory_job_kind_for_period(period)
        .context("period は YYYY-MM-DD または YYYY-Www で指定してください")?;
    if !config.memory.enabled || !config.memory.provider_consent {
        anyhow::bail!("記憶と provider 送信への同意を有効にしてください")
    }
    let _watch_lock = WatchLock::acquire(&paths.watch_lock)
        .context("watch 実行中は watch 側が記憶を整理します")?;
    let mut service = MemoryService::new(
        provider,
        MemoryStore::new(paths.clone()),
        config.memory.clone(),
        config.companion.clone(),
    );
    service
        .consolidate(period, CancellationToken::new())
        .await
        .context("記憶を整理できませんでした")?;
    println!("{period} の記憶を整理しました");
    Ok(())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
