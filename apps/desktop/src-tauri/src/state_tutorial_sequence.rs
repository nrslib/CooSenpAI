use super::*;
use crate::bubbles::{self, BubbleRecord};
use crate::tutorial_notice::TutorialBubbleOutcome;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const READING_BASE_DELAY_MS: u64 = 1_200;
const READING_DELAY_PER_CHARACTER_MS: u64 = 110;
const READING_MAX_DELAY_MS: u64 = 10_000;

struct TutorialSequenceWait {
    generation: u64,
    bubble_ids: [String; 2],
    fast_forward: CancellationToken,
}

enum TutorialWaitDecision {
    Wait(CancellationToken),
    Immediate,
}

pub(super) struct TutorialSequenceControl {
    generation: u64,
    cancellation: CancellationToken,
    armed_source: Option<(u64, String)>,
    waiting: Option<TutorialSequenceWait>,
    fast_forwarded: bool,
}

impl Default for TutorialSequenceControl {
    fn default() -> Self {
        Self {
            generation: 0,
            cancellation: CancellationToken::new(),
            armed_source: None,
            waiting: None,
            fast_forwarded: false,
        }
    }
}

impl TutorialSequenceControl {
    fn start(&mut self) -> (u64, CancellationToken) {
        self.cancellation.cancel();
        if let Some(previous) = self.waiting.take() {
            previous.fast_forward.cancel();
        }
        self.generation = self.generation.saturating_add(1);
        self.cancellation = CancellationToken::new();
        self.armed_source = None;
        self.fast_forwarded = false;
        (self.generation, self.cancellation.clone())
    }

    fn arm_source(&mut self, generation: u64, source_id: String) -> bool {
        if self.generation != generation || self.cancellation.is_cancelled() {
            return false;
        }
        self.armed_source = Some((generation, source_id));
        true
    }

    fn begin_wait(
        &mut self,
        generation: u64,
        source_id: String,
        typing_id: String,
    ) -> Option<TutorialWaitDecision> {
        if self.generation != generation || self.cancellation.is_cancelled() {
            return None;
        }
        if !matches!(&self.armed_source, Some((current, id)) if *current == generation && id == &source_id)
        {
            return None;
        }
        if let Some(previous) = self.waiting.take() {
            previous.fast_forward.cancel();
        }
        if self.fast_forwarded {
            return Some(TutorialWaitDecision::Immediate);
        }
        let fast_forward = CancellationToken::new();
        self.waiting = Some(TutorialSequenceWait {
            generation,
            bubble_ids: [source_id, typing_id],
            fast_forward: fast_forward.clone(),
        });
        Some(TutorialWaitDecision::Wait(fast_forward))
    }

    fn fast_forward(&mut self, bubble_id: Option<&str>) -> bool {
        if let Some(waiting) = &self.waiting {
            if bubble_id.is_some_and(|id| !waiting.bubble_ids.iter().any(|item| item == id)) {
                return false;
            }
            self.fast_forwarded = true;
            waiting.fast_forward.cancel();
            return true;
        }
        let Some((_, source_id)) = &self.armed_source else {
            return false;
        };
        if bubble_id.is_some_and(|id| id != source_id) {
            return false;
        }
        self.fast_forwarded = true;
        true
    }

    fn finish_wait(&mut self, generation: u64) {
        if self
            .waiting
            .as_ref()
            .is_some_and(|waiting| waiting.generation == generation)
        {
            self.waiting = None;
        }
    }

    fn finish_sequence(&mut self, generation: u64) {
        if self.generation == generation {
            self.armed_source = None;
            self.waiting = None;
            self.fast_forwarded = false;
        }
    }

    fn finish_after_final_replacement<T, E>(
        &mut self,
        generation: u64,
        outcome: Result<T, E>,
    ) -> Result<T, E> {
        self.finish_sequence(generation);
        outcome
    }

    fn cancel(&mut self) {
        self.cancellation.cancel();
        self.armed_source = None;
        if let Some(waiting) = self.waiting.take() {
            waiting.fast_forward.cancel();
        }
    }
}

pub(super) fn tutorial_reading_delay(message: &str) -> Duration {
    let characters = message
        .chars()
        .filter(|character| !character.is_whitespace())
        .count() as u64;
    Duration::from_millis(
        READING_BASE_DELAY_MS
            .saturating_add(characters.saturating_mul(READING_DELAY_PER_CHARACTER_MS))
            .min(READING_MAX_DELAY_MS),
    )
}

