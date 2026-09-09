use super::{retain_entries, BubbleEntry, BubbleMilestone, BubbleRecord, BubbleState};
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum BubbleDeckDirection {
    Older,
    Newer,
    Latest,
}

pub(crate) fn reading_delay(message: &str) -> Duration {
    let characters = message
        .chars()
        .filter(|character| !character.is_whitespace())
        .count() as u64;
    Duration::from_millis(
        1_200_u64
            .saturating_add(characters.saturating_mul(110))
            .min(10_000),
    )
}

impl BubbleState {
    pub(super) fn retire_cards(&mut self, ids: &[String], now: Instant) -> bool {
        let before = self.entries.len();
        retain_entries(&mut self.entries, |entry| {
            !ids.contains(&entry.record.id)
                || (entry.display_order.is_some() && entry.record.interaction.is_none())
        });
        let mut changed = before != self.entries.len();
        for entry in self
            .entries
            .iter_mut()
            .filter(|entry| ids.contains(&entry.record.id))
        {
            if !entry.read.is_cancelled() || entry.record.persistent || entry.expires_at.is_none() {
                entry.read.cancel();
                entry.reading_until = None;
                entry.reading_remaining = Duration::ZERO;
                entry.record.persistent = false;
                entry.restarts_setup_on_dismiss = false;
                entry.expires_at.get_or_insert(now + entry.duration);
                changed = true;
            }
        }
        self.hovered
            .retain(|id| self.entries.iter().any(|entry| &entry.record.id == id));
        changed
    }

    pub(super) fn front_record(&self) -> Option<&BubbleRecord> {
        let id = self.history_id.as_ref().or(self.active_id.as_ref())?;
        self.entries
            .iter()
            .find(|entry| &entry.record.id == id)
            .map(|entry| &entry.record)
    }

    pub(super) fn front_is_reading(&self) -> bool {
        self.history_id.is_none()
            && self
                .active_entry()
                .is_some_and(|entry| !entry.read.is_cancelled())
    }

    pub(super) fn history_ids(&self) -> Vec<String> {
        let mut displayed = self
            .entries
            .iter()
            .filter(|entry| entry.display_order.is_some())
            .collect::<Vec<_>>();
        displayed.sort_by_key(|entry| entry.display_order);
        displayed
            .into_iter()
            .map(|entry| entry.record.id.clone())
            .collect()
    }

    pub(crate) fn card_completion(
        &self,
        id: &str,
        milestone: BubbleMilestone,
    ) -> Option<(CancellationToken, CancellationToken)> {
        self.entries
            .iter()
            .find(|entry| entry.record.id == id)
            .map(|entry| {
                let completed = match milestone {
                    BubbleMilestone::Activated => &entry.activated,
                    BubbleMilestone::Read => &entry.read,
                };
                (completed.clone(), entry.presentation.clone())
            })
    }

    fn active_entry(&self) -> Option<&BubbleEntry> {
        self.entries
            .iter()
            .find(|entry| Some(&entry.record.id) == self.active_id.as_ref())
    }

