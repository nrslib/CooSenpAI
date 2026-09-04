use super::{
    canonical_daily_source, DailySummary, MemoryJob, MemoryJobKind, SourceInput, SourceInputKind,
    SourceSnapshot, SummaryState, WeeklySummary,
};
use crate::config::ConfigPaths;
use crate::persistence::{atomic_write_json, PersistenceError, SiblingLock};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[path = "store_validation.rs"]
mod validation;

#[derive(Debug, Error)]
pub enum MemoryStoreError {
    #[error("memory の永続化に失敗しました: {0}")]
    Persistence(#[from] PersistenceError),
    #[error("memory の I/O に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("memory JSON が不正です: {0}")]
    Json(#[from] serde_json::Error),
    #[error("memory source の canonical 化に失敗しました: {0}")]
    Canonical(#[from] super::CanonicalError),
    #[error("memory の期間キーが不正です")]
    InvalidPeriod,
}

#[derive(Debug, Clone)]
pub struct MemoryStore {
    paths: ConfigPaths,
}

impl MemoryStore {
    pub fn new(paths: ConfigPaths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &ConfigPaths {
        &self.paths
    }

    pub fn daily_source(
        &self,
        period: &str,
        source_max_bytes: usize,
    ) -> Result<SourceSnapshot, MemoryStoreError> {
        validate_daily_period(period)?;
        let conversation =
            read_jsonl_lines(&self.paths.conversation.join(format!("{period}.jsonl")))?;
        let observations =
            read_jsonl_lines(&self.paths.observations.join(format!("{period}.jsonl")))?;
        let inputs = conversation
            .iter()
            .map(|line| SourceInput {
                kind: SourceInputKind::Conversation,
                line,
            })
            .chain(observations.iter().map(|line| SourceInput {
                kind: SourceInputKind::Observation,
                line,
            }))
            .collect::<Vec<_>>();
        Ok(canonical_daily_source(&inputs, source_max_bytes)?)
    }

    pub fn load_job(
        &self,
        kind: MemoryJobKind,
        period: &str,
    ) -> Result<Option<MemoryJob>, MemoryStoreError> {
        let path = self.job_path(kind, period)?;
        read_optional_json(&path)
    }

    pub fn save_job(&self, job: &MemoryJob) -> Result<(), MemoryStoreError> {
        let path = self.job_path(job.kind, &job.period)?;
        let _lock = SiblingLock::acquire(&job_lock_path(&path))?;
        atomic_write_json(&path, job)?;
        Ok(())
    }

    pub fn update_job<F>(
        &self,
        kind: MemoryJobKind,
        period: &str,
        update: F,
    ) -> Result<Option<MemoryJob>, MemoryStoreError>
    where
        F: FnOnce(Option<MemoryJob>) -> Option<MemoryJob>,
    {
        let path = self.job_path(kind, period)?;
        let _lock = SiblingLock::acquire(&job_lock_path(&path))?;
        let current = read_optional_json(&path)?;
        let next = update(current);
        if let Some(job) = next.as_ref() {
            atomic_write_json(&path, job)?;
        }
        Ok(next)
    }

    pub fn update_job_with<F, T>(
        &self,
        kind: MemoryJobKind,
        period: &str,
        update: F,
    ) -> Result<(Option<MemoryJob>, T), MemoryStoreError>
    where
        F: FnOnce(Option<MemoryJob>) -> (Option<MemoryJob>, T),
    {
        let path = self.job_path(kind, period)?;
        let _lock = SiblingLock::acquire(&job_lock_path(&path))?;
        let current = read_optional_json(&path)?;
        let (next, output) = update(current);
        if let Some(job) = next.as_ref() {
            atomic_write_json(&path, job)?;
        }
        Ok((next, output))
    }

    pub fn jobs(&self) -> Result<Vec<MemoryJob>, MemoryStoreError> {
        read_json_directory(&self.paths.memory_jobs)
    }

    pub fn save_daily(&self, summary: &DailySummary) -> Result<(), MemoryStoreError> {
        validate_daily_period(&summary.local_date)?;
        let path = self
            .paths
            .memory_daily
            .join(format!("{}.json", summary.local_date));
        let _lock = SiblingLock::acquire(&job_lock_path(&path))?;
        atomic_write_json(&path, summary)?;
        Ok(())
    }

    pub fn load_daily(&self, period: &str) -> Result<Option<DailySummary>, MemoryStoreError> {
        validate_daily_period(period)?;
        validation::load_daily(self, period)
    }

    pub fn daily_summaries(&self) -> Result<Vec<DailySummary>, MemoryStoreError> {
        validation::daily_summaries(self)
    }

    pub fn load_current_daily(
        &self,
        period: &str,
        source_max_bytes: usize,
    ) -> Result<Option<DailySummary>, MemoryStoreError> {
        let Some(summary) = self.load_daily(period)? else {
            return Ok(None);
        };
        if summary.state != SummaryState::Current {
            return Ok(None);
        }
        let source = self.daily_source(period, source_max_bytes)?;
        Ok((source.source_digest == summary.source_digest).then_some(summary))
    }

    pub fn available_daily_periods(&self) -> Result<Vec<chrono::NaiveDate>, MemoryStoreError> {
        let mut periods = Vec::new();
        for directory in [&self.paths.conversation, &self.paths.observations] {
            if !directory.exists() {
                continue;
            }
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    if let Some(date) = entry
                        .path()
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
                    {
                        periods.push(date);
                    }
                }
            }
        }
        periods.sort_unstable();
        periods.dedup();
        Ok(periods)
    }

    pub fn save_weekly(&self, summary: &WeeklySummary) -> Result<(), MemoryStoreError> {
        validate_weekly_period(&summary.period)?;
        let path = self
            .paths
            .memory_weekly
            .join(format!("{}.json", summary.period));
        let _lock = SiblingLock::acquire(&job_lock_path(&path))?;
        atomic_write_json(&path, summary)?;
        Ok(())
    }

    pub fn weekly_summaries(&self) -> Result<Vec<WeeklySummary>, MemoryStoreError> {
        validation::weekly_summaries(self)
    }

    pub fn storage_bytes(&self) -> Result<u64, MemoryStoreError> {
        directory_bytes(&self.paths.memory)
    }

    pub fn prune(
        &self,
        today: chrono::NaiveDate,
        daily_days: u64,
        weekly_weeks: u64,
        job_days: u64,
    ) -> Result<(), MemoryStoreError> {
        let _lock = SiblingLock::acquire(&self.paths.memory.join(".retention.lock"))?;
        prune_daily(&self.paths.memory_daily, today, daily_days)?;
        prune_weekly(&self.paths.memory_weekly, today, weekly_weeks)?;
        prune_jobs(&self.paths.memory_jobs, today, job_days)?;
        Ok(())
    }

    fn job_path(&self, kind: MemoryJobKind, period: &str) -> Result<PathBuf, MemoryStoreError> {
        match kind {
            MemoryJobKind::Daily => validate_daily_period(period)?,
            MemoryJobKind::Weekly => validate_weekly_period(period)?,
        }
        Ok(self
            .paths
            .memory_jobs
            .join(format!("{}-{period}.json", kind.as_str())))
    }
}

fn prune_daily(
    directory: &Path,
    today: chrono::NaiveDate,
    retention: u64,
) -> Result<(), MemoryStoreError> {
    prune_directory(directory, |path| {
        path.file_stem()
            .and_then(|value| value.to_str())
            .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
            .is_some_and(|date| today.signed_duration_since(date).num_days() >= retention as i64)
    })
}

fn prune_weekly(
    directory: &Path,
    today: chrono::NaiveDate,
    retention: u64,
) -> Result<(), MemoryStoreError> {
    prune_directory(directory, |path| {
        path.file_stem()
            .and_then(|value| value.to_str())
            .and_then(parse_week)
            .is_some_and(|date| today.signed_duration_since(date).num_weeks() >= retention as i64)
    })
}

fn prune_jobs(
    directory: &Path,
    today: chrono::NaiveDate,
    retention: u64,
) -> Result<(), MemoryStoreError> {
    prune_directory(directory, |path| {
        fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<MemoryJob>(&bytes).ok())
            .and_then(|job| chrono::NaiveDate::parse_from_str(&job.day, "%Y-%m-%d").ok())
            .is_some_and(|date| today.signed_duration_since(date).num_days() >= retention as i64)
    })
}

fn prune_directory(
    directory: &Path,
    expired: impl Fn(&Path) -> bool,
) -> Result<(), MemoryStoreError> {
    if !directory.exists() {
        return Ok(());
    }
    let paths = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    let mut changed = false;
    for path in paths {
        if expired(&path) {
            fs::remove_file(path)?;
            changed = true;
        }
    }
    if changed {
        File::open(directory)?.sync_all()?;
    }
    Ok(())
}

fn parse_week(value: &str) -> Option<chrono::NaiveDate> {
    let (year, week) = value.split_once("-W")?;
    chrono::NaiveDate::from_isoywd_opt(year.parse().ok()?, week.parse().ok()?, chrono::Weekday::Mon)
}

fn read_jsonl_lines(path: &Path) -> Result<Vec<Vec<u8>>, MemoryStoreError> {
    let lock_path = path.with_file_name(format!(
        ".{}.lock",
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or(MemoryStoreError::InvalidPeriod)?
    ));
    let _lock = SiblingLock::acquire(&lock_path)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let reader = BufReader::new(File::open(path)?);
    reader
        .split(b'\n')
        .filter_map(|line| match line {
            Ok(line) if line.is_empty() => None,
            result => Some(result.map_err(MemoryStoreError::Io)),
        })
        .collect()
}

fn read_optional_json<T: serde::de::DeserializeOwned>(
    path: &Path,
) -> Result<Option<T>, MemoryStoreError> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
}

