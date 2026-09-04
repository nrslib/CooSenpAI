use super::*;
use crate::bubbles::{self, BubbleAction, BubbleInteraction, BubbleRecord};
use crate::command_guard::{CommandSource, DesktopCommand, DispatchError};
use chrono::{DateTime, Local, LocalResult, NaiveDate, NaiveTime, Utc};
use coosenpai_core::config::MemoryConfig;
use coosenpai_core::memory::{FactStore, MemoryStore};
use coosenpai_core::presence::{CompanionPresenceStore, PresenceEvent};
use coosenpai_core::state::ConversationRole;
use std::sync::atomic::Ordering;

const PRESENCE_POLL_SECONDS: u64 = 15;

impl DesktopState {
    pub(crate) async fn set_temporary_assertiveness(
        self: &Arc<Self>,
        value: String,
    ) -> coosenpai_core::config::Config {
        let now = Local::now();
        let expires_at = temporary_assertiveness_deadline(now).with_timezone(&Utc);
        let temporary = self.factory.temporary_assertiveness();
        temporary.set(value, expires_at);
        let current = temporary.current(Utc::now());
        self.publish(|snapshot| snapshot.temporary_assertiveness = current)
            .await;

        let state = self.clone();
        let delay = expires_at
            .signed_duration_since(Utc::now())
            .to_std()
            .unwrap_or(Duration::ZERO);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(delay).await;
            if temporary.clear_if_expires_at(expires_at) {
                state
                    .publish(|snapshot| snapshot.temporary_assertiveness = None)
                    .await;
            }
        });
        self.runtime_config()
    }

    pub(super) fn spawn_presence_monitor(state: Arc<Self>) {
        tauri::async_runtime::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(PRESENCE_POLL_SECONDS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = state.cancellation.cancelled() => break,
                    _ = interval.tick() => {
                        if state.tutorial_is_active().await || state.tutorial_needs_setup().await {
                            state.presence_startup_pending.store(false, Ordering::Release);
                            continue;
                        }
                        if !state.is_runtime_active() {
                            continue;
                        }
                        let startup = state.presence_startup_pending.swap(false, Ordering::AcqRel);
                        let handler_state = state.clone();
                        let _ = state.dispatch(
                            CommandSource::RuntimeMonitor,
                            DesktopCommand::CompanionPresence,
                            move |_context| async move {
                                handler_state.run_presence_tick(startup).await
                                    .map_err(DispatchError::handler)
                            },
                        ).await;
                    }
                }
            }
        });
    }

    async fn run_presence_tick(self: &Arc<Self>, startup: bool) -> anyhow::Result<()> {
        let now = Local::now();
        let config = self.runtime_config();
        let store = CompanionPresenceStore::new(&self.paths);
        let event = store.update(now.date_naive(), |state| {
            state.next_scheduled(now.time(), &config.companion, startup)
        })?;
        if let Some(event) = event {
            let mut inflight = self.presence_inflight.lock().await;
            if inflight.is_none() {
                *inflight = Some(event.id().to_owned());
                drop(inflight);
                let result = self.run_presence_event(&event).await;
                *self.presence_inflight.lock().await = None;
                if let Err(error) = result {
                    if startup {
                        self.presence_startup_pending.store(true, Ordering::Release);
                    }
                    return Err(error);
                }
                store.update(now.date_naive(), |state| {
                    state.mark_completed(&event);
                })?;
                self.refresh_conversation().await;
            }
        }
        self.present_fact_candidate(now.date_naive()).await?;
        Ok(())
    }

    async fn run_presence_event(&self, event: &PresenceEvent) -> anyhow::Result<()> {
        let context = self.presence_context(event)?;
        let observation = event.observation(&context, Utc::now());
        self.core_runtime()
            .companion_nudge(observation, context, self.cancellation.child_token())
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    fn presence_context(&self, event: &PresenceEvent) -> anyhow::Result<String> {
        let now = Local::now();
        let date = now.date_naive();
        let recent_memory = self.latest_current_memory(date)?;
        Ok(match event {
            PresenceEvent::Greeting { .. } => format!(
                "この日最初の起動です。この予定された発話は省略せず emit=true、notificationPriority=info にし、記憶があれば踏まえて自然な短い挨拶をしてください。\n直近の記憶: {}",
                recent_memory.as_deref().unwrap_or("なし")
            ),
            PresenceEvent::Review { .. } => {
                let summary = self
                    .current_memory_for(date)?
                    .unwrap_or_else(|| self.today_conversation_context(date));
                format!("今日のふりかえりです。この予定された発話は省略せず emit=true、notificationPriority=info にし、今日やったことと残っていることを普段の口調で簡潔に伝えてください。\n今日の材料:\n{summary}")
            }
            PresenceEvent::Reminder { theme, .. } => format!(
                "設定された時刻の声かけです。この予定された発話は省略せず emit=true、notificationPriority=info にし、テーマを踏まえて普段の口調で短く声をかけてください。\nテーマ: {theme}"
            ),
            PresenceEvent::CatchUp { events, .. } => {
                let materials = events
                    .iter()
                    .map(|event| self.presence_context(event))
                    .collect::<anyhow::Result<Vec<_>>>()?
                    .join("\n\n");
                format!("起動前に期限を過ぎた項目を一度にまとめます。次の項目をすべて踏まえ、重複のない一つの自然な発話として emit=true、notificationPriority=info で伝えてください。\n\n{materials}")
            }
        })
    }

    fn latest_current_memory(&self, today: NaiveDate) -> anyhow::Result<Option<String>> {
        let config = self.runtime_config();
        if !presence_memory_allowed(&config.memory) {
            return Ok(None);
        }
        let store = MemoryStore::new(self.paths.clone());
        let mut periods = store
            .daily_summaries()?
            .into_iter()
            .filter_map(|summary| {
                NaiveDate::parse_from_str(&summary.local_date, "%Y-%m-%d")
                    .is_ok_and(|period| period <= today)
                    .then_some(summary.local_date)
            })
            .collect::<Vec<_>>();
        periods.sort();
        periods.dedup();
        for period in periods.into_iter().rev() {
            if let Some(summary) =
                store.load_current_daily(&period, config.memory.source_max_bytes)?
            {
                return Ok(Some(summary.text));
            }
        }
        Ok(None)
    }

    fn current_memory_for(&self, date: NaiveDate) -> anyhow::Result<Option<String>> {
        let config = self.runtime_config();
        if !presence_memory_allowed(&config.memory) {
            return Ok(None);
        }
        Ok(MemoryStore::new(self.paths.clone())
            .load_current_daily(&date.to_string(), config.memory.source_max_bytes)?
            .map(|summary| summary.text))
    }

    fn today_conversation_context(&self, date: NaiveDate) -> String {
        CompanionStorage::from_paths(
            &self.paths,
            self.runtime_config().retention.conversation_days,
        )
        .load_conversation()
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| created_at_is_local_date(&entry.created_at, date))
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|entry| {
            let role = match entry.role {
                ConversationRole::User => "ユーザー",
                ConversationRole::Companion => "companion",
            };
            format!("{role}: {}", truncate(&entry.message, 600))
        })
        .collect::<Vec<_>>()
        .join("\n")
    }

    async fn present_fact_candidate(
        self: &Arc<Self>,
        date: chrono::NaiveDate,
    ) -> anyhow::Result<()> {
        let config = self.runtime_config();
        if !config.memory.enabled || config.memory.fact_prompt_daily_limit == 0 {
            return Ok(());
        }
        let candidates = FactStore::new(self.paths.clone()).load_candidates()?;
        let ids = candidates
            .candidates
            .iter()
            .map(|value| value.id.clone())
            .collect::<Vec<_>>();
        let store = CompanionPresenceStore::new(&self.paths);
        let selected = store.update(date, |state| {
            state.select_fact_candidate(&ids, config.memory.fact_prompt_daily_limit)
        })?;
        let Some(id) = selected else {
            return Ok(());
        };
        let Some(candidate) = candidates.candidates.iter().find(|value| value.id == id) else {
            store.update(date, |state| state.resolve_fact_candidate(&id))?;
            return Ok(());
        };
        let generation = self.bubbles.lock().await.conversation_generation();
        bubbles::show_best_effort(
            self.clone(),
            BubbleRecord {
                id: format!("fact-prompt-{id}"),
                created_at: Utc::now().to_rfc3339(),
                message: format!("これ、覚えておく？\n{}", candidate.text),
                message_kind: "fact-confirmation".to_owned(),
                notification_priority: "info".to_owned(),
                caused_by: None,
                display_name: config.companion.display_name,
                persona: config.companion.persona,
                avatar_color: config.ui.avatar_color,
                conversation_generation: generation,
                persistent: true,
                open_url: None,
                interaction: Some(BubbleInteraction {
                    select: None,
                    actions: vec![
                        BubbleAction {
                            id: "memory-confirm".to_owned(),
                            label: "はい".to_owned(),
                        },
                        BubbleAction {
                            id: "memory-reject".to_owned(),
                            label: "いいえ".to_owned(),
                        },
                    ],
                    detail: Some(id),
                    technical_detail: None,
                }),
            },
            config.notification.bubble_duration_ms,
        )
        .await;
        Ok(())
    }

    pub(super) async fn resolve_fact_prompt(
        self: &Arc<Self>,
        bubble_id: &str,
        confirm: bool,
    ) -> Result<(), ConfigCommitError> {
        let date = Local::now().date_naive();
        let store = CompanionPresenceStore::new(&self.paths);
        let active_candidate_id = store
            .load(date)
            .map_err(commit_error)?
            .active_fact_candidate_id
            .ok_or_else(|| {
                ConfigCommitError::Runtime(RuntimeError::Factory(
                    "確認する候補がありません".to_owned(),
                ))
            })?;
        let candidate_id =
            fact_candidate_for_bubble(&active_candidate_id, bubble_id).ok_or_else(|| {
                ConfigCommitError::Runtime(RuntimeError::Factory(
                    "この候補の確認操作は期限切れです".to_owned(),
                ))
            })?;
        let expected_bubble_id = format!("fact-prompt-{candidate_id}");
        let facts = FactStore::new(self.paths.clone());
        if confirm {
            facts
                .confirm(
                    &candidate_id,
                    &expected_bubble_id,
                    &Utc::now().to_rfc3339(),
                    &self.runtime_config().memory,
                )
                .map_err(commit_error)?;
        } else {
            facts.reject(&candidate_id).map_err(commit_error)?;
        }
        store
            .update(date, |state| state.resolve_fact_candidate(&candidate_id))
            .map_err(commit_error)?;
        bubbles::dismiss(self, &expected_bubble_id).await;
        Ok(())
    }

    pub(super) async fn sync_resolved_fact_prompt(
        &self,
        candidate_id: &str,
    ) -> Result<(), ConfigCommitError> {
        CompanionPresenceStore::new(&self.paths)
            .update(Local::now().date_naive(), |state| {
                state.resolve_fact_candidate(candidate_id)
            })
            .map_err(commit_error)?;
        bubbles::dismiss(self, &format!("fact-prompt-{candidate_id}")).await;
        Ok(())
    }
}

