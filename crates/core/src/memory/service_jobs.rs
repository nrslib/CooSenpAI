use super::*;

impl MemoryService {
    pub(super) fn reserve_job(
        &self,
        kind: MemoryJobKind,
        period: &str,
        source: &super::super::SourceSnapshot,
        execution_day: chrono::NaiveDate,
        now: DateTime<Utc>,
    ) -> Result<MemoryJob, MemoryServiceError> {
        let day = execution_day.to_string();
        let jobs = self.store.jobs()?;
        let used = match kind {
            MemoryJobKind::Daily => jobs
                .iter()
                .filter(|job| job.kind == kind && job.day == day)
                .count(),
            MemoryJobKind::Weekly => {
                let current_week = execution_day.iso_week();
                jobs.iter()
                    .filter(|job| job.kind == kind)
                    .filter(|job| {
                        chrono::NaiveDate::parse_from_str(&job.day, "%Y-%m-%d")
                            .is_ok_and(|date| date.iso_week() == current_week)
                    })
                    .count()
            }
        };
        let created = timestamp(now);
        let candidate = MemoryJob {
            schema_version: MEMORY_SCHEMA_VERSION,
            job_id: memory_job_id(
                kind.as_str(),
                period,
                &source.source_digest,
                MEMORY_PROMPT_VERSION,
                &self.companion.provider,
                &self.companion.model,
            ),
            kind,
            period: period.to_owned(),
            day: day.clone(),
            phase: MemoryJobPhase::Reserved,
            source_digest: source.source_digest.clone(),
            source_ids: source.source_ids.clone(),
            source_truncated: source.truncated,
            skipped_invalid_count: source.skipped_invalid_count,
            prompt_version: MEMORY_PROMPT_VERSION,
            provider: self.companion.provider.clone(),
            model: self.companion.model.clone(),
            created_at: created.clone(),
            updated_at: created,
            generated_text: None,
            failure_kind: None,
        };
        let (stored, reserved) = self.store.update_job_with(kind, period, |current| {
            if let Some(existing) = current {
                if existing.day == day || existing.phase == MemoryJobPhase::Generated {
                    return (Some(existing), Ok(()));
                }
            }
            if used >= 2 {
                return (None, Err(MemoryServiceError::Capacity));
            }
            (Some(candidate), Ok(()))
        })?;
        reserved?;
        stored.ok_or(MemoryServiceError::DeferredAfterFailure)
    }

