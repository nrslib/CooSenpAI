use super::*;

impl DesktopState {
    pub(crate) fn spawn_runtime_monitor(state: Arc<Self>) {
        let mut snapshots = state.core_runtime().subscribe_snapshots();
        tauri::async_runtime::spawn(async move {
            let initial = snapshots.borrow_and_update().clone();
            state
                .publish_event(crate::snapshot_presenter::SnapshotEvent::RuntimeObserved {
                    runtime: initial,
                    initial: true,
                })
                .await;
            loop {
                tokio::select! {
                    _ = state.cancellation.cancelled() => break,
                    changed = snapshots.changed() => {
                        if changed.is_err() { break; }
                        let runtime = snapshots.borrow_and_update().clone();
                        state.publish_event(crate::snapshot_presenter::SnapshotEvent::RuntimeObserved { runtime, initial: false }).await;
                    }
                }
            }
        });
    }

    pub(crate) async fn clear_pending_thought_bubble(&self) {
        let _ = self
            .ui
            .request(
                crate::ui_events::UiView::Application,
                crate::ui_events::UiEvent::ThoughtClear {
                    conversation_switch: false,
                },
            )
            .await;
    }

    pub(crate) async fn clear_thought_bubble_for_conversation_switch(&self) {
        let _ = self
            .ui
            .request(
                crate::ui_events::UiView::Application,
                crate::ui_events::UiEvent::ThoughtClear {
                    conversation_switch: true,
                },
            )
            .await;
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
            let voice_record = pending.record.clone();
            let notification_id = pending.record.id.clone();
            let consumer_writer = consumer.clone();
            let result = tokio::task::spawn_blocking(
                move || -> Result<(bool, bool), coosenpai_core::notification::NotificationError> {
                    if !consumer_writer.is_current(&pending)? {
                        consumer_writer.skip(pending)?;
                        Ok((true, false))
                    } else if presented {
                        consumer_writer.accept(pending)?;
                        Ok((true, true))
                    } else {
                        consumer_writer.retry(pending)?;
                        Ok((false, false))
                    }
                },
            )
            .await;
            let Ok(Ok((clear_log, accepted))) = result else {
                self.clear_bubble_delivery_log(&notification_id).await;
                return;
            };
            if clear_log {
                self.clear_bubble_delivery_log(&notification_id).await;
            }
            if accepted && matches!(target, NotificationTarget::Bubble) {
                self.accept_voice_notification(&voice_record).await;
            }
        }
    }

    async fn present_notification(
        self: &Arc<Self>,
        record: &coosenpai_core::notification::NotificationRecord,
        target: NotificationTarget,
    ) -> bool {
        let (reply, response) = tokio::sync::oneshot::channel();
        self.ui.input(
            crate::ui_events::UiView::Application,
            crate::ui_events::UiEvent::NotificationRequested {
                record: record.clone(),
                target,
                context: Box::new(self.notification_context().await),
                reply,
            },
        );
        let accepted = response.await.unwrap_or(false);
        self.ui
            .query(crate::ui_events::UiView::Application, |reply| {
                crate::ui_events::UiEvent::Tutorial(Box::new(
                    crate::tutorial_events::TutorialEvent::Response(
                        crate::tutorial_response_presenter::ResponseEvent::NotificationCompleted {
                            id: record.id.clone(),
                            message: record.message.clone(),
                            chat: record.message_kind == "chat",
                            accepted,
                            reply,
                        },
                    ),
                ))
            })
            .await
            .unwrap_or(false)
    }

    pub(crate) async fn notification_context(
        &self,
    ) -> crate::bubbles::presenter::NotificationContext {
        let runtime = self.runtime_snapshot();
        crate::bubbles::presenter::NotificationContext {
            config: self.runtime_config(),
            display_name: runtime.companion_display_name,
            input_active: self.input_active.load(Ordering::Acquire),
            tutorial_active: self.tutorial.lock().await.state().tutorial_active(),
            latest_thought: runtime.latest_companion_thought,
            latest_thought_generation: runtime.latest_companion_thought_generation,
        }
    }

    pub(crate) async fn notification_generation_guard(
        &self,
        generation: u64,
    ) -> Option<tokio::sync::OwnedMutexGuard<()>> {
        let guard = self.conversation_sync.clone().lock_owned().await;
        (generation == self.bubbles.lock().await.conversation_generation()).then_some(guard)
    }

    async fn clear_bubble_delivery_log(&self, notification_id: &str) {
        self.ui.input(
            crate::ui_events::UiView::Application,
            crate::ui_events::UiEvent::NotificationFinished(notification_id.to_owned()),
        );
    }
}

// 監視が保持した snapshot は世代切替の途中で処理されうる。
// thought を生成した世代と現在世代が一致するときだけ表示対象にする。