impl DesktopState {
    pub(super) async fn emit_tutorial_intro_sequence(
        self: &Arc<Self>,
        follows_setup_ok: bool,
    ) -> Result<(), RuntimeError> {
        let (generation, sequence_cancellation) = self.tutorial_sequence.lock().await.start();
        if bubbles::clear_tutorial_typing(self.as_ref()).await
            && !bubbles::wait_for_tutorial_bubble_transition(&sequence_cancellation).await
        {
            self.finish_tutorial_sequence(generation).await;
            return Ok(());
        }
        if follows_setup_ok {
            if !self
                .arm_tutorial_sequence_source("setup-ok", generation)
                .await?
            {
                self.finish_tutorial_sequence(generation).await;
                return Ok(());
            }
            if self.emit_tutorial_message("setup-ok").await? != TutorialBubbleOutcome::Acknowledged
            {
                self.finish_tutorial_sequence(generation).await;
                return Ok(());
            }
            if !self
                .wait_for_tutorial_message("setup-ok", "intro", generation, &sequence_cancellation)
                .await?
            {
                self.finish_tutorial_sequence(generation).await;
                return Ok(());
            }
        }
        if !self
            .arm_tutorial_sequence_source("intro", generation)
            .await?
        {
            self.remove_tutorial_sequence_slot("intro").await;
            self.finish_tutorial_sequence(generation).await;
            return Ok(());
        }
        let intro_outcome = self.emit_tutorial_message("intro").await;
        if !matches!(intro_outcome, Ok(TutorialBubbleOutcome::Acknowledged)) {
            if intro_outcome.is_err() {
                self.remove_tutorial_sequence_slot("intro").await;
            }
            self.finish_tutorial_sequence(generation).await;
            return intro_outcome.map(|_| ());
        }
        if !self
            .wait_for_tutorial_message("intro", "intro-click", generation, &sequence_cancellation)
            .await?
        {
            self.finish_tutorial_sequence(generation).await;
            return Ok(());
        }
        let mut replaced = self.tutorial_notice_ids(&["intro"]).await?;
        if follows_setup_ok {
            replaced.extend(self.tutorial_notice_ids(&["setup-ok"]).await?);
        }
        let outcome = self
            .emit_tutorial_message_replacing("intro-click", replaced, sequence_cancellation)
            .await;
        if outcome.is_err() {
            self.remove_tutorial_sequence_slot("intro-click").await;
        }
        self.finish_tutorial_sequence_after_replacement(generation, outcome)
            .await
            .map(|_| ())
    }

    pub(super) async fn emit_watch_intro_sequence(
        self: &Arc<Self>,
    ) -> Result<TutorialBubbleOutcome, RuntimeError> {
        let (generation, sequence_cancellation) = self.tutorial_sequence.lock().await.start();
        if bubbles::clear_tutorial_typing(self.as_ref()).await
            && !bubbles::wait_for_tutorial_bubble_transition(&sequence_cancellation).await
        {
            self.finish_tutorial_sequence(generation).await;
            return Ok(TutorialBubbleOutcome::Dismissed);
        }
        if !self
            .arm_tutorial_sequence_source("after-persona", generation)
            .await?
        {
            self.finish_tutorial_sequence(generation).await;
            return Ok(TutorialBubbleOutcome::Dismissed);
        }
        let outcome = self.emit_tutorial_message("after-persona").await?;
        if outcome != TutorialBubbleOutcome::Acknowledged {
            self.finish_tutorial_sequence(generation).await;
            return Ok(outcome);
        }
        if !self
            .wait_for_tutorial_message(
                "after-persona",
                "watch-intro",
                generation,
                &sequence_cancellation,
            )
            .await?
        {
            self.finish_tutorial_sequence(generation).await;
            return Ok(TutorialBubbleOutcome::Dismissed);
        }
        let replaced = self
            .tutorial_notice_ids(&["persona-intro", "after-persona"])
            .await?;
        let outcome = self
            .emit_tutorial_message_replacing("watch-intro", replaced, sequence_cancellation)
            .await;
        if outcome.is_err() {
            self.remove_tutorial_sequence_slot("watch-intro").await;
        }
        self.finish_tutorial_sequence_after_replacement(generation, outcome)
            .await
    }