    pub(super) async fn generate(
        &self,
        job: &mut MemoryJob,
        prompt: String,
        cancellation: CancellationToken,
    ) -> Result<String, MemoryServiceError> {
        if let Some(text) = job.generated_text.clone() {
            return Ok(text);
        }
        if !self.memory.enabled || !self.memory.provider_consent {
            self.fail_job_if_current(job, JobFailureKind::ConsentWithdrawn, self.clock.now())?;
            return Err(MemoryServiceError::ConsentWithdrawn);
        }
        let job_id = job.job_id.clone();
        let now = timestamp(self.clock.now());
        let (stored, claimed) = self
            .store
            .update_job_with(job.kind, &job.period, |current| {
                let Some(mut current) = current else {
                    return (None, false);
                };
                let claimed = current.job_id == job_id && current.phase == MemoryJobPhase::Reserved;
                if claimed {
                    current.mark_calling(&now);
                }
                (Some(current), claimed)
            })?;
        if let Some(stored) = stored {
            *job = stored;
        }
        if !claimed {
            return job
                .generated_text
                .clone()
                .ok_or(MemoryServiceError::DeferredAfterFailure);
        }
        let debug_call_id = DebugStore::new_id();
        let system_prompt =
            "あなたは CooSenpAI の記憶を整理します。指定された JSON だけを返してください。";
        if let Some(store) = &self.debug_store {
            if let Err(error) = store.record_prompt(
                "memory",
                &debug_call_id,
                system_prompt,
                &prompt,
                self.clock.now(),
            ) {
                self.record_debug_error(error);
            }
        }
        let provider = self.provider.clone();
        let cancellation_must_complete = provider.cancellation_must_complete();
        let model = model_value(&self.companion.model);
        let effort = effort_value(&self.companion.effort);
        let timeout = Duration::from_millis(self.companion.timeout_ms);
        let system_prompt_for_call = system_prompt.to_owned();
        let provider_cancellation = cancellation.clone();
        let mut provider_call = Box::pin(provider.call(
            ProviderCall {
                system_prompt: system_prompt_for_call,
                prompt,
                images: Vec::new(),
                tools_disabled: true,
                output_schema: Some(memory_summary_schema()),
                session: SessionRequest::Ephemeral,
                model,
                effort,
                timeout,
                tutorial_response_key: None,
            },
            provider_cancellation,
        ));
        let result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                if cancellation_must_complete {
                    provider_call.await
                } else {
                    tokio::time::timeout(Duration::from_millis(100), &mut provider_call)
                        .await
                        .unwrap_or_else(|_| Err(ProviderError {
                            kind: crate::provider::ProviderErrorKind::Retryable,
                            message: "memory provider をキャンセルしました".to_owned(),
                        }))
                }
            },
            result = &mut provider_call => result,
        };
        let result = if cancellation.is_cancelled() {
            Err(ProviderError {
                kind: crate::provider::ProviderErrorKind::Retryable,
                message: "memory provider をキャンセルしました".to_owned(),
            })
        } else {
            result
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                if let Some(store) = &self.debug_store {
                    if let Err(debug_error) = store.record_provider_error(
                        "memory",
                        &debug_call_id,
                        &error.message,
                        self.clock.now(),
                    ) {
                        self.record_debug_error(debug_error);
                    }
                }
                if cancellation.is_cancelled() {
                    self.release_job_after_preemption(job)?;
                    return Err(MemoryServiceError::Preempted);
                }
                self.fail_job_if_current(job, JobFailureKind::Provider, self.clock.now())?;
                return Err(MemoryServiceError::Provider(error));
            }
        };
        if let (Some(store), Some(value)) = (&self.debug_store, result.value.as_ref()) {
            if let Err(error) =
                store.record_response("memory", &debug_call_id, value, self.clock.now())
            {
                self.record_debug_error(error);
            }
        }
        let text = result
            .value
            .as_ref()
            .and_then(|value| value.get("text"))
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.trim().is_empty() && text.len() <= SUMMARY_TEXT_MAX_BYTES)
            .map(str::to_owned);
        let Some(text) = text else {
            self.fail_job_if_current(job, JobFailureKind::InvalidOutput, self.clock.now())?;
            return Err(MemoryServiceError::InvalidOutput);
        };
        let now = timestamp(self.clock.now());
        let expected = job.job_id.clone();
        let (stored, changed) = self
            .store
            .update_job_with(job.kind, &job.period, |current| {
                let Some(mut current) = current else {
                    return (None, false);
                };
                let changed =
                    current.job_id == expected && current.phase == MemoryJobPhase::Calling;
                if changed {
                    current.mark_generated(text.clone(), &now);
                }
                (Some(current), changed)
            })?;
        if let Some(stored) = stored {
            *job = stored;
        }
        changed
            .then_some(text)
            .ok_or(MemoryServiceError::DeferredAfterFailure)
    }

    pub(super) fn reconcile_jobs(
        &self,
        local_date: chrono::NaiveDate,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryServiceError> {
        let day = local_date.to_string();
        let now = timestamp(now);
        for job in self.store.jobs()? {
            self.store.update_job(job.kind, &job.period, |current| {
                current.map(|mut current| {
                    current.recover_after_crash(&day, &now);
                    current
                })
            })?;
        }
        Ok(())
    }

    pub(super) fn fail_active_jobs_for_withdrawn_consent(
        &self,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryServiceError> {
        let now = timestamp(now);
        for job in self.store.jobs()? {
            self.store.update_job(job.kind, &job.period, |current| {
                current.map(|mut current| {
                    if matches!(
                        current.phase,
                        MemoryJobPhase::Reserved | MemoryJobPhase::Calling
                    ) {
                        current.fail(JobFailureKind::ConsentWithdrawn, &now);
                    }
                    current
                })
            })?;
        }
        Ok(())
    }

    pub(super) fn fail_job_if_current(
        &self,
        job: &mut MemoryJob,
        failure: JobFailureKind,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryServiceError> {
        let expected = job.job_id.clone();
        let now = timestamp(now);
        let stored = self.store.update_job(job.kind, &job.period, |current| {
            current.map(|mut current| {
                if current.job_id == expected
                    && !matches!(
                        current.phase,
                        MemoryJobPhase::Committed | MemoryJobPhase::Failed
                    )
                {
                    current.fail(failure, &now);
                }
                current
            })
        })?;
        if let Some(stored) = stored {
            *job = stored;
        }
        Ok(())
    }

    pub(super) fn release_job_after_preemption(
        &self,
        job: &mut MemoryJob,
    ) -> Result<(), MemoryServiceError> {
        let expected = job.job_id.clone();
        let now = timestamp(self.clock.now());
        let stored = self.store.update_job(job.kind, &job.period, |current| {
            current.map(|mut current| {
                if current.job_id == expected {
                    current.release_after_preemption(&now);
                }
                current
            })
        })?;
        if let Some(stored) = stored {
            *job = stored;
        }
        Ok(())
    }

    pub(super) fn commit_job(
        &self,
        job: &mut MemoryJob,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryServiceError> {
        let expected = job.job_id.clone();
        let now = timestamp(now);
        let (stored, committed) = self
            .store
            .update_job_with(job.kind, &job.period, |current| {
                let Some(mut current) = current else {
                    return (None, false);
                };
                let committed =
                    current.job_id == expected && current.phase == MemoryJobPhase::Generated;
                if committed {
                    current.mark_committed(&now);
                }
                (Some(current), committed)
            })?;
        if let Some(stored) = stored {
            *job = stored;
        }
        committed
            .then_some(())
            .ok_or(MemoryServiceError::DeferredAfterFailure)
    }
}
