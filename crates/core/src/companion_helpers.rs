use crate::state::{ObservationEventType, ObservationRecord};
use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub(super) fn ordered_observation_image_paths(
    observations: &[ObservationRecord],
    observation_frame_paths: &HashMap<String, Vec<PathBuf>>,
) -> Vec<PathBuf> {
    let mut ordered = observations.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| {
        right
            .created_at()
            .cmp(left.created_at())
            .then_with(|| right.id().cmp(left.id()))
    });

    let mut seen = HashSet::new();
    ordered
        .into_iter()
        .flat_map(|observation| {
            observation_frame_paths
                .get(observation.id())
                .into_iter()
                .flat_map(|paths| paths.iter().rev())
        })
        .filter(|path| seen.insert((*path).clone()))
        .cloned()
        .collect()
}

pub(super) fn repeated_error_count(observations: &[ObservationRecord]) -> usize {
    let mut last = None;
    let mut count = 0;
    for observation in observations.iter().rev() {
        let Some(value) = (match observation {
            ObservationRecord::Visual(item) => item
                .data
                .events
                .iter()
                .find(|event| event.event_type == ObservationEventType::Error)
                .map(|event| &event.detail),
            ObservationRecord::NoChange(_) | ObservationRecord::Audio(_) => None,
        }) else {
            break;
        };
        if last
            .as_deref()
            .is_none_or(|previous: &str| previous == value)
        {
            count += 1;
            last = Some(value.clone());
        } else {
            break;
        }
    }
    count
}

pub(super) fn remember_sent_observations(
    sent_ids: &mut VecDeque<String>,
    observations: &[ObservationRecord],
) {
    for observation in observations {
        if !sent_ids.iter().any(|id| id == observation.id()) {
            sent_ids.push_back(observation.id().to_owned());
        }
    }
    while sent_ids.len() > 500 {
        sent_ids.pop_front();
    }
}

pub(super) fn unsent_observations(
    observations: Vec<ObservationRecord>,
    sent_ids: &VecDeque<String>,
) -> Vec<ObservationRecord> {
    let mut unique = std::collections::HashSet::new();
    let mut observations = observations
        .into_iter()
        .filter(|observation| {
            unique.insert(observation.id().to_owned())
                && !sent_ids.iter().any(|id| id == observation.id())
        })
        .collect::<Vec<_>>();
    observations.sort_by(|left, right| left.created_at().cmp(right.created_at()));
    observations
}

pub(super) fn select_observations(
    observations: &[ObservationRecord],
    limit: usize,
) -> (Vec<&ObservationRecord>, Vec<&ObservationRecord>) {
    if limit == 0 || observations.is_empty() {
        return (Vec::new(), observations.iter().collect());
    }
    if observations.len() <= limit {
        return (observations.iter().collect(), Vec::new());
    }
    let mut selected = Vec::new();
    add_observation(&mut selected, observations.first(), limit);
    add_observation(&mut selected, observations.last(), limit);
    for observation in observations.iter().filter(
        |value| matches!(value, ObservationRecord::Visual(item) if !item.data.events.is_empty()),
    ) {
        add_observation(&mut selected, Some(observation), limit);
    }
    for observation in observations {
        add_observation(&mut selected, Some(observation), limit);
    }
    let omitted = observations
        .iter()
        .filter(|value| !selected.iter().any(|item| item.id() == value.id()))
        .collect();
    (selected, omitted)
}

fn add_observation<'a>(
    selected: &mut Vec<&'a ObservationRecord>,
    value: Option<&'a ObservationRecord>,
    limit: usize,
) {
    if let Some(value) = value {
        if selected.len() < limit && !selected.iter().any(|item| item.id() == value.id()) {
            selected.push(value);
        }
    }
}
