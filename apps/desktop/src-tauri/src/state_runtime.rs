use super::*;

impl DesktopState {
    pub(crate) fn spawn_runtime_monitor(state: Arc<Self>) {
        let mut snapshots = state.core_runtime().subscribe_snapshots();
        tauri::async_runtime::spawn(async move {
            let initial = snapshots.borrow_and_update().clone();
            let mut previous_thought = initial.latest_companion_thought.clone();
            state
                .publish(|snapshot| snapshot.apply_runtime(&initial))
                .await;
            crate::windows::sync_persona(&state.app, &state.runtime_config().companion.persona);
            state.refresh_conversation().await;
            state.present_pending_tutorial_response().await;
            loop {
                tokio::select! {
                    _ = state.cancellation.cancelled() => break,
                    changed = snapshots.changed() => {
                        if changed.is_err() { break; }
                        let runtime = snapshots.borrow_and_update().clone();
                        state.publish(|snapshot| snapshot.apply_runtime(&runtime)).await;
                        state
                            .present_updated_thought(previous_thought.as_deref(), &runtime)
                            .await;
                        previous_thought = runtime.latest_companion_thought.clone();
                        crate::windows::sync_persona(
                            &state.app,
                            &state.runtime_config().companion.persona,
                        );
                        state.refresh_conversation().await;
                        state.present_pending_tutorial_response().await;
                    }
                }
            }
        });
    }