    pub(crate) async fn fast_forward_tutorial_sequence(&self, bubble_id: Option<&str>) -> bool {
        self.tutorial_sequence.lock().await.fast_forward(bubble_id)
    }

    pub(super) async fn cancel_tutorial_sequence(&self) {
        self.tutorial_sequence.lock().await.cancel();
        bubbles::clear_tutorial_typing(self).await;
    }

    async fn wait_for_tutorial_message(
        self: &Arc<Self>,
        source_key: &str,
        target_key: &str,
        generation: u64,
        sequence_cancellation: &CancellationToken,
    ) -> Result<bool, RuntimeError> {
        let (message, source_id, target_id) = {
            let tutorial = self.tutorial.lock().await;
            let provider = tutorial.provider().ok_or_else(|| {
                RuntimeError::Factory("チュートリアルが開始されていません".to_owned())
            })?;
            let message = provider
                .render(source_key)
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
            let source_id = tutorial
                .state()
                .tutorial_notice_id(source_key)
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
            let target_id = tutorial
                .state()
                .tutorial_notice_id(target_key)
                .map_err(|error| RuntimeError::Factory(error.to_string()))?;
            (message, source_id, target_id)
        };
        let Some(wait) = self.tutorial_sequence.lock().await.begin_wait(
            generation,
            source_id,
            target_id.clone(),
        ) else {
            return Ok(false);
        };
        let TutorialWaitDecision::Wait(fast_forward) = wait else {
            return Ok(true);
        };
        self.show_tutorial_typing(target_id.clone()).await;
        let completed = tokio::select! {
            () = tokio::time::sleep(tutorial_reading_delay(&message)) => true,
            () = fast_forward.cancelled() => true,
            () = sequence_cancellation.cancelled() => false,
        };
        if !completed {
            bubbles::complete_action(self.as_ref(), &target_id).await;
        }
        self.tutorial_sequence.lock().await.finish_wait(generation);
        Ok(completed)
    }

    async fn remove_tutorial_sequence_slot(&self, key: &str) {
        let Ok(ids) = self.tutorial_notice_ids(&[key]).await else {
            return;
        };
        bubbles::complete_tutorial_typing(self, &ids[0]).await;
    }

    async fn finish_tutorial_sequence(&self, generation: u64) {
        self.tutorial_sequence
            .lock()
            .await
            .finish_sequence(generation);
    }

    async fn finish_tutorial_sequence_after_replacement<T, E>(
        &self,
        generation: u64,
        outcome: Result<T, E>,
    ) -> Result<T, E> {
        self.tutorial_sequence
            .lock()
            .await
            .finish_after_final_replacement(generation, outcome)
    }

    async fn arm_tutorial_sequence_source(
        &self,
        key: &str,
        generation: u64,
    ) -> Result<bool, RuntimeError> {
        let source_id = self.tutorial_notice_ids(&[key]).await?.remove(0);
        Ok(self
            .tutorial_sequence
            .lock()
            .await
            .arm_source(generation, source_id))
    }

    async fn show_tutorial_typing(self: &Arc<Self>, id: String) {
        if crate::windows::main_is_focused(&self.app) {
            return;
        }
        let config = self.runtime.config();
        let conversation_generation = self.bubbles.lock().await.conversation_generation();
        let record = BubbleRecord {
            id,
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            message: "…".to_owned(),
            message_kind: "tutorial-typing".to_owned(),
            notification_priority: "none".to_owned(),
            caused_by: None,
            display_name: config.companion.display_name,
            persona: config.companion.persona,
            avatar_color: config.ui.avatar_color,
            conversation_generation,
            persistent: true,
            open_url: None,
            interaction: None,
        };
        bubbles::show_best_effort(self.clone(), record, config.notification.bubble_duration_ms)
            .await;
    }

    pub(super) async fn tutorial_notice_ids(
        &self,
        keys: &[&str],
    ) -> Result<Vec<String>, RuntimeError> {
        let tutorial = self.tutorial.lock().await;
        keys.iter()
            .map(|key| {
                tutorial
                    .state()
                    .tutorial_notice_id(key)
                    .map_err(|error| RuntimeError::Factory(error.to_string()))
            })
            .collect()
    }
}

