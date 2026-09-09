use crate::snapshot::AppSnapshot;
use crate::ui_events::{PresenterId, UiEffect, UiEvent, UiTask};
use coosenpai_core::state::ConversationRole;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ConversationInput {
    Mounted,
    Opened,
    Layout,
    Wheel {
        delta: f64,
    },
    Scroll {
        previous_top: f64,
        top: f64,
        programmatic: bool,
    },
    Escape {
        composing: bool,
        key_code: u32,
    },
    Action {
        action: ConversationAction,
        input_id: String,
        generation: u64,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ConversationAction {
    Cancel,
    Retry,
    Resend,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RowActions {
    pub cancel: bool,
    pub retry: bool,
    pub resend: bool,
}

#[derive(Debug)]
pub(crate) struct ConversationTask {
    pub token: u64,
    pub conversation_generation: u64,
    pub input_id: String,
    pub action: ConversationAction,
    pub message: Option<String>,
}

#[derive(Debug)]
pub(crate) enum ConversationEvent {
    Input(ConversationInput),
    Selected(String),
    Sent,
    CancelCurrent,
    Completed {
        token: u64,
        conversation_generation: u64,
        result: Result<Arc<AppSnapshot>, String>,
    },
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationView {
    pub scroll_request: u64,
    pub target: &'static str,
    pub entry_id: Option<String>,
    pub selected_id: Option<String>,
    pub thinking: bool,
    pub work_input_id: Option<String>,
    pub operation_generation: u64,
    pub actions: BTreeMap<String, RowActions>,
    pub busy: bool,
    pub error: Option<String>,
}
#[derive(Default)]
pub(crate) struct ConversationPresenter {
    view: ConversationView,
    newest: Option<String>,
    tutorial_notice: bool,
    user_scrolled_up: bool,
    thinking_layout: Option<String>,
    active_response: bool,
    snapshot: Option<Arc<AppSnapshot>>,
    pending_sends: usize,
    next_token: u64,
    operation: Option<(u64, u64)>,
}
impl ConversationPresenter {
    pub(crate) fn observe(
        &mut self,
        snapshot: Arc<AppSnapshot>,
        pending_sends: usize,
    ) -> Vec<UiEffect> {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.revision > snapshot.revision)
        {
            return vec![];
        }
        if self.snapshot.as_ref().is_some_and(|current| {
            current.selected_conversation_generation != snapshot.selected_conversation_generation
        }) {
            self.view.operation_generation += 1;
            self.view.error = None;
        }
        self.pending_sends = pending_sends;
        let newest = snapshot.conversation.last();
        let next = newest.map(|e| e.id.clone());
        let changed = self.newest != next;
        let first = self.newest.is_none();
        self.newest = next;
        self.tutorial_notice = newest.is_some_and(|e| {
            e.role == coosenpai_core::state::ConversationRole::Companion
                && e.tutorial_response_key.is_some()
        });
        self.active_response = snapshot.active_user_message_id.is_some();
        self.view.work_input_id = (!snapshot.onboarding.tutorial_active)
            .then(|| snapshot.active_user_message_id.clone())
            .flatten();
        self.view.thinking = pending_sends > 0
            || (self.active_response
                && !snapshot
                    .last_error
                    .as_ref()
                    .and_then(|e| e.attachment_ocr.as_ref())
                    .is_some_and(|e| {
                        Some(&e.input_id) == snapshot.active_user_message_id.as_ref()
                    }));
        let layout = self
            .view
            .thinking
            .then(|| snapshot.companion_draft.clone().unwrap_or_default());
        let thinking_changed = layout.is_some() && layout != self.thinking_layout;
        self.thinking_layout = layout;
        if first || changed || (thinking_changed && !self.user_scrolled_up) {
            self.scroll();
        }
        self.snapshot = Some(snapshot);
        self.refresh_actions();
        vec![self.render()]
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.operation.is_some()
    }

    pub(crate) fn handle(&mut self, event: ConversationEvent) -> Vec<UiEffect> {
        match event {
            ConversationEvent::CancelCurrent => return self.cancel_current(),
            ConversationEvent::Completed {
                token,
                conversation_generation,
                result,
            } => {
                if self.operation != Some((token, conversation_generation)) {
                    return vec![UiEffect::Log("ui: presenter=Conversation event=Completed ignored=true reason=stale-token".into())];
                }
                self.operation = None;
                self.view.operation_generation += 1;
                if self
                    .snapshot
                    .as_ref()
                    .is_none_or(|s| s.selected_conversation_generation != conversation_generation)
                {
                    self.refresh_actions();
                    return vec![self.render(), UiEffect::Log("ui: presenter=Conversation event=Completed ignored=true reason=stale-conversation".into())];
                }
                match result {
                    Ok(snapshot) => {
                        self.view.error = None;
                        let mut effects = self.observe(snapshot.clone(), self.pending_sends);
                        effects.push(UiEffect::Deliver {
                            child: PresenterId::Chat,
                            event: UiEvent::ChatLoaded(snapshot),
                        });
                        self.refresh_actions();
                        effects.push(self.render());
                        return effects;
                    }
                    Err(error) => self.view.error = Some(error),
                }
                self.refresh_actions();
            }
            ConversationEvent::Selected(id) => {
                self.view.selected_id = Some(id.clone());
                self.view.scroll_request += 1;
                self.view.target = "entry-center";
                self.view.entry_id = Some(id);
                self.user_scrolled_up = false;
            }
            ConversationEvent::Sent => {
                self.view.thinking = true;
                self.scroll();
            }
            ConversationEvent::Input(input) => match input {
                ConversationInput::Action {
                    action,
                    input_id,
                    generation,
                } => return self.action(action, input_id, generation),
                ConversationInput::Mounted | ConversationInput::Opened => self.scroll(),
                ConversationInput::Layout if !self.user_scrolled_up => {
                    self.view.scroll_request += 1
                }
                ConversationInput::Wheel { delta } => {
                    if delta < 0.0 {
                        self.user_scrolled_up = true;
                    }
                }
                ConversationInput::Scroll {
                    previous_top,
                    top,
                    programmatic,
                } => {
                    if !programmatic && top < previous_top {
                        self.user_scrolled_up = true;
                    }
                }
                ConversationInput::Escape {
                    composing: false,
                    key_code,
                } if key_code != 229 && self.active_response => {
                    return self.cancel_current();
                }
                _ => {}
            },
        }
        vec![self.render()]
    }

    fn cancel_current(&mut self) -> Vec<UiEffect> {
        let Some(id) = self
            .snapshot
            .as_ref()
            .and_then(|s| s.active_user_message_id.clone())
        else {
            return vec![self.render()];
        };
        self.action(
            ConversationAction::Cancel,
            id,
            self.view.operation_generation,
        )
    }

    fn action(
        &mut self,
        action: ConversationAction,
        input_id: String,
        generation: u64,
    ) -> Vec<UiEffect> {
        let accepted = self
            .view
            .actions
            .get(&input_id)
            .is_some_and(|row| match action {
                ConversationAction::Cancel => row.cancel,
                ConversationAction::Retry => row.retry,
                ConversationAction::Resend => row.resend,
            });
        if generation != self.view.operation_generation
            || self.is_busy()
            || self.pending_sends > 0
            || !accepted
        {
            return vec![UiEffect::Log(
                "ui: presenter=Conversation event=Action ignored=true reason=stale-or-unavailable"
                    .into(),
            )];
        }
        let snapshot = self.snapshot.as_ref().unwrap();
        let message = (action == ConversationAction::Resend).then(|| {
            snapshot
                .conversation
                .iter()
                .find(|entry| entry.id == input_id)
                .unwrap()
                .message
                .clone()
        });
        let conversation_generation = snapshot.selected_conversation_generation;
        self.next_token += 1;
        self.operation = Some((self.next_token, conversation_generation));
        self.view.operation_generation += 1;
        self.view.error = None;
        self.refresh_actions();
        vec![
            self.render(),
            UiEffect::Spawn(UiTask::Conversation(ConversationTask {
                token: self.next_token,
                conversation_generation,
                input_id,
                action,
                message,
            })),
        ]
    }

    fn refresh_actions(&mut self) {
        self.view.busy = self.is_busy();
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let active = snapshot.active_user_message_id.as_deref();
        let terminal = snapshot
            .last_error
            .as_ref()
            .and_then(|error| error.attachment_ocr.as_ref())
            .filter(|failure| !failure.retryable)
            .map(|failure| failure.input_id.as_str());
        let available =
            !self.view.busy && self.pending_sends == 0 && !snapshot.onboarding.setup_required;
        let can_send = available
            && (!snapshot.onboarding.tutorial_active || snapshot.onboarding.chat_input_enabled);
        let actions = snapshot
            .conversation
            .iter()
            .filter(|entry| entry.role == ConversationRole::User)
            .map(|entry| {
                let id = entry.id.as_str();
                let recovery_idle = active.is_none() && !snapshot.user_work_pending;
                let row = RowActions {
                    cancel: available
                        && (active == Some(id) || (terminal == Some(id) && recovery_idle)),
                    retry: can_send && terminal == Some(id) && recovery_idle,
                    resend: can_send
                        && active.is_none()
                        && !snapshot.user_work_pending
                        && snapshot.cancelled_user_message_ids.contains(&entry.id)
                        && !entry.message.trim().is_empty(),
                };
                (entry.id.clone(), row)
            })
            .collect();
        if self.view.actions != actions {
            self.view.operation_generation += 1;
            self.view.actions = actions;
        }
    }
    fn scroll(&mut self) {
        self.user_scrolled_up = false;
        self.view.scroll_request += 1;
        self.view.target = if self.tutorial_notice {
            "entry-start"
        } else {
            "bottom"
        };
        self.view.entry_id = self.tutorial_notice.then(|| self.newest.clone()).flatten();
    }
    pub(crate) fn render(&self) -> UiEffect {
        UiEffect::ConversationRender(Box::new(self.view.clone()))
    }
}

pub(crate) async fn run(state: Arc<crate::state::DesktopState>, task: ConversationTask) -> UiEvent {
    use crate::command_guard::{CommandSource, DesktopCommand, DispatchError};
    let ConversationTask {
        token,
        conversation_generation,
        input_id,
        action,
        message,
    } = task;
    let command = match action {
        ConversationAction::Cancel => DesktopCommand::ChatCancel,
        ConversationAction::Retry => DesktopCommand::ChatRetry,
        ConversationAction::Resend => DesktopCommand::ChatSend,
    };
    let handler = state.clone();
    let result = state
        .dispatch(CommandSource::IpcMain, command, move |context| async move {
            if handler.snapshot().await.selected_conversation_generation != conversation_generation
            {
                return Err(DispatchError::handler(
                    "会話が切り替わったため操作を取り消しました",
                ));
            }
            match action {
                ConversationAction::Cancel => handler
                    .core_runtime()
                    .cancel_user_message_for(input_id)
                    .await
                    .map_err(|error| DispatchError::handler(error.to_string())),
                ConversationAction::Retry => handler
                    .core_runtime()
                    .retry_user_message_for(input_id)
                    .await
                    .map_err(|error| DispatchError::handler(error.to_string())),
                ConversationAction::Resend => handler
                    .command_enqueue_user_message(
                        &context,
                        message.expect("Presenter supplies resend text"),
                        Vec::new(),
                        crate::state::user_input::UserMessageAttachment::None,
                    )
                    .await
                    .map_err(DispatchError::handler),
            }
        })
        .await;
    let result =
        match result {
            Ok(_) => Ok(Arc::new(state.snapshot().await)),
            Err(error) => Err(error.format_for_locale(
                coosenpai_core::locale::Locale::from_config(&state.runtime_config().ui.language),
            )),
        };
    UiEvent::Conversation(ConversationEvent::Completed {
        token,
        conversation_generation,
        result,
    })
}