    async fn present_updated_thought(
        self: &Arc<Self>,
        previous: Option<&str>,
        runtime: &RuntimeSnapshot,
    ) {
        let Some(thought) = runtime
            .latest_companion_thought
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return;
        };
        if previous == Some(thought) {
            return;
        }
        if !should_show_thought_bubble(
            self.runtime_config().ui.thought_bubble,
            self.input_active.load(Ordering::Acquire),
            crate::windows::main_is_focused(&self.app),
        ) {
            self.clear_pending_thought_bubble().await;
            return;
        }
        let action = self
            .thought_bubble
            .lock()
            .await
            .queue(Instant::now(), thought.to_owned());
        match action {
            ThoughtBubbleAction::Show(thought) => self.show_thought_bubble(thought).await,
            ThoughtBubbleAction::Wait(delay) => self.schedule_thought_flush(delay),
            ThoughtBubbleAction::None => {}
        }
    }

    async fn show_thought_bubble(self: &Arc<Self>, thought: String) {
        let runtime = self.runtime_snapshot();
        if runtime.latest_companion_thought.as_deref() != Some(thought.as_str())
            || !should_show_thought_bubble(
                self.runtime_config().ui.thought_bubble,
                self.input_active.load(Ordering::Acquire),
                crate::windows::main_is_focused(&self.app),
            )
        {
            return;
        }
        let config = self.runtime_config();
        let conversation_generation = self.bubbles.lock().await.conversation_generation();
        let record = BubbleRecord {
            id: "thought-bubble".to_owned(),
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message: thought,
            message_kind: "thought".to_owned(),
            notification_priority: "none".to_owned(),
            caused_by: None,
            display_name: runtime.companion_display_name,
            persona: config.companion.persona,
            avatar_color: config.ui.avatar_color,
            conversation_generation,
            persistent: false,
            open_url: None,
            interaction: None,
        };
        let _ =
            bubbles::show_best_effort(self.clone(), record, config.notification.bubble_duration_ms)
                .await;
    }

    fn schedule_thought_flush(self: &Arc<Self>, delay: Duration) {
        let state = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::select! {
                _ = state.cancellation.cancelled() => {}
                _ = tokio::time::sleep(delay) => state.flush_pending_thought().await,
            }
        });
    }

    async fn flush_pending_thought(self: &Arc<Self>) {
        let action = self.thought_bubble.lock().await.flush(Instant::now());
        match action {
            ThoughtBubbleAction::Show(thought) => self.show_thought_bubble(thought).await,
            ThoughtBubbleAction::Wait(delay) => self.schedule_thought_flush(delay),
            ThoughtBubbleAction::None => {}
        }
    }

    pub(crate) async fn clear_pending_thought_bubble(&self) {
        self.thought_bubble.lock().await.clear_pending();
    }

    pub(crate) fn spawn_own_bounds_monitor(state: Arc<Self>) {
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = state.cancellation.cancelled() => break,
                    _ = interval.tick() => {
                        if let Err(error) = state.own_bounds.request_refresh() {
                            let _ = state.logger.write(
                                "WARN",
                                &format!("自ウィンドウ bounds の更新に失敗しました: error-type=window-bounds ({error})"),
                            );
                        }
                    }
                }
            }
        });
    }

    pub(crate) fn spawn_notification_monitor(state: Arc<Self>) {
        tauri::async_runtime::spawn(async move {
            while !state.runtime_active.load(Ordering::Acquire) {
                tokio::select! {
                    _ = state.cancellation.cancelled() => return,
                    _ = tokio::time::sleep(Duration::from_millis(400)) => {}
                }
            }
            let mut app_consumer = match NotificationConsumer::new(
                state.paths.mailbox.clone(),
                "app",
                state.paths.notification_processed.clone(),
                "info",
            ) {
                Ok(value) => value.with_logger(state.logger.clone()),
                Err(_) => return,
            };
            let mut notify_consumer = match NotificationConsumer::new(
                state.paths.mailbox.clone(),
                "notify",
                state.paths.notification_processed.clone(),
                "info",
            ) {
                Ok(value) => value.with_logger(state.logger.clone()),
                Err(_) => return,
            };
            loop {
                tokio::select! {
                    _ = state.cancellation.cancelled() => break,
                    _ = tokio::time::sleep(Duration::from_millis(400)) => {
                        let priority = state.runtime.config().notification.min_priority;
                        if app_consumer.update_minimum_priority(priority.clone()).is_err()
                            || notify_consumer.update_minimum_priority(priority).is_err()
                        {
                            continue;
                        }
                        state.consume_notification(&app_consumer, NotificationTarget::Bubble).await;
                        state.consume_notification(&notify_consumer, NotificationTarget::Os).await;
                    }
                }
            }
        });
    }

    async fn consume_notification(
        self: &Arc<Self>,
        consumer: &NotificationConsumer,
        target: NotificationTarget,
    ) {
        loop {
            let consumer_reader = consumer.clone();
            let pending = tokio::task::spawn_blocking(move || consumer_reader.claim_next()).await;
            let Ok(Ok(Some(pending))) = pending else {
                return;
            };
            let current = consumer.is_current(&pending).unwrap_or(false);
            if !current {
                let notification_id = pending.record.id.clone();
                let consumer_writer = consumer.clone();
                let _ = tokio::task::spawn_blocking(move || consumer_writer.skip(pending)).await;
                self.clear_bubble_delivery_log(&notification_id).await;
                continue;
            }
            let presented = self.present_notification(&pending.record, target).await;
            let notification_id = pending.record.id.clone();
            let consumer_writer = consumer.clone();
            let result = tokio::task::spawn_blocking(
                move || -> Result<bool, coosenpai_core::notification::NotificationError> {
                    if !consumer_writer.is_current(&pending)? {
                        consumer_writer.skip(pending)?;
                        Ok(true)
                    } else if presented {
                        consumer_writer.accept(pending)?;
                        Ok(true)
                    } else {
                        consumer_writer.retry(pending)?;
                        Ok(false)
                    }
                },
            )
            .await;
            let Ok(Ok(clear_log)) = result else {
                self.clear_bubble_delivery_log(&notification_id).await;
                return;
            };
            if clear_log {
                self.clear_bubble_delivery_log(&notification_id).await;
            }
        }
    }

    async fn present_notification(
        self: &Arc<Self>,
        record: &coosenpai_core::notification::NotificationRecord,
        target: NotificationTarget,
    ) -> bool {
        let runtime_config = self.runtime.config();
        let config = runtime_config.notification;
        let signed = signed_build();
        let main_visible = crate::windows::main_is_visible(&self.app);
        let main_focused = crate::windows::main_is_focused(&self.app);
        let accepted = match target {
            NotificationTarget::Bubble
                if record.message_kind == "chat"
                    || matches!(config.mode.as_str(), "bubble" | "both")
                    || !signed =>
            {
                let tutorial_active = self.tutorial.lock().await.state().tutorial_active();
                let input_active = self.input_active.load(Ordering::Acquire);
                let decision =
                    bubble_delivery_decision(&record.message_kind, input_active, main_focused);
                let log_key = BubbleDeliveryLogKey {
                    decision,
                    main_focused,
                    input_active,
                };
                if self.should_log_bubble_delivery(&record.id, log_key).await {
                    let _ = self.logger.write(
                        "INFO",
                        &bubble_delivery_log(
                            &record.message_kind,
                            decision,
                            main_focused,
                            input_active,
                        ),
                    );
                }
                match decision {
                    BubbleDeliveryDecision::SuppressUnread => {
                        self.publish(|snapshot| snapshot.unread_count += 1).await;
                        true
                    }
                    BubbleDeliveryDecision::SuppressRead => true,
                    BubbleDeliveryDecision::Show => {
                        let presentation = bubble_presentation_style(
                            &record.message_kind,
                            tutorial_active,
                            runtime_config.bubble.keep_latest,
                        );
                        bubbles::show(
                            self.clone(),
                            BubbleRecord {
                                id: record.id.clone(),
                                created_at: record.created_at.clone(),
                                message: record.message.clone(),
                                message_kind: if presentation.tutorial {
                                    "tutorial".to_owned()
                                } else {
                                    record.message_kind.clone()
                                },
                                notification_priority: record.priority.clone(),
                                caused_by: record.caused_by.clone(),
                                display_name: self.runtime.snapshot().companion_display_name,
                                persona: runtime_config.companion.persona,
                                avatar_color: runtime_config.ui.avatar_color,
                                conversation_generation: record.conversation_generation,
                                persistent: presentation.persistent,
                                open_url: None,
                                interaction: None,
                            },
                            config.bubble_duration_ms,
                        )
                        .await
                        .is_ok_and(|outcome| {
                            outcome == bubbles::BubblePresentationOutcome::Acknowledged
                        })
                    }
                }
            }
            NotificationTarget::Os if main_visible || record.message_kind == "chat" => true,
            NotificationTarget::Os if signed && matches!(config.mode.as_str(), "os" | "both") => {
                let notifier = crate::platform::MacNotifier::new(
                    self.runtime.snapshot().companion_display_name,
                );
                notifier
                    .show(&record.message, &record.priority, Duration::from_secs(5))
                    .await
                    .is_ok()
            }
            _ => true,
        };
        if accepted {
            self.refresh_conversation().await;
            if record.message_kind == "chat" {
                self.clone()
                    .tutorial_response_presented(record.id.clone(), record.message.clone())
                    .await;
            }
        }
        accepted
    }

    async fn should_log_bubble_delivery(
        &self,
        notification_id: &str,
        key: BubbleDeliveryLogKey,
    ) -> bool {
        self.bubble_delivery_log_state
            .lock()
            .await
            .should_log(notification_id, key)
    }

    async fn clear_bubble_delivery_log(&self, notification_id: &str) {
        self.bubble_delivery_log_state
            .lock()
            .await
            .clear(notification_id);
    }
}
