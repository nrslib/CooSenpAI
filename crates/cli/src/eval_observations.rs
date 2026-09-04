use anyhow::Result;
use chrono::{DateTime, Utc};
use coosenpai_core::config::ConfigPaths;
use coosenpai_core::state::ObservationRecord;

pub(super) fn read_recent(
    paths: &ConfigPaths,
    retention_days: u64,
) -> Result<Vec<ObservationRecord>> {
    read_recent_at(paths, retention_days, Utc::now())
}

pub(super) fn read_recent_at(
    paths: &ConfigPaths,
    _retention_days: u64,
    now: DateTime<Utc>,
) -> Result<Vec<ObservationRecord>> {
    Ok(coosenpai_core::recent_observations::read_recent_observations(&paths.observations, now)?)
}
