use crate::persistence::{atomic_write_json, PersistenceError, SiblingLock};
use crate::ports::ActivitySnapshot;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct StagnationTracker {
    last_meaningful_change: Instant,
    last_idle_ms: Option<u64>,
    activity_signals: bool,
    reported: bool,
    generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagnationCandidate {
    generation: u64,
    elapsed_ms: u64,
}

impl StagnationCandidate {
    pub fn elapsed_ms(self) -> u64 {
        self.elapsed_ms
    }
}

impl StagnationTracker {
    pub fn new(now: Instant, initial: Option<&ActivitySnapshot>) -> Self {
        Self {
            last_meaningful_change: now,
            last_idle_ms: initial.map(|activity| activity.idle_ms),
            activity_signals: false,
            reported: false,
            generation: 0,
        }
    }

    pub fn resume(
        now: Instant,
        elapsed: Duration,
        initial: Option<&ActivitySnapshot>,
        reported: bool,
    ) -> Self {
        Self {
            last_meaningful_change: now.checked_sub(elapsed).unwrap_or(now),
            last_idle_ms: initial.map(|activity| activity.idle_ms),
            activity_signals: false,
            reported,
            generation: 0,
        }
    }

    pub fn observe_activity(
        &mut self,
        activity: Option<&ActivitySnapshot>,
        active_threshold_ms: u64,
    ) -> bool {
        let Some(activity) = activity else {
            self.last_idle_ms = None;
            return false;
        };
        let fresh_activity = self
            .last_idle_ms
            .is_some_and(|previous| activity.idle_ms < previous);
        if activity.idle_ms < active_threshold_ms || fresh_activity {
            self.activity_signals = true;
        }
        self.last_idle_ms = Some(activity.idle_ms);
        fresh_activity
    }

    pub fn mark_meaningful_change(&mut self, now: Instant) {
        self.last_meaningful_change = now;
        self.activity_signals = false;
        self.reported = false;
        self.generation = self.generation.saturating_add(1);
    }

    pub fn prepare_stagnation(
        &self,
        now: Instant,
        stuck_after_ms: u64,
    ) -> Option<StagnationCandidate> {
        let elapsed = now
            .saturating_duration_since(self.last_meaningful_change)
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        if self.reported || !self.activity_signals || elapsed < stuck_after_ms {
            return None;
        }
        Some(StagnationCandidate {
            generation: self.generation,
            elapsed_ms: elapsed,
        })
    }

    pub fn commit_stagnation(&mut self, candidate: StagnationCandidate) -> bool {
        if self.reported || candidate.generation != self.generation {
            return false;
        }
        self.activity_signals = false;
        self.reported = true;
        true
    }

    pub fn mark_reported(&mut self) {
        self.activity_signals = false;
        self.reported = true;
    }

    pub fn is_reported(&self) -> bool {
        self.reported
    }

    pub fn sync_durable_episode(&mut self, now: Instant, elapsed: Duration, reported: bool) {
        self.last_meaningful_change = now.checked_sub(elapsed).unwrap_or(now);
        self.activity_signals = false;
        self.reported = reported;
        self.generation = self.generation.saturating_add(1);
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WatchStagnationState {
    #[serde(default = "schema_version")]
    schema_version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_meaningful_change_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reported_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_reported_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_report: Option<StagnationReportIntent>,
    #[serde(default)]
    fingerprints: BTreeMap<String, StagnationFingerprint>,
}

fn schema_version() -> u8 {
    1
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StagnationFingerprint {
    #[serde(default)]
    pub image_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_signature: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StagnationReportIntent {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub episode_started_at: String,
    #[serde(default)]
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagnationSnapshot {
    pub last_meaningful_change_at: DateTime<Utc>,
    pub reported: bool,
    pub pending_report: Option<StagnationReportIntent>,
    pub fingerprints: BTreeMap<String, StagnationFingerprint>,
}

impl StagnationSnapshot {
    pub fn elapsed(&self, now: DateTime<Utc>) -> Duration {
        now.signed_duration_since(self.last_meaningful_change_at)
            .to_std()
            .unwrap_or(Duration::ZERO)
    }
}

#[derive(Debug, Clone)]
pub struct WatchStagnationStore {
    path: PathBuf,
}

impl WatchStagnationStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self, now: DateTime<Utc>) -> Result<StagnationSnapshot, PersistenceError> {
        let _lock = SiblingLock::acquire(&self.path.with_extension("json.lock"))?;
        let state = self.load_unlocked()?;
        Ok(snapshot_from_state(state, now))
    }

    pub fn record_meaningful_change(
        &self,
        target: &str,
        fingerprint: StagnationFingerprint,
        changed_at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        let _lock = SiblingLock::acquire(&self.path.with_extension("json.lock"))?;
        let mut state = self.load_unlocked()?;
        if state.fingerprints.get(target) == Some(&fingerprint) {
            return Ok(false);
        }
        state.fingerprints.insert(target.to_owned(), fingerprint);
        state.last_meaningful_change_at = Some(changed_at.to_rfc3339());
        state.reported_at = None;
        state.pending_report = None;
        atomic_write_json(&self.path, &state)?;
        Ok(true)
    }

    pub fn prepare_report(
        &self,
        episode_started_at: DateTime<Utc>,
        elapsed_ms: u64,
        created_at: DateTime<Utc>,
    ) -> Result<Option<StagnationReportIntent>, PersistenceError> {
        let _lock = SiblingLock::acquire(&self.path.with_extension("json.lock"))?;
        let mut state = self.load_unlocked()?;
        let current = parse_timestamp(state.last_meaningful_change_at.as_deref());
        if current != Some(episode_started_at) || state.reported_at.is_some() {
            return Ok(None);
        }
        if let Some(intent) = &state.pending_report {
            if parse_timestamp(Some(&intent.episode_started_at)) == Some(episode_started_at) {
                return Ok(Some(intent.clone()));
            }
        }
        if parse_timestamp(state.last_reported_at.as_deref()).is_some_and(|last_reported_at| {
            created_at.signed_duration_since(last_reported_at) < chrono::Duration::minutes(15)
        }) {
            return Ok(None);
        }
        let Some(episode_started_at) = state.last_meaningful_change_at.clone() else {
            return Ok(None);
        };
        let intent = StagnationReportIntent {
            id: format!("stagnation-{}", Uuid::new_v4()),
            created_at: created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            // Keep the durable episode identity byte-for-byte. Reformatting to milliseconds
            // would truncate a capture timestamp and make mark_reported reject its own intent.
            episode_started_at,
            elapsed_ms,
        };
        state.pending_report = Some(intent.clone());
        atomic_write_json(&self.path, &state)?;
        Ok(Some(intent))
    }

    pub fn mark_reported(
        &self,
        intent: &StagnationReportIntent,
        reported_at: DateTime<Utc>,
    ) -> Result<bool, PersistenceError> {
        let _lock = SiblingLock::acquire(&self.path.with_extension("json.lock"))?;
        let mut state = self.load_unlocked()?;
        let episode_started_at = parse_timestamp(Some(&intent.episode_started_at));
        let current = parse_timestamp(state.last_meaningful_change_at.as_deref());
        if current != episode_started_at
            || state.reported_at.is_some()
            || state.pending_report.as_ref().map(|value| value.id.as_str())
                != Some(intent.id.as_str())
        {
            return Ok(false);
        }
        state.reported_at = Some(reported_at.to_rfc3339());
        state.last_reported_at = Some(reported_at.to_rfc3339());
        state.pending_report = None;
        atomic_write_json(&self.path, &state)?;
        Ok(true)
    }

    pub fn record_reaction(&self, reacted_at: DateTime<Utc>) -> Result<bool, PersistenceError> {
        let _lock = SiblingLock::acquire(&self.path.with_extension("json.lock"))?;
        let mut state = self.load_unlocked()?;
        if parse_timestamp(state.last_meaningful_change_at.as_deref())
            .is_some_and(|current| current >= reacted_at)
        {
            return Ok(false);
        }
        state.last_meaningful_change_at = Some(reacted_at.to_rfc3339());
        state.reported_at = None;
        state.pending_report = None;
        atomic_write_json(&self.path, &state)?;
        Ok(true)
    }

    fn load_unlocked(&self) -> Result<WatchStagnationState, PersistenceError> {
        if !self.path.exists() {
            return Ok(WatchStagnationState {
                schema_version: schema_version(),
                ..WatchStagnationState::default()
            });
        }
        let state = serde_json::from_slice::<WatchStagnationState>(&fs::read(&self.path)?)?;
        if state.schema_version != schema_version() {
            return Err(PersistenceError::Invalid(
                "watch-stagnation.json の schemaVersion が不正です".to_owned(),
            ));
        }
        if state.pending_report.as_ref().is_some_and(|intent| {
            intent.id.is_empty()
                || intent.elapsed_ms == 0
                || parse_timestamp(Some(&intent.created_at)).is_none()
                || parse_timestamp(Some(&intent.episode_started_at)).is_none()
        }) {
            return Err(PersistenceError::Invalid(
                "watch-stagnation.json の pendingReport が不正です".to_owned(),
            ));
        }
        Ok(state)
    }
}

fn parse_timestamp(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn snapshot_from_state(state: WatchStagnationState, now: DateTime<Utc>) -> StagnationSnapshot {
    let last_meaningful_change_at =
        parse_timestamp(state.last_meaningful_change_at.as_deref()).unwrap_or(now);
    let reported = state
        .reported_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|reported| reported.with_timezone(&Utc) >= last_meaningful_change_at)
        .unwrap_or(false);
    StagnationSnapshot {
        last_meaningful_change_at,
        reported,
        pending_report: state.pending_report,
        fingerprints: state.fingerprints,
    }
}

