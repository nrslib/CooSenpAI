use crate::state::DesktopState;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[path = "bubble_registration.rs"]
mod registration;
pub(crate) use registration::{complete_presentation, register, show_replacing};
#[cfg(test)]
pub(crate) use registration::{
    register_replacing_for_surface, wait_for_acknowledgement, wait_for_presentation_completion,
};
pub use registration::{show, show_best_effort};

const EXIT_ANIMATION: Duration = Duration::from_millis(180);
const TUTORIAL_SEQUENCE_GAP: Duration = Duration::from_millis(250);

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interaction: Option<BubbleInteraction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BubbleInteraction {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub select: Option<BubbleSelect>,
    #[serde(default)]
    pub actions: Vec<BubbleAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub technical_detail: Option<String>,
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
    pub theme: String,
    pub font: String,
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

struct BubbleEntry {
    record: BubbleRecord,
    expires_at: Option<Instant>,
    presentation: CancellationToken,
    restarts_setup_on_dismiss: bool,
}

pub(crate) struct BubblePresentation {
    generation: u64,
    acknowledgements: watch::Receiver<u64>,
    dismissed: CancellationToken,
    registered_on_bubble_surface: bool,
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
}

#[derive(Debug, Default)]
pub(crate) struct BubbleWindowSyncState {
    has_content: bool,
    position: Option<String>,
    display: Option<String>,
}

impl BubbleWindowSyncState {
    fn needs_initial_show(&self, has_content: bool) -> bool {
        has_content && !self.has_content
    }

    fn layout_changed(&self, position: &str, display: &str) -> bool {
        self.position.as_deref() != Some(position) || self.display.as_deref() != Some(display)
    }

    fn commit(&mut self, has_content: bool, position: &str, display: &str) {
        self.has_content = has_content;
        self.position = Some(position.to_owned());
        self.display = Some(display.to_owned());
    }
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
        }
    }
}

