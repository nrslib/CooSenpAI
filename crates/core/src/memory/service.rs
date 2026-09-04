use super::canonical::canonical_weekly_source;
use super::jobs::{MEMORY_PROMPT_VERSION, MEMORY_SCHEMA_VERSION};
use super::{
    daily_summary_prompt, memory_job_id, memory_summary_schema, select_schedule,
    weekly_summary_prompt, DailySummary, JobFailureKind, MemoryJob, MemoryJobKind, MemoryJobPhase,
    MemorySchedule, MemoryStore, MemoryStoreError, ScheduleInput, SummaryState, WeeklySummary,
};
use crate::config::{CompanionConfig, MemoryConfig};
use crate::debug::{DebugError, DebugStore};
use crate::ports::{Clock, RuntimeLogger, SystemClock};
use crate::provider::{ProviderCall, ProviderClient, ProviderError, SessionRequest};
use chrono::{DateTime, Datelike, Local, NaiveDateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[path = "service_jobs.rs"]
mod job_lifecycle;

const SUMMARY_TEXT_MAX_BYTES: usize = 16 * 1_024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MemoryErrorKind {
    Provider,
    InvalidOutput,
    Persistence,
    Consent,
    SourceChanged,
    Capacity,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStatus {
    pub enabled: bool,
    pub provider_consent: bool,
    pub daily_count: usize,
    pub weekly_count: usize,
    pub fact_count: usize,
    pub candidate_count: usize,
    pub delayed_jobs: usize,
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_kind: Option<MemoryErrorKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_at: Option<String>,
    pub suggest_consolidation: bool,
    pub capacity_blocked: bool,
}

#[derive(Debug, Error)]
pub enum MemoryServiceError {
    #[error("記憶ジョブはユーザー優先のため中断されました")]
    Preempted,
    #[error("記憶 provider の呼び出しに失敗しました")]
    Provider(#[from] ProviderError),
    #[error("記憶 provider の応答が不正です")]
    InvalidOutput,
    #[error("記憶の provider 送信同意がありません")]
    ConsentWithdrawn,
    #[error("記憶 source が生成中に変更されました")]
    SourceChanged,
    #[error("記憶ジョブの呼び出し上限に達しました")]
    Capacity,
    #[error("この期間の記憶ジョブは同日の失敗後なので翌日の枠まで再実行しません")]
    DeferredAfterFailure,
    #[error("記憶の永続化に失敗しました: {0}")]
    Store(#[from] MemoryStoreError),
    #[error("記憶 source の canonical 化に失敗しました: {0}")]
    Canonical(#[from] super::CanonicalError),
    #[error("記憶 fact の永続化に失敗しました: {0}")]
    Fact(#[from] super::MemoryFactError),
}

pub struct MemoryService {
    provider: Arc<dyn ProviderClient>,
    store: MemoryStore,
    memory: MemoryConfig,
    companion: CompanionConfig,
    logger: Option<Arc<dyn RuntimeLogger>>,
    clock: Arc<dyn Clock>,
    status: MemoryStatus,
    debug_store: Option<DebugStore>,
    preempted: bool,
}

impl MemoryService {
    pub fn new(
        provider: Arc<dyn ProviderClient>,
        store: MemoryStore,
        memory: MemoryConfig,
        companion: CompanionConfig,
    ) -> Self {
        Self {
            provider,
            store,
            status: MemoryStatus {
                enabled: memory.enabled,
                provider_consent: memory.provider_consent,
                ..MemoryStatus::default()
            },
            memory,
            companion,
            logger: None,
            clock: Arc::new(SystemClock),
            debug_store: None,
            preempted: false,
        }
    }

    pub fn with_logger(mut self, logger: Arc<dyn RuntimeLogger>) -> Self {
        self.logger = Some(logger);
        self
    }

    pub fn with_debug_store(mut self, store: DebugStore) -> Self {
        self.debug_store = Some(store);
        self
    }

    fn record_debug_error(&self, error: DebugError) {
        if let Some(logger) = &self.logger {
            let _ = logger.write(
                "WARN",
                &format!(
                    "記憶のデバッグ記録に失敗しました: error-type=debug-persistence detail={error}"
                ),
            );
        }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    pub fn status(&self) -> &MemoryStatus {
        &self.status
    }

    pub fn was_preempted(&self) -> bool {
        self.preempted
    }

    pub fn update_config(&mut self, memory: MemoryConfig, companion: CompanionConfig) {
        self.status.enabled = memory.enabled;
        self.status.provider_consent = memory.provider_consent;
        self.memory = memory;
        self.companion = companion;
    }

    pub async fn run_due(
        &mut self,
        cancellation: CancellationToken,
    ) -> Result<MemoryStatus, MemoryServiceError> {
        let now = self.clock.now();
        let local = now.with_timezone(&Local).naive_local();
        let timezone = match iana_time_zone::get_timezone() {
            Ok(value) => value,
            Err(_) => "local".to_owned(),
        };
        self.run_due_at(now, local, &timezone, cancellation).await
    }

    pub async fn run_due_at(
        &mut self,
        now: DateTime<Utc>,
        local_now: NaiveDateTime,
        timezone_id: &str,
        cancellation: CancellationToken,
    ) -> Result<MemoryStatus, MemoryServiceError> {
        self.preempted = false;
        let result = self
            .run_due_at_inner(now, local_now, timezone_id, cancellation)
            .await;
        if let Err(error) = &result {
            self.record_error(error, retry_for(MemoryJobKind::Daily, local_now));
        }
        result
    }

    async fn run_due_at_inner(
        &mut self,
        now: DateTime<Utc>,
        local_now: NaiveDateTime,
        timezone_id: &str,
        cancellation: CancellationToken,
    ) -> Result<MemoryStatus, MemoryServiceError> {
        self.store.prune(
            local_now.date(),
            self.memory.daily_retention_days,
            self.memory.weekly_retention_weeks,
            self.memory.job_retention_days,
        )?;
        self.reconcile_jobs(local_now.date(), now)?;
        super::FactStore::new(self.store.paths().clone()).reconcile(&self.memory)?;
        self.refresh_freshness()?;
        let mut attempted = false;
        let mut failed = false;
        let generated = self
            .store
            .jobs()?
            .into_iter()
            .filter(|job| job.phase == MemoryJobPhase::Generated)
            .map(|job| (job.kind, job.period))
            .collect();
        let outcome = self
            .run_periods(generated, now, local_now, timezone_id, &cancellation)
            .await;
        attempted |= outcome.0;
        failed |= outcome.1;
        self.refresh_freshness()?;
        self.refresh_status(&self.schedule(local_now)?)?;
        if !self.memory.enabled || !self.memory.provider_consent {
            if self.memory.enabled {
                self.fail_active_jobs_for_withdrawn_consent(now)?;
            }
            return Ok(self.status.clone());
        }
        if self.status.capacity_blocked {
            self.status.last_error_kind = Some(MemoryErrorKind::Capacity);
            return Ok(self.status.clone());
        }
        let active = self
            .store
            .jobs()?
            .into_iter()
            .filter(|job| {
                job.phase == MemoryJobPhase::Reserved && job.day == local_now.date().to_string()
            })
            .map(|job| (job.kind, job.period))
            .collect::<Vec<_>>();
        let outcome = self
            .run_periods(active, now, local_now, timezone_id, &cancellation)
            .await;
        attempted |= outcome.0;
        failed |= outcome.1;
        self.refresh_freshness()?;
        let schedule = self.schedule(local_now)?;
        self.refresh_status(&schedule)?;
        if self.status.capacity_blocked {
            self.status.last_error_kind = Some(MemoryErrorKind::Capacity);
            return Ok(self.status.clone());
        }
        let scheduled = schedule
            .daily
            .into_iter()
            .map(|period| (MemoryJobKind::Daily, period))
            .chain(
                schedule
                    .weekly
                    .into_iter()
                    .map(|period| (MemoryJobKind::Weekly, period)),
            )
            .collect();
        let outcome = self
            .run_periods(scheduled, now, local_now, timezone_id, &cancellation)
            .await;
        attempted |= outcome.0;
        failed |= outcome.1;
        if attempted && !failed {
            self.status.last_error_kind = None;
            self.status.retry_at = None;
        }
        self.refresh_status(&self.schedule(local_now)?)?;
        Ok(self.status.clone())
    }

    pub async fn consolidate(
        &mut self,
        period: &str,
        cancellation: CancellationToken,
    ) -> Result<MemoryStatus, MemoryServiceError> {
        let now = self.clock.now();
        let local_date = now.with_timezone(&Local).date_naive();
        if !self.memory.enabled || !self.memory.provider_consent {
            self.record_error(
                &MemoryServiceError::ConsentWithdrawn,
                retry_for(
                    MemoryJobKind::Daily,
                    now.with_timezone(&Local).naive_local(),
                ),
            );
            return Err(MemoryServiceError::ConsentWithdrawn);
        }
        self.store.prune(
            local_date,
            self.memory.daily_retention_days,
            self.memory.weekly_retention_weeks,
            self.memory.job_retention_days,
        )?;
        self.reconcile_jobs(local_date, now)?;
        self.refresh_status(&self.schedule(now.with_timezone(&Local).naive_local())?)?;
        if self.status.capacity_blocked {
            self.status.last_error_kind = Some(MemoryErrorKind::Capacity);
            return Err(MemoryServiceError::Capacity);
        }
        let kind = super::memory_job_kind_for_period(period)?;
        let local_now = now.with_timezone(&Local).naive_local();
        let timezone_id = iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_owned());
        let result = self
            .run_period(kind, period, local_date, now, &timezone_id, cancellation)
            .await;
        if let Err(error) = result {
            self.record_error(&error, retry_for(kind, local_now));
            return Err(error);
        }
        if let Some(job) = self.store.load_job(kind, period)? {
            if job.phase == MemoryJobPhase::Failed {
                return Err(MemoryServiceError::DeferredAfterFailure);
            }
            if job.phase == MemoryJobPhase::Committed {
                self.status.last_error_kind = None;
                self.status.retry_at = None;
            }
        }
        self.refresh_status(&self.schedule(now.with_timezone(&Local).naive_local())?)?;
        Ok(self.status.clone())
    }

    async fn run_period(
        &mut self,
        kind: MemoryJobKind,
        period: &str,
        execution_day: chrono::NaiveDate,
        now: DateTime<Utc>,
        timezone_id: &str,
        cancellation: CancellationToken,
    ) -> Result<(), MemoryServiceError> {
        match kind {
            MemoryJobKind::Daily => {
                self.run_daily(period, execution_day, now, timezone_id, cancellation)
                    .await
            }
            MemoryJobKind::Weekly => {
                self.run_weekly(period, execution_day, now, timezone_id, cancellation)
                    .await
            }
        }
    }

    async fn run_periods(
        &mut self,
        jobs: Vec<(MemoryJobKind, String)>,
        now: DateTime<Utc>,
        local_now: NaiveDateTime,
        timezone_id: &str,
        cancellation: &CancellationToken,
    ) -> (bool, bool) {
        let attempted = !jobs.is_empty();
        let mut failed = false;
        for (kind, period) in jobs {
            if let Err(error) = self
                .run_period(
                    kind,
                    &period,
                    local_now.date(),
                    now,
                    timezone_id,
                    cancellation.child_token(),
                )
                .await
            {
                failed = true;
                if matches!(error, MemoryServiceError::Preempted) {
                    self.preempted = true;
                    continue;
                }
                self.record_error(&error, retry_for(kind, local_now));
            }
        }
        (attempted, failed)
    }

    fn schedule(&self, local_now: NaiveDateTime) -> Result<MemorySchedule, MemoryServiceError> {
        let available = self.store.available_daily_periods()?;
        let daily = self.store.daily_summaries()?;
        let current = daily
            .iter()
            .filter(|summary| summary.state == SummaryState::Current)
            .filter_map(|summary| {
                chrono::NaiveDate::parse_from_str(&summary.local_date, "%Y-%m-%d").ok()
            })
            .collect::<Vec<_>>();
        let failed_or_stale = daily
            .iter()
            .filter(|summary| summary.state == SummaryState::Stale)
            .filter_map(|summary| {
                chrono::NaiveDate::parse_from_str(&summary.local_date, "%Y-%m-%d").ok()
            })
            .collect::<Vec<_>>();
        let jobs = self.store.jobs()?;
        let day = local_now.date().to_string();
        let week = local_now.date().iso_week();
        let daily_jobs_today = jobs
            .iter()
            .filter(|job| job.kind == MemoryJobKind::Daily && job.day == day)
            .map(|job| job.period.clone())
            .collect();
        let weekly_jobs_this_week = jobs
            .iter()
            .filter(|job| {
                job.kind == MemoryJobKind::Weekly
                    && chrono::NaiveDate::parse_from_str(&job.day, "%Y-%m-%d").is_ok_and(|date| {
                        date.iso_week().year() == week.year()
                            && date.iso_week().week() == week.week()
                    })
            })
            .map(|job| job.period.clone())
            .collect();
        let stale_weekly_periods = self
            .store
            .weekly_summaries()?
            .into_iter()
            .filter(|summary| summary.state == SummaryState::Stale)
            .map(|summary| summary.period)
            .collect();
        Ok(select_schedule(&ScheduleInput {
            local_now,
            grace_minutes: self.memory.grace_minutes,
            available_daily_periods: available,
            current_daily_periods: current,
            failed_or_stale_daily_periods: failed_or_stale,
            daily_jobs_today,
            weekly_jobs_this_week,
            stale_weekly_periods,
        }))
    }

    async fn run_daily(
        &mut self,
        period: &str,
        execution_day: chrono::NaiveDate,
        now: DateTime<Utc>,
        timezone_id: &str,
        cancellation: CancellationToken,
    ) -> Result<(), MemoryServiceError> {
        let source = self
            .store
            .daily_source(period, self.memory.source_max_bytes)?;
        if source.source_ids.is_empty() {
            return Ok(());
        }
        let mut job =
            self.reserve_job(MemoryJobKind::Daily, period, &source, execution_day, now)?;
        if matches!(
            job.phase,
            MemoryJobPhase::Committed | MemoryJobPhase::Failed
        ) {
            return Ok(());
        }
        if job.phase == MemoryJobPhase::Reserved && source.source_digest != job.source_digest {
            self.fail_job_if_current(&mut job, JobFailureKind::SourceChanged, now)?;
            return Err(MemoryServiceError::SourceChanged);
        }
        let text = self
            .generate(
                &mut job,
                daily_summary_prompt(&source.canonical_bytes)?,
                cancellation,
            )
            .await?;
        let current = self
            .store
            .daily_source(period, self.memory.source_max_bytes)?;
        if current.source_digest != job.source_digest {
            self.fail_job_if_current(&mut job, JobFailureKind::SourceChanged, now)?;
            return Err(MemoryServiceError::SourceChanged);
        }
        self.store.save_daily(&DailySummary {
            schema_version: MEMORY_SCHEMA_VERSION,
            local_date: period.to_owned(),
            time_zone_id: timezone_id.to_owned(),
            source_digest: job.source_digest.clone(),
            source_ids: job.source_ids.clone(),
            truncated: job.source_truncated,
            prompt_version: job.prompt_version,
            provider: job.provider.clone(),
            model: job.model.clone(),
            generated_at: timestamp(now),
            text_digest: digest(text.as_bytes()),
            text,
            state: SummaryState::Current,
        })?;
        self.commit_job(&mut job, now)?;
        Ok(())
    }

    async fn run_weekly(
        &mut self,
        period: &str,
        execution_day: chrono::NaiveDate,
        now: DateTime<Utc>,
        timezone_id: &str,
        cancellation: CancellationToken,
    ) -> Result<(), MemoryServiceError> {
        let daily = self.store.daily_summaries()?;
        let (source, dependencies) =
            canonical_weekly_source(&daily, period, self.memory.source_max_bytes)?;
        if source.source_ids.is_empty() {
            return Ok(());
        }
        let mut job =
            self.reserve_job(MemoryJobKind::Weekly, period, &source, execution_day, now)?;
        if matches!(
            job.phase,
            MemoryJobPhase::Committed | MemoryJobPhase::Failed
        ) {
            return Ok(());
        }
        if job.phase == MemoryJobPhase::Reserved && source.source_digest != job.source_digest {
            self.fail_job_if_current(&mut job, JobFailureKind::SourceChanged, now)?;
            return Err(MemoryServiceError::SourceChanged);
        }
        let text = self
            .generate(
                &mut job,
                weekly_summary_prompt(&source.canonical_bytes)?,
                cancellation,
            )
            .await?;
        let (current, current_dependencies) = canonical_weekly_source(
            &self.store.daily_summaries()?,
            period,
            self.memory.source_max_bytes,
        )?;
        if current.source_digest != job.source_digest || current_dependencies != dependencies {
            self.fail_job_if_current(&mut job, JobFailureKind::SourceChanged, now)?;
            return Err(MemoryServiceError::SourceChanged);
        }
        self.store.save_weekly(&WeeklySummary {
            schema_version: MEMORY_SCHEMA_VERSION,
            period: period.to_owned(),
            time_zone_id: timezone_id.to_owned(),
            source_digest: job.source_digest.clone(),
            source_ids: job.source_ids.clone(),
            truncated: job.source_truncated,
            prompt_version: job.prompt_version,
            provider: job.provider.clone(),
            model: job.model.clone(),
            generated_at: timestamp(now),
            text_digest: digest(text.as_bytes()),
            text,
            state: SummaryState::Current,
            depends_on: dependencies,
        })?;
        self.commit_job(&mut job, now)?;
        Ok(())
    }

    fn refresh_freshness(&self) -> Result<(), MemoryServiceError> {
        let available_periods = self.store.available_daily_periods()?;
        let mut daily = self.store.daily_summaries()?;
        for summary in &mut daily {
            if summary.state != SummaryState::Current {
                continue;
            }
            let available = chrono::NaiveDate::parse_from_str(&summary.local_date, "%Y-%m-%d")
                .is_ok_and(|period| available_periods.contains(&period));
            if !available {
                continue;
            }
            let current = self
                .store
                .daily_source(&summary.local_date, self.memory.source_max_bytes)?;
            if current.source_digest != summary.source_digest {
                summary.state = SummaryState::Stale;
                self.store.save_daily(summary)?;
            }
        }
        let current_daily = self.store.daily_summaries()?;
        for mut weekly in self.store.weekly_summaries()? {
            if weekly.state != SummaryState::Current {
                continue;
            }
            let (source, dependencies) = canonical_weekly_source(
                &current_daily,
                &weekly.period,
                self.memory.source_max_bytes,
            )?;
            if source.source_digest != weekly.source_digest
                || source.source_ids != weekly.source_ids
                || source.truncated != weekly.truncated
                || dependencies != weekly.depends_on
            {
                weekly.state = SummaryState::Stale;
                self.store.save_weekly(&weekly)?;
            }
        }
        Ok(())
    }

    fn refresh_status(&mut self, schedule: &MemorySchedule) -> Result<(), MemoryServiceError> {
        let daily = self.store.daily_summaries()?;
        let weekly = self.store.weekly_summaries()?;
        let facts = super::FactStore::new(self.store.paths().clone()).active_facts()?;
        let candidates = super::FactStore::new(self.store.paths().clone()).load_candidates()?;
        let bytes = self.store.storage_bytes()? as usize;
        self.status.enabled = self.memory.enabled;
        self.status.provider_consent = self.memory.provider_consent;
        self.status.daily_count = daily.len();
        self.status.weekly_count = weekly.len();
        self.status.fact_count = facts.len();
        self.status.candidate_count = candidates.candidates.len() + candidates.updates.len();
        self.status.delayed_jobs = schedule.delayed_daily + schedule.delayed_weekly;
        self.status.stale = daily
            .iter()
            .any(|summary| summary.state == SummaryState::Stale)
            || weekly
                .iter()
                .any(|summary| summary.state == SummaryState::Stale);
        self.status.suggest_consolidation = bytes >= self.memory.storage_max_bytes * 9 / 10;
        self.status.capacity_blocked = bytes >= self.memory.storage_max_bytes;
        if !self.status.capacity_blocked
            && self.status.last_error_kind == Some(MemoryErrorKind::Capacity)
        {
            self.status.last_error_kind = None;
            self.status.retry_at = None;
        }
        Ok(())
    }

    fn record_error(&mut self, error: &MemoryServiceError, retry_at: NaiveDateTime) {
        self.status.last_error_kind = Some(match error {
            MemoryServiceError::Preempted => return,
            MemoryServiceError::Provider(_) => MemoryErrorKind::Provider,
            MemoryServiceError::InvalidOutput => MemoryErrorKind::InvalidOutput,
            MemoryServiceError::ConsentWithdrawn => MemoryErrorKind::Consent,
            MemoryServiceError::SourceChanged => MemoryErrorKind::SourceChanged,
            MemoryServiceError::Capacity => MemoryErrorKind::Capacity,
            MemoryServiceError::DeferredAfterFailure => MemoryErrorKind::Provider,
            MemoryServiceError::Store(_)
            | MemoryServiceError::Canonical(_)
            | MemoryServiceError::Fact(_) => MemoryErrorKind::Persistence,
        });
        self.status.retry_at = Some(retry_at.to_string());
        if let Some(logger) = &self.logger {
            let detail = match error {
                MemoryServiceError::Provider(error) => format!(" detail={}", error.message),
                _ => String::new(),
            };
            let _ = logger.write(
                "WARN",
                &format!("記憶の生成に失敗しました: error-type=memory{detail}"),
            );
        }
    }
}

fn timestamp(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn model_value(value: &str) -> Option<String> {
    (value != "default").then(|| value.to_owned())
}

fn effort_value(value: &str) -> Option<String> {
    (value != "default").then(|| value.to_owned())
}

fn retry_for(kind: MemoryJobKind, local_now: NaiveDateTime) -> NaiveDateTime {
    match kind {
        MemoryJobKind::Daily => local_now + chrono::Duration::days(1),
        MemoryJobKind::Weekly => local_now + chrono::Duration::weeks(1),
    }
}