fn read_json_directory<T: serde::de::DeserializeOwned>(
    directory: &Path,
) -> Result<Vec<T>, MemoryStoreError> {
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .map(|path| Ok(serde_json::from_slice(&fs::read(path)?)?))
        .collect()
}

fn directory_bytes(directory: &Path) -> Result<u64, MemoryStoreError> {
    if !directory.exists() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        total = total.saturating_add(if metadata.is_dir() {
            directory_bytes(&entry.path())?
        } else if metadata.is_file() {
            metadata.len()
        } else {
            0
        });
    }
    Ok(total)
}

fn validate_daily_period(period: &str) -> Result<(), MemoryStoreError> {
    (period.len() == 10
        && period.as_bytes().get(4) == Some(&b'-')
        && period.as_bytes().get(7) == Some(&b'-'))
    .then_some(())
    .ok_or(MemoryStoreError::InvalidPeriod)?;
    chrono::NaiveDate::parse_from_str(period, "%Y-%m-%d")
        .map(|_| ())
        .map_err(|_| MemoryStoreError::InvalidPeriod)
}

pub fn memory_job_kind_for_period(period: &str) -> Result<MemoryJobKind, MemoryStoreError> {
    if validate_daily_period(period).is_ok() {
        Ok(MemoryJobKind::Daily)
    } else if validate_weekly_period(period).is_ok() {
        Ok(MemoryJobKind::Weekly)
    } else {
        Err(MemoryStoreError::InvalidPeriod)
    }
}

fn validate_weekly_period(period: &str) -> Result<(), MemoryStoreError> {
    let valid = period.len() == 8 && parse_week(period).is_some();
    valid.then_some(()).ok_or(MemoryStoreError::InvalidPeriod)
}

fn job_lock_path(path: &Path) -> PathBuf {
    path.with_extension("json.lock")
}