    fn pause_reading(&mut self, now: Instant) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| Some(&entry.record.id) == self.active_id.as_ref())
        {
            if let Some(deadline) = entry.reading_until.take() {
                entry.reading_remaining = deadline.saturating_duration_since(now);
            }
        }
    }

    pub(super) fn activate(&mut self, id: String, now: Instant) {
        self.pause_reading(now);
        if let Some(previous) = self
            .entries
            .iter_mut()
            .find(|entry| Some(&entry.record.id) == self.active_id.as_ref())
        {
            if previous.read.is_cancelled() && previous.record.interaction.is_none() {
                previous.expires_at.get_or_insert(now + previous.duration);
                previous.restarts_setup_on_dismiss = false;
            }
        }
        self.active_id = Some(id.clone());
        self.history_id = None;
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.record.id == id)
            .expect("deck activation requires a retained card");
        self.display_sequence = self.display_sequence.saturating_add(1);
        entry.display_order = Some(self.display_sequence);
        entry.activated.cancel();
        if !entry.read.is_cancelled() {
            entry.reading_until = Some(now + entry.reading_remaining);
        }
    }

    fn finish_reading(&mut self, now: Instant) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| Some(&entry.record.id) == self.active_id.as_ref())
        else {
            return false;
        };
        if entry.read.is_cancelled() {
            return false;
        }
        entry.read.cancel();
        entry.reading_until = None;
        entry.reading_remaining = Duration::ZERO;
        if !entry.record.persistent && entry.record.interaction.is_none() {
            entry.expires_at = Some(now + entry.duration);
        }
        true
    }

    pub(crate) fn fast_forward(&mut self, id: Option<&str>, now: Instant) -> bool {
        let Some(front) = self.front_record() else {
            return false;
        };
        if id.is_some_and(|id| id != front.id) {
            return false;
        }
        if self.history_id.is_some() {
            return self.navigate(BubbleDeckDirection::Latest, now);
        }
        // Actions keep their position; clicking their text only completes the typing effect.
        let finished = self.finish_reading(now);
        let moved = self.reconcile_deck(now);
        if finished && !moved {
            self.mark_changed();
        }
        finished || moved
    }

    pub(crate) fn navigate(&mut self, direction: BubbleDeckDirection, now: Instant) -> bool {
        if self
            .active_entry()
            .is_some_and(|entry| entry.record.interaction.is_some())
        {
            return false;
        }
        let ids = self.history_ids();
        let Some(front) = self.front_record() else {
            return false;
        };
        let Some(index) = ids.iter().position(|id| id == &front.id) else {
            return false;
        };
        let target = match direction {
            BubbleDeckDirection::Older => index.checked_sub(1).and_then(|index| ids.get(index)),
            BubbleDeckDirection::Newer => ids.get(index + 1),
            BubbleDeckDirection::Latest => self.active_id.as_ref(),
        }
        .cloned();
        let Some(target) = target else {
            return false;
        };
        if target == front.id {
            return false;
        }
        self.pause_reading(now);
        self.interrupted_history = None;
        self.history_id = (Some(&target) != self.active_id.as_ref()).then_some(target);
        self.reconcile_deck(now);
        self.mark_changed();
        true
    }

    pub(super) fn reconcile_deck(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if self
            .history_id
            .as_ref()
            .is_some_and(|id| !self.entries.iter().any(|entry| &entry.record.id == id))
        {
            self.history_id = None;
            changed = true;
        }
        if self
            .active_id
            .as_ref()
            .is_some_and(|id| !self.entries.iter().any(|entry| &entry.record.id == id))
        {
            self.active_id = None;
            self.history_id = None;
            changed = true;
        }
        let critical = self
            .entries
            .iter()
            .rev()
            .find(|entry| {
                entry.display_order.is_none() && entry.record.notification_priority == "critical"
            })
            .map(|entry| entry.record.id.clone());
        if let Some(id) = critical {
            if let (Some(history), Some(active)) = (&self.history_id, &self.active_id) {
                self.interrupted_history = Some((history.clone(), active.clone()));
            }
            self.activate(id, now);
            changed = true;
        }
        let paused = self.history_id.is_some();
        if !paused
            && self
                .active_entry()
                .and_then(|entry| entry.reading_until)
                .is_some_and(|deadline| deadline <= now)
        {
            changed |= self.finish_reading(now);
        }
        if self.interrupted_history.is_some()
            && !self.entries.iter().any(|entry| {
                entry.record.notification_priority == "critical"
                    && (!entry.read.is_cancelled() || entry.record.interaction.is_some())
            })
        {
            let (history, active) = self
                .interrupted_history
                .take()
                .expect("interrupted history was checked");
            if self.entries.iter().any(|entry| entry.record.id == active) {
                self.activate(active, now);
                if self.entries.iter().any(|entry| entry.record.id == history) {
                    self.pause_reading(now);
                    self.history_id = Some(history);
                }
            }
            changed = true;
        }
        let paused = self.history_id.is_some();
        let can_advance = !paused
            && self.active_entry().is_none_or(|entry| {
                entry.record.interaction.is_none() && entry.read.is_cancelled()
            });
        if can_advance {
            let next = self
                .entries
                .iter()
                .find(|entry| {
                    entry.display_order.is_some()
                        && entry.record.notification_priority == "critical"
                        && !entry.read.is_cancelled()
                        && Some(&entry.record.id) != self.active_id.as_ref()
                })
                .or_else(|| {
                    self.entries.iter().find(|entry| {
                        entry.display_order.is_some()
                            && entry.record.interaction.is_some()
                            && Some(&entry.record.id) != self.active_id.as_ref()
                    })
                })
                .or_else(|| {
                    self.entries.iter().find(|entry| {
                        entry.display_order.is_some()
                            && !entry.read.is_cancelled()
                            && Some(&entry.record.id) != self.active_id.as_ref()
                    })
                })
                .or_else(|| {
                    self.entries
                        .iter()
                        .find(|entry| entry.display_order.is_none())
                })
                .or_else(|| {
                    if self.active_id.is_none() {
                        self.entries.last()
                    } else {
                        None
                    }
                })
                .map(|entry| entry.record.id.clone());
            if let Some(id) = next {
                self.activate(id, now);
                changed = true;
            }
        }
        if !paused {
            if let Some(entry) = self
                .entries
                .iter_mut()
                .find(|entry| Some(&entry.record.id) == self.active_id.as_ref())
            {
                if !entry.read.is_cancelled() && entry.reading_until.is_none() {
                    entry.reading_until = Some(now + entry.reading_remaining);
                }
            }
        }
        changed |= self.trim_deck();
        if changed {
            self.mark_changed();
        }
        changed
    }

    pub(super) fn trim_deck(&mut self) -> bool {
        let before = self.entries.len();
        // Unanswered actions survive both limits. Pending ordinary notices keep only the newest n.
        for displayed in [false, true] {
            while self
                .entries
                .iter()
                .filter(|entry| {
                    entry.display_order.is_some() == displayed && entry.record.interaction.is_none()
                })
                .count()
                > self.max_stack
            {
                let candidate = self.entries.iter().find(|entry| {
                    entry.display_order.is_some() == displayed
                        && entry.record.interaction.is_none()
                        && Some(&entry.record.id) != self.active_id.as_ref()
                });
                let Some(id) = candidate.map(|entry| entry.record.id.clone()) else {
                    break;
                };
                retain_entries(&mut self.entries, |entry| entry.record.id != id);
                self.hovered.remove(&id);
                if self.history_id.as_ref() == Some(&id) {
                    self.history_id = None;
                }
            }
        }
        self.entries.len() != before
    }
}
