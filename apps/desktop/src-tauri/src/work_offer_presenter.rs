use crate::command_guard::CommandSource;
use crate::commands::IpcResult;
use crate::snapshot::AppSnapshot;
use crate::ui_events::{PresenterId, UiEffect, UiEvent, UiTask};
use coosenpai_core::work::{WorkOffer, WorkOfferStatus};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkOfferProjection {
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer: Option<WorkOffer>,
    pub operation_generation: u64,
    pub busy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum WorkOfferAction {
    Accept,
    Decline,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkOfferInput {
    pub offer_id: String,
    pub operation_generation: u64,
    pub action: WorkOfferAction,
}

#[derive(Debug)]
pub(crate) enum WorkOfferEvent {
    Input {
        source: CommandSource,
        input: WorkOfferInput,
    },
    Completed {
        operation_generation: u64,
        result: Box<IpcResult<()>>,
    },
}

#[derive(Debug)]
pub(crate) struct WorkOfferTask {
    pub source: CommandSource,
    pub offer_id: String,
    pub conversation_generation: u64,
    pub operation_generation: u64,
    pub action: WorkOfferAction,
}

#[derive(Default)]
pub(crate) struct WorkOfferPresenter {
    projection: WorkOfferProjection,
    operation_generation: u64,
    active: Option<ActiveOperation>,
}

struct ActiveOperation {
    offer_id: String,
    conversation_generation: u64,
    generation: u64,
}

impl WorkOfferPresenter {
    pub(crate) fn observe(&mut self, snapshot: &AppSnapshot) -> WorkOfferProjection {
        if snapshot.revision < self.projection.revision {
            return self.projection.clone();
        }
        let offer = pending_offer(snapshot);
        let identity = offer
            .as_ref()
            .map(|offer| (offer.id.clone(), offer.conversation_generation));
        let current = self
            .projection
            .offer
            .as_ref()
            .map(|offer| (offer.id.clone(), offer.conversation_generation));
        if identity != current {
            self.invalidate();
        }
        self.projection.revision = snapshot.revision;
        self.projection.offer = offer;
        self.projection.operation_generation = self.operation_generation;
        self.projection.clone()
    }

    pub(crate) fn handle(&mut self, event: WorkOfferEvent) -> Vec<UiEffect> {
        match event {
            WorkOfferEvent::Input { source, input } => {
                if self.active.is_some()
                    || input.operation_generation != self.operation_generation
                    || self
                        .projection
                        .offer
                        .as_ref()
                        .is_none_or(|offer| offer.id != input.offer_id)
                {
                    return Vec::new();
                }
                let offer = self.projection.offer.as_ref().unwrap();
                self.operation_generation = self.operation_generation.saturating_add(1);
                self.active = Some(ActiveOperation {
                    offer_id: offer.id.clone(),
                    conversation_generation: offer.conversation_generation,
                    generation: self.operation_generation,
                });
                self.projection.operation_generation = self.operation_generation;
                self.projection.busy = true;
                self.projection.error = None;
                let task = WorkOfferTask {
                    source,
                    offer_id: offer.id.clone(),
                    conversation_generation: offer.conversation_generation,
                    operation_generation: self.operation_generation,
                    action: input.action,
                };
                let mut effects = vec![UiEffect::Spawn(UiTask::WorkOffer(task))];
                effects.extend(self.deliver());
                effects
            }
            WorkOfferEvent::Completed {
                operation_generation,
                result,
            } => {
                if self.active.as_ref().is_none_or(|active| {
                    active.generation != operation_generation
                        || self.projection.offer.as_ref().is_none_or(|offer| {
                            active.offer_id != offer.id
                                || active.conversation_generation != offer.conversation_generation
                        })
                }) {
                    return Vec::new();
                }
                self.active = None;
                self.projection.busy = false;
                self.projection.error = match *result {
                    IpcResult::Success { .. } => None,
                    IpcResult::Failure { error, .. } => Some(error.message),
                };
                self.deliver()
            }
        }
    }

    fn invalidate(&mut self) {
        self.operation_generation = self.operation_generation.saturating_add(1);
        self.active = None;
        self.projection.operation_generation = self.operation_generation;
        self.projection.busy = false;
        self.projection.error = None;
    }

    pub(crate) fn deliver(&self) -> Vec<UiEffect> {
        [PresenterId::Chat, PresenterId::Bubble]
            .into_iter()
            .map(|child| self.deliver_to(child))
            .collect()
    }

    pub(crate) fn deliver_to(&self, child: PresenterId) -> UiEffect {
        UiEffect::Deliver {
            child,
            event: UiEvent::WorkOfferProjection(self.projection.clone()),
        }
    }
}

pub(crate) async fn run(
    state: std::sync::Arc<crate::state::DesktopState>,
    task: WorkOfferTask,
) -> UiEvent {
    let operation_generation = task.operation_generation;
    let command = match task.action {
        WorkOfferAction::Accept => crate::command_guard::DesktopCommand::WorkOfferAccept,
        WorkOfferAction::Decline => crate::command_guard::DesktopCommand::WorkOfferDecline,
    };
    let result = crate::commands::dispatch_result(
        state.clone(),
        task.source,
        command,
        move |_| async move {
            let result = match task.action {
                WorkOfferAction::Accept => state
                    .core_runtime()
                    .accept_work_offer(&task.offer_id, task.conversation_generation)
                    .await
                    .map(|_| ()),
                WorkOfferAction::Decline => {
                    state
                        .core_runtime()
                        .decline_work_offer(&task.offer_id, task.conversation_generation)
                        .await
                }
            };
            match result {
                Ok(()) => IpcResult::success(()),
                Err(error) => IpcResult::failure(error.to_string()),
            }
        },
    )
    .await;
    UiEvent::WorkOffer(WorkOfferEvent::Completed {
        operation_generation,
        result: Box::new(result),
    })
}

fn pending_offer(snapshot: &AppSnapshot) -> Option<WorkOffer> {
    snapshot.work_offer.as_ref().and_then(|offer| {
        (offer.status == WorkOfferStatus::Proposed
            && offer.conversation_generation == snapshot.selected_conversation_generation
            && snapshot
                .conversation
                .iter()
                .any(|entry| entry.id == offer.entry_id))
        .then(|| offer.clone())
    })
}