fn temporary_assertiveness_deadline(now: DateTime<Local>) -> DateTime<Local> {
    let two_hours = now + chrono::Duration::hours(2);
    let eight = NaiveTime::from_hms_opt(8, 0, 0).unwrap_or(NaiveTime::MIN);
    let morning_date = if now.time() < eight {
        now.date_naive()
    } else {
        now.date_naive().succ_opt().unwrap_or(now.date_naive())
    };
    let morning = morning_date.and_time(eight).and_local_timezone(Local);
    let morning = match morning {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(first, second) => first.min(second),
        LocalResult::None => two_hours,
    };
    two_hours.min(morning)
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn created_at_is_local_date(created_at: &str, date: NaiveDate) -> bool {
    DateTime::parse_from_rfc3339(created_at)
        .is_ok_and(|timestamp| timestamp.with_timezone(&Local).date_naive() == date)
}

fn fact_candidate_for_bubble(active_candidate_id: &str, bubble_id: &str) -> Option<String> {
    (bubble_id == format!("fact-prompt-{active_candidate_id}"))
        .then(|| active_candidate_id.to_owned())
}

fn presence_memory_allowed(config: &MemoryConfig) -> bool {
    config.enabled && config.provider_consent
}

fn commit_error(error: impl ToString) -> ConfigCommitError {
    ConfigCommitError::Runtime(RuntimeError::Factory(error.to_string()))
}

