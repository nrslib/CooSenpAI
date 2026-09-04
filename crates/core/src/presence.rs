use crate::config::{CompanionConfig, ConfigPaths};
use crate::persistence::{atomic_write_json, PersistenceError, SiblingLock};
use crate::state::{
    truncate_bytes, ActivityTriggerKind, ObservationConfidence, ObservationFrame,
    ObservationRecord, VisualObservation, VisualObservationData,
};
use chrono::{DateTime, NaiveDate, NaiveTime, SecondsFormat, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompanionPresenceState {
    pub schema_version: u8,
    pub local_date: String,
    #[serde(default)]
    pub greeting_done: bool,
    #[serde(default)]
    pub review_done: bool,
    #[serde(default)]
    pub reminder_ids: Vec<String>,
    #[serde(default, skip_serializing)]
    pub quiet_count: u32,
    #[serde(default, skip_serializing)]
    pub quiet_sequence: u64,
    #[serde(default, skip_serializing)]
    pub pending_quiet: Option<serde_json::Value>,
    #[serde(default)]
    pub fact_prompt_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_fact_candidate_id: Option<String>,
}

impl CompanionPresenceState {
    pub fn new(date: NaiveDate) -> Self {
        Self {
            schema_version: 1,
            local_date: date.to_string(),
            greeting_done: false,
            review_done: false,
            reminder_ids: Vec::new(),
            quiet_count: 0,
            quiet_sequence: 0,
            pending_quiet: None,
            fact_prompt_ids: Vec::new(),
            active_fact_candidate_id: None,
        }
    }

    pub fn roll_date(&mut self, date: NaiveDate) {
        if self.local_date == date.to_string() {
            return;
        }
        self.local_date = date.to_string();
        self.greeting_done = false;
        self.review_done = false;
        self.fact_prompt_ids.clear();
        self.active_fact_candidate_id = None;
    }

    pub fn next_scheduled(
        &mut self,
        now: NaiveTime,
        config: &CompanionConfig,
        startup: bool,
    ) -> Option<PresenceEvent> {
        if startup {
            let mut overdue = Vec::new();
            if !self.greeting_done {
                if (5..12).contains(&now.hour()) {
                    overdue.push(PresenceEvent::Greeting {
                        id: format!("presence-greeting-{}", self.local_date),
                    });
                } else {
                    self.greeting_done = true;
                }
            }
            overdue.extend(self.due_reminders(now, config));
            if let Some(review) = self.due_review(now, config) {
                overdue.push(review);
            }
            if let Some(event) = coalesce_startup_events(&self.local_date, overdue) {
                return Some(event);
            }
        }
        if let Some(reminder) = self.due_reminders(now, config).into_iter().next() {
            return Some(reminder);
        }
        if let Some(review) = self.due_review(now, config) {
            return Some(review);
        }
        None
    }

    pub fn mark_completed(&mut self, event: &PresenceEvent) {
        match event {
            PresenceEvent::Greeting { .. } => self.greeting_done = true,
            PresenceEvent::Review { .. } => self.review_done = true,
            PresenceEvent::Reminder { id, .. } => {
                if !self.reminder_ids.contains(id) {
                    self.reminder_ids.push(id.clone());
                }
            }
            PresenceEvent::CatchUp { events, .. } => {
                for event in events {
                    self.mark_completed(event);
                }
            }
        }
    }

    fn due_reminders(&self, now: NaiveTime, config: &CompanionConfig) -> Vec<PresenceEvent> {
        config
            .reminders
            .iter()
            .filter_map(|reminder| {
                let id = reminder_id(&self.local_date, &reminder.id);
                (parse_time(&reminder.time).is_some_and(|time| time <= now)
                    && !self.reminder_ids.contains(&id))
                .then(|| PresenceEvent::Reminder {
                    id,
                    theme: reminder.theme.clone(),
                })
            })
            .collect()
    }

    fn due_review(&self, now: NaiveTime, config: &CompanionConfig) -> Option<PresenceEvent> {
        (!self.review_done
            && !config.review_time.is_empty()
            && parse_time(&config.review_time).is_some_and(|time| time <= now))
        .then(|| PresenceEvent::Review {
            id: format!("presence-review-{}", self.local_date),
        })
    }

    pub fn select_fact_candidate(&mut self, ids: &[String], daily_limit: u32) -> Option<String> {
        if let Some(active) = &self.active_fact_candidate_id {
            return ids.contains(active).then(|| active.clone());
        }
        if daily_limit == 0 || self.fact_prompt_ids.len() >= daily_limit as usize {
            return None;
        }
        let id = ids
            .iter()
            .find(|id| !self.fact_prompt_ids.contains(id))?
            .clone();
        self.fact_prompt_ids.push(id.clone());
        self.active_fact_candidate_id = Some(id.clone());
        Some(id)
    }

    pub fn resolve_fact_candidate(&mut self, id: &str) {
        if self.active_fact_candidate_id.as_deref() == Some(id) {
            self.active_fact_candidate_id = None;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceEvent {
    Greeting {
        id: String,
    },
    Review {
        id: String,
    },
    Reminder {
        id: String,
        theme: String,
    },
    CatchUp {
        id: String,
        events: Vec<PresenceEvent>,
    },
}

impl PresenceEvent {
    pub fn id(&self) -> &str {
        match self {
            Self::Greeting { id }
            | Self::Review { id }
            | Self::Reminder { id, .. }
            | Self::CatchUp { id, .. } => id,
        }
    }

    pub fn observation(&self, context: &str, now: DateTime<Utc>) -> ObservationRecord {
        let (activity, target) = match self {
            Self::Greeting { .. } => ("起動時の挨拶", "presence:greeting"),
            Self::Review { .. } => ("今日のふりかえり", "presence:review"),
            Self::Reminder { .. } => ("予定した声かけ", "presence:reminder"),
            Self::CatchUp { .. } => ("期限を過ぎた声かけのまとめ", "presence:catch-up"),
        };
        let timestamp = now.to_rfc3339_opts(SecondsFormat::Millis, true);
        ObservationRecord::Visual(VisualObservation {
            kind: "visual".to_owned(),
            schema_version: 1,
            id: self.id().to_owned(),
            created_at: timestamp.clone(),
            window_start: timestamp.clone(),
            window_end: timestamp,
            frame_count: 1,
            frames: vec![ObservationFrame {
                trigger: ActivityTriggerKind::Timer,
                front_app: None,
                app: None,
                target: target.to_owned(),
            }],
            source_frame_ids: Vec::new(),
            data: VisualObservationData {
                activity: activity.to_owned(),
                outline: truncate_bytes(context, 2_000),
                changes: Vec::new(),
                events: Vec::new(),
                guess: Some(activity.to_owned()),
                confidence: Some(ObservationConfidence::High),
                wake_companion: true,
            },
        })
    }
}

#[derive(Debug, Clone)]
pub struct CompanionPresenceStore {
    path: std::path::PathBuf,
}

impl CompanionPresenceStore {
    pub fn new(paths: &ConfigPaths) -> Self {
        Self {
            path: paths.companion_presence.clone(),
        }
    }

    pub fn load(&self, date: NaiveDate) -> Result<CompanionPresenceState, PersistenceError> {
        let _lock = SiblingLock::acquire(&self.path.with_extension("json.lock"))?;
        self.load_unlocked(date)
    }

    pub fn update<T>(
        &self,
        date: NaiveDate,
        update: impl FnOnce(&mut CompanionPresenceState) -> T,
    ) -> Result<T, PersistenceError> {
        let _lock = SiblingLock::acquire(&self.path.with_extension("json.lock"))?;
        let mut state = self.load_unlocked(date)?;
        state.roll_date(date);
        let result = update(&mut state);
        atomic_write_json(&self.path, &state)?;
        Ok(result)
    }

    fn load_unlocked(&self, date: NaiveDate) -> Result<CompanionPresenceState, PersistenceError> {
        if !self.path.exists() {
            return Ok(CompanionPresenceState::new(date));
        }
        let bytes = fs::read(&self.path)?;
        let mut state = serde_json::from_slice::<CompanionPresenceState>(&bytes)
            .map_err(PersistenceError::Json)?;
        state.roll_date(date);
        Ok(state)
    }
}

fn parse_time(value: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(value, "%H:%M").ok()
}

fn reminder_id(date: &str, reminder_id: &str) -> String {
    format!("presence-reminder-{date}-{reminder_id}")
}

fn coalesce_startup_events(date: &str, mut events: Vec<PresenceEvent>) -> Option<PresenceEvent> {
    match events.len() {
        0 => None,
        1 => events.pop(),
        _ => {
            let mut digest = Sha256::new();
            digest.update(date.as_bytes());
            for event in &events {
                digest.update([0]);
                digest.update(event.id().as_bytes());
            }
            Some(PresenceEvent::CatchUp {
                id: format!("presence-catch-up-{date}-{:x}", digest.finalize()),
                events,
            })
        }
    }
}