impl BubbleState {
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
        self.generation = self.generation.saturating_add(1);
        true
    }

    pub(crate) fn appearance_preview(&self) -> Option<BubbleAppearancePreview> {
        self.appearance_preview.clone()
    }

    pub(crate) fn record_for_message_kind(&self, message_kind: &str) -> Option<BubbleRecord> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.record.message_kind == message_kind)
            .map(|entry| entry.record.clone())
    }

    pub(crate) fn open_url_for(&self, id: &str) -> Option<String> {
        self.entries
            .iter()
            .find(|entry| entry.record.id == id)
            .and_then(|entry| entry.record.open_url.clone())
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
        retain_entries(&mut self.entries, |entry| {
            entry.record.conversation_generation >= generation
        });
        let retained = self
            .entries
            .iter()
            .map(|entry| entry.record.id.as_str())
            .collect::<HashSet<_>>();
        self.hovered.retain(|id| retained.contains(id.as_str()));
        self.generation = self.generation.saturating_add(1);
        true
    }

    pub fn set_max_stack(&mut self, max_stack: usize) -> bool {
        let before = self.entries.len();
        while self.entries.len() > max_stack {
            let Some(index) = eviction_index(&self.entries) else {
                break;
            };
            let removed = self.entries.remove(index);
            removed.presentation.cancel();
            self.hovered.remove(&removed.record.id);
        }
        if self.entries.len() == before {
            return false;
        }
        self.generation = self.generation.saturating_add(1);
        true
    }

    pub fn show(
        &mut self,
        record: BubbleRecord,
        now: Instant,
        duration: Duration,
        max_stack: usize,
    ) -> bool {
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
                entry.expires_at = (!(record.persistent || record.interaction.is_some()))
                    .then_some(now + duration);
                entry.record = record;
                self.generation = self.generation.saturating_add(1);
                return true;
            }
        }
        let replaced_ids = if is_latest_companion_bubble(&record) {
            self.entries
                .iter()
                .filter(|entry| is_latest_companion_bubble(&entry.record))
                .map(|entry| entry.record.id.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        self.show_replacing(record, now, duration, max_stack, &replaced_ids)
    }

    pub(crate) fn show_replacing(
        &mut self,
        mut record: BubbleRecord,
        now: Instant,
        duration: Duration,
        max_stack: usize,
        replaced_ids: &[String],
    ) -> bool {
        if is_tutorial_progress_kind(&record.message_kind) {
            record.persistent = true;
        }
        if record.conversation_generation < self.conversation_generation {
            return false;
        }
        self.advance_conversation_generation(record.conversation_generation);
        let restarts_setup_on_dismiss = record.message_kind == "setup";
        retain_entries(&mut self.entries, |item| {
            item.record.id != record.id && !replaced_ids.contains(&item.record.id)
        });
        self.hovered.retain(|id| !replaced_ids.contains(id));
        self.entries.push(BubbleEntry {
            expires_at: (!(record.persistent || record.interaction.is_some()))
                .then_some(now + duration),
            record,
            presentation: CancellationToken::new(),
            restarts_setup_on_dismiss,
        });
        while self.entries.len() > max_stack {
            let Some(index) = eviction_index(&self.entries) else {
                break;
            };
            let removed = self.entries.remove(index);
            removed.presentation.cancel();
            self.hovered.remove(&removed.record.id);
        }
        self.generation = self.generation.saturating_add(1);
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
        self.generation = self.generation.saturating_add(1);
        true
    }

    pub fn dismiss(&mut self, id: &str) -> bool {
        if !self.allows_manual_dismiss(id) {
            return false;
        }
        self.hovered.remove(id);
        let before = self.entries.len();
        retain_entries(&mut self.entries, |entry| entry.record.id != id);
        let changed = before != self.entries.len();
        if changed {
            self.generation = self.generation.saturating_add(1);
        }
        changed
    }

    pub(crate) fn complete_action(&mut self, id: &str) -> bool {
        self.hovered.remove(id);
        let before = self.entries.len();
        retain_entries(&mut self.entries, |entry| entry.record.id != id);
        let changed = before != self.entries.len();
        if changed {
            self.generation = self.generation.saturating_add(1);
        }
        changed
    }

    pub(crate) fn complete_actions(&mut self, ids: &[String]) -> bool {
        let removed = self
            .entries
            .iter()
            .filter(|entry| ids.contains(&entry.record.id))
            .map(|entry| entry.record.id.clone())
            .collect::<HashSet<_>>();
        if removed.is_empty() {
            return false;
        }
        retain_entries(&mut self.entries, |entry| {
            !removed.contains(&entry.record.id)
        });
        self.hovered.retain(|id| !removed.contains(id));
        self.generation = self.generation.saturating_add(1);
        true
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
        self.generation = self.generation.saturating_add(1);
        true
    }

    fn complete_tutorial_typing(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        retain_entries(&mut self.entries, |entry| {
            entry.record.id != id || entry.record.message_kind != "tutorial-typing"
        });
        let changed = before != self.entries.len();
        if changed {
            self.hovered.remove(id);
            self.generation = self.generation.saturating_add(1);
        }
        changed
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
        self.generation = self.generation.saturating_add(1);
        true
    }

    pub(crate) fn clear_tutorial_progress(&mut self) -> bool {
        let removed = self
            .entries
            .iter()
            .filter(|entry| is_tutorial_progress_kind(&entry.record.message_kind))
            .map(|entry| entry.record.id.clone())
            .collect::<HashSet<_>>();
        if removed.is_empty() {
            return false;
        }
        retain_entries(&mut self.entries, |entry| {
            !is_tutorial_progress_kind(&entry.record.message_kind)
        });
        self.hovered.retain(|id| !removed.contains(id));
        self.generation = self.generation.saturating_add(1);
        true
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
        interaction.select.as_ref().is_some_and(|select| {
            select.action == action
                && value
                    .is_some_and(|value| select.options.iter().any(|option| option.value == value))
        })
    }

    pub fn set_hover(&mut self, id: &str, hovering: bool) {
        if hovering {
            self.hovered.insert(id.to_owned());
        } else {
            self.hovered.remove(id);
        }
        self.generation = self.generation.saturating_add(1);
    }

    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.entries.len();
        let hovered = &self.hovered;
        retain_entries(&mut self.entries, |entry| {
            hovered.contains(&entry.record.id)
                || entry.expires_at.is_none_or(|expires_at| expires_at > now)
        });
        let changed = before != self.entries.len();
        if changed {
            self.generation = self.generation.saturating_add(1);
        }
        changed
    }

    pub fn snapshot(&self) -> BubbleSnapshot {
        BubbleSnapshot {
            generation: self.generation,
            records: self
                .entries
                .iter()
                .map(|entry| entry.record.clone())
                .collect(),
            theme: "system".to_owned(),
            font: "system".to_owned(),
            position: "bottom-right".to_owned(),
            avatar_color: None,
            avatar_image_png: None,
        }
    }

    pub fn snapshot_with_appearance(
        &self,
        theme: &str,
        font: &str,
        avatar_color: Option<&str>,
        position: &str,
        avatar_image_png: Option<&[u8]>,
    ) -> BubbleSnapshot {
        let mut snapshot = self.snapshot();
        snapshot.theme = theme.to_owned();
        snapshot.font = font.to_owned();
        snapshot.avatar_color = avatar_color.map(str::to_owned);
        snapshot.position = position.to_owned();
        snapshot.avatar_image_png = avatar_image_png.map(ToOwned::to_owned);
        snapshot
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

    fn next_expiry(&self) -> Option<(u64, Instant)> {
        self.entries
            .iter()
            .filter(|entry| !self.hovered.contains(&entry.record.id))
            .filter_map(|entry| entry.expires_at)
            .min()
            .map(|deadline| (self.generation, deadline))
    }
}

