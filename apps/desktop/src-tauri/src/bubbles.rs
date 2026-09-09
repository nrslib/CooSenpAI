use crate::state::DesktopState;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[path = "bubble_presenter.rs"]
pub(crate) mod presenter;
#[path = "bubble_registration.rs"]
mod registration;
#[cfg(test)]
pub(crate) use presenter::register_replacing_for_surface;
pub(crate) use registration::await_presentation;
pub(crate) use registration::{complete_presentation, register, show_replacing};
pub use registration::{show, show_best_effort};
#[cfg(test)]
pub(crate) use registration::{wait_for_acknowledgement, wait_for_presentation_completion};

#[path = "bubble_deck.rs"]
mod deck;
pub(crate) use deck::{reading_delay, BubbleDeckDirection};

pub(crate) const TUTORIAL_TRANSITION_DELAY: Duration = Duration::from_millis(430);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BubbleRecord {
    pub id: String,
    pub created_at: String,
    pub message: String,
    pub message_kind: String,
    pub notification_priority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caused_by: Option<String>,
    pub display_name: String,
    pub persona: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_color: Option<String>,
    pub conversation_generation: u64,
    #[serde(default)]
    pub persistent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interaction: Option<BubbleInteraction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BubbleInteraction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub select: Option<BubbleSelect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_input: Option<BubbleSecretInput>,
    #[serde(default)]
    pub actions: Vec<BubbleAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub technical_detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BubbleSecretInput {
    pub label: String,
    pub placeholder: String,
    pub action: String,
    pub submit_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BubbleSelect {
    pub options: Vec<BubbleOption>,
    pub selected: String,
    pub action: String,
    pub confirm_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BubbleOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BubbleAction {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BubbleSnapshot {
    pub generation: u64,
    pub records: Vec<BubbleRecord>,
    pub front_id: Option<String>,
    pub history_ids: Vec<String>,
    pub reading: bool,
    pub theme: String,
    pub font: String,
    pub language: String,
    pub position: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_image_png: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BubbleAppearancePreview {
    pub theme: String,
    pub font: String,
    pub avatar_color: String,
    pub position: String,
    pub display: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum BubbleMilestone {
    Activated,
    Read,
}

struct BubbleEntry {
    record: BubbleRecord,
    registration: u64,
    explicitly_dismissed: CancellationToken,
    expires_at: Option<Instant>,
    presentation: CancellationToken,
    restarts_setup_on_dismiss: bool,
    display_order: Option<u64>,
    duration: Duration,
    reading_remaining: Duration,
    reading_until: Option<Instant>,
    read: CancellationToken,
    activated: CancellationToken,
}

#[derive(Debug)]
pub(crate) struct BubblePresentation {
    generation: u64,
    acknowledgements: watch::Receiver<u64>,
    dismissed: CancellationToken,
    pub(crate) registered_on_bubble_surface: bool,
    pub(crate) explicitly_dismissed: CancellationToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BubbleClickTarget {
    pub(crate) registration: u64,
    pub(crate) record: Arc<BubbleRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BubblePresentationOutcome {
    Acknowledged,
    Dismissed,
}

pub struct BubbleState {
    entries: Vec<BubbleEntry>,
    hovered: HashSet<String>,
    generation: u64,
    conversation_generation: u64,
    acknowledgement: watch::Sender<u64>,
    appearance_preview: Option<BubbleAppearancePreview>,
    display_sequence: u64,
    active_id: Option<String>,
    history_id: Option<String>,
    interrupted_history: Option<(String, String)>,
    max_stack: usize,
}

impl Default for BubbleState {
    fn default() -> Self {
        let (acknowledgement, _) = watch::channel(0);
        Self {
            entries: Vec::new(),
            hovered: HashSet::new(),
            generation: 0,
            conversation_generation: 0,
            acknowledgement,
            appearance_preview: None,
            display_sequence: 0,
            active_id: None,
            history_id: None,
            interrupted_history: None,
            max_stack: 3,
        }
    }
}

impl BubbleState {
    fn mark_changed(&mut self) {
        self.generation = self.generation.saturating_add(1);
    }

    pub fn for_conversation_generation(conversation_generation: u64) -> Self {
        Self {
            conversation_generation,
            ..Self::default()
        }
    }

    pub fn conversation_generation(&self) -> u64 {
        self.conversation_generation
    }

    pub(crate) fn set_appearance_preview(
        &mut self,
        preview: Option<BubbleAppearancePreview>,
    ) -> bool {
        if self.appearance_preview == preview {
            return false;
        }
        self.appearance_preview = preview;
        self.mark_changed();
        true
    }

    pub(crate) fn appearance_preview(&self) -> Option<BubbleAppearancePreview> {
        self.appearance_preview.clone()
    }

    pub(crate) fn record_for_message_kind(&self, message_kind: &str) -> Option<BubbleRecord> {
        self.entries
            .iter()
            .rev()
            .find(|entry| {
                entry.record.message_kind == message_kind
                    && (message_kind != "setup" || entry.restarts_setup_on_dismiss)
            })
            .map(|entry| entry.record.clone())
    }

    pub(crate) fn restarts_setup_on_dismiss(&self, id: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.record.id == id && entry.restarts_setup_on_dismiss)
    }

    pub(crate) fn allows_manual_dismiss(&self, id: &str) -> bool {
        self.entries
            .iter()
            .find(|entry| entry.record.id == id)
            .is_none_or(|entry| !is_tutorial_progress_kind(&entry.record.message_kind))
    }

    pub fn advance_conversation_generation(&mut self, generation: u64) -> bool {
        if generation <= self.conversation_generation {
            return false;
        }
        self.conversation_generation = generation;
        self.interrupted_history = None;
        retain_entries(&mut self.entries, |entry| {
            entry.record.conversation_generation >= generation
        });
        let retained = self
            .entries
            .iter()
            .map(|entry| entry.record.id.as_str())
            .collect::<HashSet<_>>();
        self.hovered.retain(|id| retained.contains(id.as_str()));
        self.mark_changed();
        true
    }

    pub fn switch_conversation_generation(&mut self, generation: u64) -> bool {
        if generation == self.conversation_generation {
            return false;
        }
        self.conversation_generation = generation;
        self.interrupted_history = None;
        retain_entries(&mut self.entries, |entry| {
            entry.record.conversation_generation == generation
        });
        let retained = self
            .entries
            .iter()
            .map(|entry| entry.record.id.as_str())
            .collect::<HashSet<_>>();
        self.hovered.retain(|id| retained.contains(id.as_str()));
        self.mark_changed();
        true
    }

    pub fn set_max_stack(&mut self, max_stack: usize) -> bool {
        self.max_stack = max_stack;
        if !self.trim_deck() {
            return false;
        }
        self.reconcile_deck(Instant::now());
        self.mark_changed();
        true
    }

    pub fn show(
        &mut self,
        record: BubbleRecord,
        now: Instant,
        duration: Duration,
        max_stack: usize,
    ) -> bool {
        if record.conversation_generation < self.conversation_generation {
            return false;
        }
        if !is_thought_bubble(&record) && self.entries.len() >= max_stack {
            if let Some(index) = thought_eviction_index(&self.entries) {
                let removed = self.entries.remove(index);
                removed.presentation.cancel();
                self.hovered.remove(&removed.record.id);
            }
        }
        if is_thought_bubble(&record)
            && self.entries.len() >= max_stack
            && thought_eviction_index(&self.entries).is_none()
        {
            return false;
        }
        if is_thought_bubble(&record) {
            if record.conversation_generation < self.conversation_generation {
                return false;
            }
            self.advance_conversation_generation(record.conversation_generation);
            if let Some(entry) = self
                .entries
                .iter_mut()
                .find(|entry| is_thought_bubble(&entry.record))
            {
                entry.expires_at = None;
                entry.duration = duration;
                entry.reading_remaining = reading_delay(&record.message);
                if self.active_id.as_ref() == Some(&entry.record.id) {
                    self.active_id = Some(record.id.clone());
                }
                if self.history_id.as_ref() == Some(&entry.record.id) {
                    self.history_id = Some(record.id.clone());
                }
                entry.reading_until = (self.active_id.as_ref() == Some(&record.id)
                    && self.history_id.is_none())
                .then_some(now + entry.reading_remaining);
                entry.read = CancellationToken::new();
                entry.registration = self.generation.saturating_add(1);
                entry.record = record;
                self.reconcile_deck(now);
                self.mark_changed();
                return true;
            }
        }
        self.show_replacing(record, now, duration, max_stack, &[])
    }

    pub(crate) fn show_replacing(
        &mut self,
        record: BubbleRecord,
        now: Instant,
        duration: Duration,
        max_stack: usize,
        replaced_ids: &[String],
    ) -> bool {
        if record.conversation_generation < self.conversation_generation {
            return false;
        }
        self.advance_conversation_generation(record.conversation_generation);
        self.max_stack = max_stack;
        let restarts_setup_on_dismiss = record.message_kind == "setup";
        let replacing_front = self.active_id.as_ref() == Some(&record.id);
        self.retire_cards(replaced_ids, now);
        retain_entries(&mut self.entries, |item| item.record.id != record.id);
        self.hovered.retain(|id| !replaced_ids.contains(id));
        let id = record.id.clone();
        let reading_remaining = reading_delay(&record.message);
        self.entries.push(BubbleEntry {
            registration: self.generation.saturating_add(1),
            explicitly_dismissed: CancellationToken::new(),
            expires_at: None,
            record,
            presentation: CancellationToken::new(),
            restarts_setup_on_dismiss,
            display_order: None,
            duration,
            reading_remaining,
            reading_until: None,
            read: CancellationToken::new(),
            activated: CancellationToken::new(),
        });
        if replacing_front {
            self.activate(id, now);
        }
        self.reconcile_deck(now);
        self.mark_changed();
        true
    }

    pub(crate) fn clear_thought_bubbles(&mut self) -> bool {
        let before = self.entries.len();
        retain_entries(&mut self.entries, |entry| !is_thought_bubble(&entry.record));
        if self.entries.len() == before {
            return false;
        }
        let retained = self
            .entries
            .iter()
            .map(|entry| entry.record.id.as_str())
            .collect::<HashSet<_>>();
        self.hovered.retain(|id| retained.contains(id.as_str()));
        self.mark_changed();
        true
    }

    pub fn dismiss(&mut self, id: &str) -> bool {
        if !self.allows_manual_dismiss(id) {
            return false;
        }
        if let Some(entry) = self.entries.iter().find(|entry| entry.record.id == id) {
            entry.explicitly_dismissed.cancel();
        }
        self.hovered.remove(id);
        let before = self.entries.len();
        retain_entries(&mut self.entries, |entry| entry.record.id != id);
        let changed = before != self.entries.len();
        if changed {
            self.reconcile_deck(Instant::now());
            self.mark_changed();
        }
        changed
    }

    pub(crate) fn complete_action(&mut self, id: &str) -> bool {
        self.hovered.remove(id);
        let before = self.entries.len();
        retain_entries(&mut self.entries, |entry| entry.record.id != id);
        let changed = before != self.entries.len();
        if changed {
            self.reconcile_deck(Instant::now());
            self.mark_changed();
        }
        changed
    }

    pub(crate) fn click_target(&self, id: &str) -> Option<BubbleClickTarget> {
        self.entries
            .iter()
            .find(|entry| entry.record.id == id)
            .map(|entry| BubbleClickTarget {
                registration: entry.registration,
                record: Arc::new(entry.record.clone()),
            })
    }

    pub(crate) fn accepts_click(&self, target: &BubbleClickTarget) -> bool {
        target.record.conversation_generation == self.conversation_generation
            && self.entries.iter().any(|entry| {
                entry.registration == target.registration && entry.record == *target.record
            })
    }

    pub(crate) fn complete_action_if_not_interactive(&mut self, id: &str) -> bool {
        if self
            .entries
            .iter()
            .any(|entry| entry.record.id == id && entry.record.interaction.is_some())
        {
            return false;
        }
        self.complete_action(id)
    }

    pub(crate) fn clear_for_main_window(&mut self) -> bool {
        if self.entries.is_empty() && self.hovered.is_empty() {
            return false;
        }
        let previous_entry_count = self.entries.len();
        let previous_hover_count = self.hovered.len();
        retain_entries(&mut self.entries, |entry| {
            entry.record.interaction.is_some()
        });
        let entries = &self.entries;
        self.hovered
            .retain(|id| entries.iter().any(|entry| entry.record.id == *id));
        if self.entries.len() == previous_entry_count && self.hovered.len() == previous_hover_count
        {
            return false;
        }
        self.mark_changed();
        true
    }

    pub fn dismiss_message_kind(&mut self, message_kind: &str) -> bool {
        let removed = self
            .entries
            .iter()
            .filter(|entry| entry.record.message_kind == message_kind)
            .map(|entry| entry.record.id.clone())
            .collect::<HashSet<_>>();
        if removed.is_empty() {
            return false;
        }
        retain_entries(&mut self.entries, |entry| {
            entry.record.message_kind != message_kind
        });
        self.hovered.retain(|id| !removed.contains(id));
        self.mark_changed();
        true
    }

    pub(crate) fn clear_tutorial_progress(&mut self) -> bool {
        let removed = self
            .entries
            .iter()
            .filter(|entry| is_tutorial_progress_kind(&entry.record.message_kind))
            .map(|entry| entry.record.id.clone())
            .collect::<Vec<_>>();
        if removed.is_empty() {
            return false;
        }
        let changed = self.retire_cards(&removed, Instant::now());
        if changed {
            self.reconcile_deck(Instant::now());
            self.mark_changed();
        }
        changed
    }

    pub fn accepts_interaction(&self, id: &str, action: &str, value: Option<&str>) -> bool {
        let Some(interaction) = self
            .entries
            .iter()
            .find(|entry| entry.record.id == id)
            .and_then(|entry| entry.record.interaction.as_ref())
        else {
            return false;
        };
        if interaction.actions.iter().any(|item| item.id == action) {
            return value.is_none();
        }
        if interaction
            .secret_input
            .as_ref()
            .is_some_and(|input| input.action == action)
        {
            return value.is_some_and(|value| !value.trim().is_empty());
        }
        interaction.select.as_ref().is_some_and(|select| {
            select.action == action
                && value
                    .is_some_and(|value| select.options.iter().any(|option| option.value == value))
        })
    }

    pub fn set_hover(&mut self, id: &str, hovering: bool) {
        let changed = if hovering {
            if !self.entries.iter().any(|entry| entry.record.id == id) {
                return;
            }
            self.hovered.insert(id.to_owned())
        } else {
            self.hovered.remove(id)
        };
        if changed {
            self.mark_changed();
        }
    }

    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.entries.len();
        let hovered = &self.hovered;
        retain_entries(&mut self.entries, |entry| {
            hovered.contains(&entry.record.id)
                || entry.expires_at.is_none_or(|expires_at| expires_at > now)
        });
        let expired = before != self.entries.len();
        let advanced = self.reconcile_deck(now);
        if expired && !advanced {
            self.mark_changed();
        }
        expired || advanced
    }

    pub fn snapshot(&self) -> BubbleSnapshot {
        BubbleSnapshot {
            generation: self.generation,
            records: self
                .entries
                .iter()
                .map(|entry| entry.record.clone())
                .collect(),
            front_id: self.front_record().map(|record| record.id.clone()),
            history_ids: self.history_ids(),
            reading: self.front_is_reading(),
            theme: "system".to_owned(),
            font: "system".to_owned(),
            language: "ja".to_owned(),
            position: "bottom-right".to_owned(),
            avatar_color: None,
            avatar_image_png: None,
        }
    }

    pub fn acknowledge(&self, generation: u64) -> bool {
        if generation > self.generation {
            return false;
        }
        self.acknowledgement.send_if_modified(|acknowledged| {
            if generation > *acknowledged {
                *acknowledged = generation;
                true
            } else {
                false
            }
        });
        true
    }

    fn subscribe_acknowledgements(&self) -> watch::Receiver<u64> {
        self.acknowledgement.subscribe()
    }

    fn presentation_cancellation(&self, id: &str) -> Option<CancellationToken> {
        self.entries
            .iter()
            .find(|entry| entry.record.id == id)
            .map(|entry| entry.presentation.clone())
    }

    fn next_expiry(&self) -> Option<Instant> {
        self.entries
            .iter()
            .filter(|entry| !self.hovered.contains(&entry.record.id))
            .filter_map(|entry| entry.expires_at)
            .chain(self.entries.iter().filter_map(|entry| entry.reading_until))
            .min()
    }
}

fn thought_eviction_index(entries: &[BubbleEntry]) -> Option<usize> {
    entries.iter().enumerate().find_map(|(index, entry)| {
        (is_thought_bubble(&entry.record)
            && entry.record.interaction.is_none()
            && !entry.record.persistent)
            .then_some(index)
    })
}

fn is_thought_bubble(record: &BubbleRecord) -> bool {
    record.message_kind == "thought"
}

fn is_tutorial_progress_kind(message_kind: &str) -> bool {
    message_kind == "tutorial"
}

fn retain_entries(entries: &mut Vec<BubbleEntry>, mut keep: impl FnMut(&BubbleEntry) -> bool) {
    entries.retain(|entry| {
        let retained = keep(entry);
        if !retained {
            entry.presentation.cancel();
        }
        retained
    });
}

#[derive(Debug)]
pub(crate) enum BubbleMutation {
    ConversationGeneration(u64),
    Preview(Option<BubbleAppearancePreview>),
    FastForward(Option<String>),
    Navigate(BubbleDeckDirection),
    Dismiss(String),
    CompleteAction(String),
    ClearTutorialProgress,
    ClearThoughtBubbles,
    SetMaxStack(usize),
    DismissMessageKind(String),
    Hover { id: String, hovering: bool },
}

pub(crate) async fn mutate_checked(
    ui: &crate::ui_root::UiHandle,
    mutation: BubbleMutation,
) -> Result<bool, String> {
    let (reply, response) = tokio::sync::oneshot::channel();
    ui.request(
        crate::ui_events::UiView::Application,
        crate::ui_events::UiEvent::BubbleMutation { mutation, reply },
    )
    .await?;
    response
        .await
        .map_err(|_| "吹き出しの状態変更が終了しました".to_owned())
}

pub(crate) async fn mutate(state: &DesktopState, mutation: BubbleMutation) -> bool {
    mutate_checked(&state.ui, mutation).await.unwrap_or(false)
}

pub async fn dismiss(state: &DesktopState, id: &str) {
    mutate(state, BubbleMutation::Dismiss(id.to_owned())).await;
}

pub async fn complete_action(state: &DesktopState, id: &str) {
    mutate(state, BubbleMutation::CompleteAction(id.to_owned())).await;
}

pub async fn clear_tutorial_progress(state: &DesktopState) -> bool {
    mutate(state, BubbleMutation::ClearTutorialProgress).await
}

pub(crate) async fn wait_for_reading(
    state: &DesktopState,
    id: &str,
    main_reading_delay: Duration,
) -> bool {
    wait_for_card_milestone(state, id, BubbleMilestone::Read, main_reading_delay).await
}

pub(crate) async fn wait_for_card_milestone(
    state: &DesktopState,
    id: &str,
    milestone: BubbleMilestone,
    main_reading_delay: Duration,
) -> bool {
    wait_for_card_milestone_with_cancellation(
        state,
        id,
        milestone,
        main_reading_delay,
        state.cancellation.clone(),
    )
    .await
}

pub(crate) async fn wait_for_card_milestone_with_cancellation(
    state: &DesktopState,
    id: &str,
    milestone: BubbleMilestone,
    main_delay: Duration,
    cancellation: CancellationToken,
) -> bool {
    state
        .ui
        .query(crate::ui_events::UiView::Application, |reply| {
            crate::ui_events::UiEvent::Tutorial(Box::new(
                crate::tutorial_events::TutorialEvent::Card(
                    crate::tutorial_events::CardEvent::WaitCard {
                        notice_id: id.to_owned(),
                        milestone,
                        main_delay,
                        cancellation,
                        reply,
                    },
                ),
            ))
        })
        .await
        .unwrap_or(false)
}

pub async fn set_hover(state: Arc<DesktopState>, id: &str, hovering: bool) {
    mutate(
        &state,
        BubbleMutation::Hover {
            id: id.to_owned(),
            hovering,
        },
    )
    .await;
}

pub(crate) async fn sync_window(state: &DesktopState) -> Result<()> {
    state
        .ui
        .request(
            crate::ui_events::UiView::Application,
            crate::ui_events::UiEvent::BubbleRefresh,
        )
        .await
        .map_err(anyhow::Error::msg)?;
    Ok(())
}

