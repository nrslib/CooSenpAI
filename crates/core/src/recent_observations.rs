use crate::persistence::{JsonlStore, PersistenceError};
use crate::state::{parse_observation, ObservationRecord, DEFAULT_OBSERVATION_LIMITS};
use chrono::{DateTime, Duration, Local, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

const RECENT_WINDOW_MINUTES: i64 = 10;
const RECENT_LIMIT: usize = 10;
const RECENT_MAX_BYTES: usize = 16 * 1024;

pub fn read_recent_observations(
    directory: &Path,
    now: DateTime<Utc>,
) -> Result<Vec<ObservationRecord>, PersistenceError> {
    let cutoff = now - Duration::minutes(RECENT_WINDOW_MINUTES);
    let oldest_date = now.with_timezone(&Local).date_naive() - Duration::days(1);
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut by_id = HashMap::<String, (DateTime<Utc>, ObservationRecord)>::new();
    for entry in entries {
        let path = entry?.path();
        let Some(date) = path
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
        else {
            continue;
        };
        if path.extension().and_then(|value| value.to_str()) != Some("jsonl") || date < oldest_date
        {
            continue;
        }
        for value in JsonlStore::new(path).read::<Value>()? {
            let Ok(observation) = parse_observation(value, DEFAULT_OBSERVATION_LIMITS) else {
                continue;
            };
            let Ok(created_at) = DateTime::parse_from_rfc3339(observation.created_at()) else {
                continue;
            };
            let created_at = created_at.with_timezone(&Utc);
            if created_at < cutoff || created_at > now {
                continue;
            }
            let id = observation.id().to_owned();
            if by_id
                .get(&id)
                .is_none_or(|(existing, _)| created_at > *existing)
            {
                by_id.insert(id, (created_at, observation));
            }
        }
    }
    Ok(bounded_sorted(by_id))
}

pub fn merge_recent_observations(
    supplied: Vec<ObservationRecord>,
    stored: Vec<ObservationRecord>,
) -> Vec<ObservationRecord> {
    let mut by_id = HashMap::<String, (DateTime<Utc>, ObservationRecord)>::new();
    for observation in stored.into_iter().chain(supplied) {
        let Ok(created_at) = DateTime::parse_from_rfc3339(observation.created_at()) else {
            continue;
        };
        let created_at = created_at.with_timezone(&Utc);
        let id = observation.id().to_owned();
        if by_id
            .get(&id)
            .is_none_or(|(existing, _)| created_at > *existing)
        {
            by_id.insert(id, (created_at, observation));
        }
    }
    bounded_sorted(by_id)
}

fn bounded_sorted(
    by_id: HashMap<String, (DateTime<Utc>, ObservationRecord)>,
) -> Vec<ObservationRecord> {
    let mut observations = by_id.into_values().collect::<Vec<_>>();
    observations.sort_by_key(|(created_at, _)| *created_at);
    if observations.len() > RECENT_LIMIT {
        observations.drain(..observations.len() - RECENT_LIMIT);
    }
    let mut retained = Vec::new();
    for (_, observation) in observations.into_iter().rev() {
        let values = std::iter::once(&observation)
            .chain(retained.iter().rev())
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>();
        let Ok(values) = values else {
            continue;
        };
        if crate::prompts::compact_observation_injection(values.iter()).len() <= RECENT_MAX_BYTES {
            retained.push(observation);
        }
    }
    retained.reverse();
    retained
}