fn eviction_index(entries: &[BubbleEntry]) -> Option<usize> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let rank = if is_thought_bubble(&entry.record) {
                0
            } else if is_companion_bubble(&entry.record) {
                1
            } else {
                return None;
            };
            (entry.record.interaction.is_none() && !entry.record.persistent)
                .then_some((rank, index))
        })
        .min_by_key(|(rank, index)| (*rank, *index))
        .map(|(_, index)| index)
}

fn thought_eviction_index(entries: &[BubbleEntry]) -> Option<usize> {
    entries.iter().enumerate().find_map(|(index, entry)| {
        (is_thought_bubble(&entry.record)
            && entry.record.interaction.is_none()
            && !entry.record.persistent)
            .then_some(index)
    })
}

fn is_latest_companion_bubble(record: &BubbleRecord) -> bool {
    record.persistent && record.interaction.is_none() && is_companion_bubble(record)
}

fn is_thought_bubble(record: &BubbleRecord) -> bool {
    record.message_kind == "thought"
}

fn is_companion_bubble(record: &BubbleRecord) -> bool {
    matches!(
        record.message_kind.as_str(),
        "advice" | "encouragement" | "nudge" | "celebration" | "summary" | "chat"
    )
}

fn is_tutorial_progress_kind(message_kind: &str) -> bool {
    matches!(message_kind, "tutorial" | "tutorial-typing")
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

pub async fn dismiss(state: &DesktopState, id: &str) {
    if state.bubbles.lock().await.dismiss(id) {
        let _ = sync_window(state).await;
    }
}

pub async fn complete_action(state: &DesktopState, id: &str) {
    if state.bubbles.lock().await.complete_action(id) {
        let _ = sync_window(state).await;
    }
}

pub(crate) async fn complete_actions(state: &DesktopState, ids: &[String]) -> bool {
    let changed = state.bubbles.lock().await.complete_actions(ids);
    if changed {
        let _ = sync_window(state).await;
    }
    changed
}

pub async fn complete_tutorial_typing(state: &DesktopState, id: &str) {
    if state.bubbles.lock().await.complete_tutorial_typing(id) {
        let _ = sync_window(state).await;
    }
}

pub async fn clear_tutorial_typing(state: &DesktopState) -> bool {
    let changed = state
        .bubbles
        .lock()
        .await
        .dismiss_message_kind("tutorial-typing");
    if changed {
        let _ = sync_window(state).await;
    }
    changed
}

pub async fn clear_tutorial_progress(state: &DesktopState) -> bool {
    let changed = state.bubbles.lock().await.clear_tutorial_progress();
    if changed {
        let _ = sync_window(state).await;
    }
    changed
}

pub(crate) async fn wait_for_tutorial_bubble_transition(cancellation: &CancellationToken) -> bool {
    if cancellation.is_cancelled() {
        return false;
    }
    tokio::select! {
        () = tokio::time::sleep(EXIT_ANIMATION + TUTORIAL_SEQUENCE_GAP) => true,
        () = cancellation.cancelled() => false,
    }
}

pub(crate) async fn clear_for_main_window(state: &DesktopState) {
    let cleared = {
        let mut bubbles = state.bubbles.lock().await;
        if !state.main_window_focused.load(Ordering::Acquire) {
            return;
        }
        bubbles.clear_for_main_window()
    };
    if cleared {
        let _ = sync_window(state).await;
    }
}

pub async fn set_hover(state: Arc<DesktopState>, id: &str, hovering: bool) {
    state.bubbles.lock().await.set_hover(id, hovering);
    schedule_expiry(state).await;
}

pub(crate) async fn sync_window(state: &DesktopState) -> Result<()> {
    let mut window_sync = state.bubble_window_sync.lock().await;
    let config = state.runtime_config();
    let avatar_image_png = state.snapshot().await.avatar_image_png;
    let (snapshot, display) = {
        let bubbles = state.bubbles.lock().await;
        let preview = bubbles.appearance_preview();
        let theme = preview
            .as_ref()
            .map_or(config.ui.theme.as_str(), |value| value.theme.as_str());
        let font = preview
            .as_ref()
            .map_or(config.ui.font.as_str(), |value| value.font.as_str());
        let avatar_color = preview
            .as_ref()
            .map(|value| value.avatar_color.as_str())
            .or(config.ui.avatar_color.as_deref());
        let position = preview
            .as_ref()
            .map_or(config.bubble.position.as_str(), |value| {
                value.position.as_str()
            });
        let display = preview
            .as_ref()
            .map_or(config.bubble.display.clone(), |value| value.display.clone());
        (
            bubbles.snapshot_with_appearance(
                theme,
                font,
                avatar_color,
                position,
                avatar_image_png.as_deref(),
            ),
            display,
        )
    };
    let window = state
        .app
        .get_webview_window("bubble")
        .context("吹き出しウィンドウがありません")?;
    window.emit("coosenpai:bubble:show", snapshot.clone())?;
    let has_content = !snapshot.records.is_empty();
    let initial_show = window_sync.needs_initial_show(has_content);
    let layout_changed = window_sync.layout_changed(&snapshot.position, &display);
    if !has_content {
        if window_sync.has_content {
            window.set_ignore_cursor_events(true)?;
        }
        let app = state.app.clone();
        let generation = snapshot.generation;
        if window_sync.has_content {
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(EXIT_ANIMATION).await;
                let Some(state) = app.try_state::<Arc<DesktopState>>() else {
                    return;
                };
                let current = state.bubbles.lock().await.snapshot();
                if current.generation == generation && current.records.is_empty() {
                    if let Some(window) = app.get_webview_window("bubble") {
                        let _ = window.hide();
                    }
                }
            });
        }
    } else {
        if initial_show || layout_changed {
            crate::window_bubble::update_layout(
                &window,
                snapshot.records.len(),
                &snapshot.position,
                &display,
            )?;
        }
        if initial_show {
            window.set_ignore_cursor_events(false)?;
            window.show()?;
        }
    }
    window_sync.commit(has_content, &snapshot.position, &display);
    Ok(())
}

pub(crate) fn accepts_pointer(record_count: usize) -> bool {
    record_count > 0
}

pub(super) async fn schedule_expiry(state: Arc<DesktopState>) {
    let Some((mut generation, mut deadline)) = state.bubbles.lock().await.next_expiry() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(deadline.saturating_duration_since(Instant::now())).await;
            let (changed, next) = {
                let mut bubbles = state.bubbles.lock().await;
                if bubbles.generation != generation {
                    return;
                }
                let changed = bubbles.expire(Instant::now());
                (changed, bubbles.next_expiry())
            };
            if changed {
                let _ = sync_window(&state).await;
            }
            let Some((next_generation, next_deadline)) = next else {
                return;
            };
            generation = next_generation;
            deadline = next_deadline;
            if deadline <= Instant::now() {
                tokio::task::yield_now().await;
            }
        }
    });
}

