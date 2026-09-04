use super::*;
use std::time::Instant;

impl CompanionAgent {
    pub(super) async fn call_provider(
        &mut self,
        turn: ProviderTurn<'_>,
        cancellation: CancellationToken,
    ) -> Result<ProviderCallOutcome, CompanionError> {
        let ProviderTurn {
            data,
            user,
            image_paths,
            events,
            source_ids,
            additional_inputs,
            tutorial_response_key,
        } = turn;
        let prompt = build_companion_prompt(data);
        let request = self
            .session
            .clone()
            .map_or(SessionRequest::New, SessionRequest::Resume);
        if additional_inputs.is_some() {
            let resumed = matches!(request, SessionRequest::Resume(_));
            let result = self
                .invoke_provider(
                    ProviderInvocation {
                        prompt: &prompt,
                        source_ids,
                        user,
                        image_paths,
                        session: request,
                        events,
                        additional_inputs,
                        tutorial_response_key,
                    },
                    cancellation,
                )
                .await;
            if result.is_err() && resumed {
                self.discard_provider_session();
            }
            return result;
        }
        match self
            .invoke_provider(
                ProviderInvocation {
                    prompt: &prompt,
                    source_ids,
                    user,
                    image_paths,
                    session: request.clone(),
                    events: events.clone(),
                    additional_inputs: None,
                    tutorial_response_key,
                },
                cancellation.clone(),
            )
            .await
        {
            Ok(response) => Ok(response),
            Err(_error)
                if matches!(request, SessionRequest::Resume(_)) && !cancellation.is_cancelled() =>
            {
                if let Some(events) = &events {
                    events.reset();
                }
                self.prepare_new_session(cancellation.clone(), user).await?;
                let mut fallback_data = data.clone();
                self.apply_session_context(&mut fallback_data, user, source_ids)?;
                let fallback_prompt = build_companion_prompt(&fallback_data);
                self.invoke_provider(
                    ProviderInvocation {
                        prompt: &fallback_prompt,
                        source_ids,
                        user,
                        image_paths,
                        session: SessionRequest::New,
                        events,
                        additional_inputs: None,
                        tutorial_response_key,
                    },
                    cancellation,
                )
                .await
            }
            Err(error) => Err(error),
        }
    }

    async fn invoke_provider(
        &mut self,
        invocation: ProviderInvocation<'_>,
        cancellation: CancellationToken,
    ) -> Result<ProviderCallOutcome, CompanionError> {
        let ProviderInvocation {
            prompt,
            source_ids,
            user,
            image_paths,
            session,
            events,
            additional_inputs,
            tutorial_response_key,
        } = invocation;
        let kind = if user {
            CompanionCallKind::User
        } else {
            CompanionCallKind::Proactive
        };
        self.record_call_attempt(kind, user)?;
        let mode = session_mode(&session);
        let started = Instant::now();
        self.log_call_start(mode)?;
        let debug_call_id = DebugStore::new_id();
        let system_prompt = self.system_prompt();
        if let Some(store) = &self.debug_store {
            if store
                .record_prompt(
                    "companion",
                    &debug_call_id,
                    &system_prompt,
                    prompt,
                    self.clock.now(),
                )
                .is_err()
            {
                self.log_debug_failure();
            }
        }
        let call = self.provider_call(prompt, image_paths, session.clone(), tutorial_response_key);
        let result_cancellation = cancellation.clone();
        let measured_events = Arc::new(MeasuredProviderEvents::new(events));
        let event_sink: Arc<dyn ProviderEventSink> = measured_events.clone();
        let provider = self.provider.clone();
        let cancellation_must_complete = provider.cancellation_must_complete();
        let result = match additional_inputs {
            Some(additional_inputs) => {
                let provider_cancellation = cancellation.clone();
                let mut provider_call = Box::pin(provider.call_streaming_with_mid_turn(
                    call,
                    provider_cancellation,
                    event_sink,
                    additional_inputs,
                ));
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        if cancellation_must_complete {
                            provider_call.await
                        } else {
                            tokio::time::timeout(Duration::from_millis(100), &mut provider_call)
                                .await
                                .unwrap_or_else(|_| Err(cancelled_provider_error()))
                        }
                    },
                    result = &mut provider_call => result,
                }
            }
            None => {
                let provider_cancellation = cancellation.clone();
                let mut provider_call =
                    Box::pin(provider.call_streaming(call, provider_cancellation, event_sink));
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        if cancellation_must_complete {
                            provider_call.await
                        } else {
                            tokio::time::timeout(Duration::from_millis(100), &mut provider_call)
                                .await
                                .unwrap_or_else(|_| Err(cancelled_provider_error()))
                        }
                    },
                    result = &mut provider_call => result,
                }
            }
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.log_call_failure(mode, error.kind, Some(&error.message));
                if cancellation.is_cancelled() {
                    self.discard_provider_session();
                }
                if let Some(store) = &self.debug_store {
                    if store
                        .record_provider_error(
                            "companion",
                            &debug_call_id,
                            &error.message,
                            self.clock.now(),
                        )
                        .is_err()
                    {
                        self.log_debug_failure();
                    }
                }
                return Err(error.into());
            }
        };
        if result_cancellation.is_cancelled() {
            self.discard_provider_session();
            return Err(CompanionError::Cancelled);
        }
        self.log_call_end(mode, started.elapsed().as_millis())?;
        if let (Some(store), Some(value)) = (&self.debug_store, result.value.as_ref()) {
            if store
                .record_companion_call(
                    &debug_call_id,
                    source_ids.to_vec(),
                    prompt,
                    value,
                    self.clock.now(),
                )
                .is_err()
            {
                self.log_debug_failure();
            }
        }
        let response = match parse_response(&result) {
            Ok(response) => response,
            Err(error) => {
                self.log_call_failure(mode, ProviderErrorKind::InvalidOutput, None);
                return Err(error);
            }
        };
        if let Err(error) = self.accept_session(&session, result.session) {
            self.log_session_rejection(mode, &error);
            return Err(error);
        }
        Ok(ProviderCallOutcome {
            response,
            usage: measured_events.measured_usage(),
        })
    }

    pub(super) fn provider_call(
        &self,
        prompt: &str,
        image_paths: &[std::path::PathBuf],
        session: SessionRequest,
        tutorial_response_key: Option<&str>,
    ) -> ProviderCall {
        ProviderCall {
            system_prompt: self.system_prompt(),
            prompt: prompt.to_owned(),
            images: image_paths.iter().cloned().map(Into::into).collect(),
            tools_disabled: true,
            output_schema: Some(companion_schema()),
            session,
            model: Some(self.config.model.clone()),
            effort: Some(self.config.effort.clone()),
            timeout: Duration::from_millis(self.config.timeout_ms),
            tutorial_response_key: tutorial_response_key.map(str::to_owned),
        }
    }
}

fn cancelled_provider_error() -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::Retryable,
        message: "companion provider をキャンセルしました".to_owned(),
    }
}
