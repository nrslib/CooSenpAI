use crate::snapshot::AppSnapshot;
use crate::ui_events::{PresenterId, UiEffect, UiEvent, UiTask};
use crate::work::{WorkPhase, WorkSnapshot};
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
    Feedback {
        id: String,
        action: crate::conversation_feedback::FeedbackAction,
        value: Option<String>,
        revision: u64,
        generation: u64,
    },
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
    ProgressToggle {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<crate::status_presenter::UiText>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProgressHistoryEntry {
    pub at: String,
    pub text: crate::status_presenter::UiText,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProgressView {
    pub visible: bool,
    pub input_id: Option<String>,
    pub status: Option<crate::status_presenter::UiText>,
    pub started_at: Option<String>,
    pub history: Vec<ProgressHistoryEntry>,
    pub expanded: bool,
    pub can_cancel: bool,
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
    FeedbackCompleted {
        token: u64,
        generation: u64,
        result: Result<(), String>,
    },
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
    pub feedback: BTreeMap<String, crate::conversation_feedback::FeedbackRow>,
    pub scroll_request: u64,
    pub target: &'static str,
    pub entry_id: Option<String>,
    pub selected_id: Option<String>,
    pub thinking: bool,
    pub progress: ProgressView,
    pub work_input_id: Option<String>,
    pub operation_generation: u64,
    pub actions: BTreeMap<String, RowActions>,
    pub busy: bool,
    pub error: Option<String>,
}
#[derive(Default)]
pub(crate) struct ConversationPresenter {
    view: ConversationView,
    feedback: crate::conversation_feedback::ConversationFeedback,
    newest: Option<String>,
    tutorial_notice: bool,
    user_scrolled_up: bool,
    thinking_layout: Option<String>,
    active_response: bool,
    conversation_generation: Option<u64>,
    pending_sends: usize,
    next_token: u64,
    operation: Option<(u64, u64)>,
    work_snapshot: WorkSnapshot,
    progress_input_id: Option<String>,
    progress_generation: Option<u64>,
    progress_started_at: Option<String>,
    progress_history: Vec<ProgressHistoryEntry>,
    progress_seen_messages: std::collections::BTreeSet<String>,
    progress_work: Option<(WorkPhase, bool)>,
    progress_expanded: bool,
}
impl ConversationPresenter {
    pub(crate) fn observe(
        &mut self,
        snapshot: Arc<AppSnapshot>,
        pending_sends: usize,
    ) -> Vec<UiEffect> {
        if self
            .conversation_generation
            .is_some_and(|current| current != snapshot.selected_conversation_generation)
        {
            self.view.operation_generation += 1;
            self.view.error = None;
        }
        self.conversation_generation = Some(snapshot.selected_conversation_generation);
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
        self.feedback.observe(&snapshot);
        self.refresh_actions(Some(&snapshot));
        self.refresh_progress(&snapshot);
        vec![self.render()]
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.operation.is_some()
    }

    pub(crate) fn observe_work(
        &mut self,
        work: &WorkSnapshot,
        snapshot: Option<&Arc<AppSnapshot>>,
    ) -> Vec<UiEffect> {
        self.work_snapshot = work.clone();
        let Some(snapshot) = snapshot else {
            return vec![];
        };
        self.refresh_progress(snapshot);
        vec![self.render()]
    }

    pub(crate) fn handle(
        &mut self,
        event: ConversationEvent,
        snapshot: Option<&Arc<AppSnapshot>>,
    ) -> Vec<UiEffect> {
        match event {
            ConversationEvent::FeedbackCompleted {
                token,
                generation,
                result,
            } => self.feedback.complete(token, generation, result, snapshot),
            ConversationEvent::CancelCurrent => return self.cancel_current(snapshot),
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
                if snapshot
                    .is_none_or(|s| s.selected_conversation_generation != conversation_generation)
                {
                    self.refresh_actions(snapshot);
                    if let Some(snapshot) = snapshot {
                        self.refresh_progress(snapshot);
                    }
                    return vec![self.render(), UiEffect::Log("ui: presenter=Conversation event=Completed ignored=true reason=stale-conversation".into())];
                }
                match result {
                    Ok(completed) => {
                        self.view.error = None;
                        self.refresh_actions(Some(&completed));
                        self.refresh_progress(&completed);
                        return vec![
                            UiEffect::Deliver {
                                child: PresenterId::Chat,
                                event: UiEvent::ChatLoaded(completed),
                            },
                            self.render(),
                        ];
                    }
                    Err(error) => self.view.error = Some(error),
                }
                self.refresh_actions(snapshot);
                if let Some(snapshot) = snapshot {
                    self.refresh_progress(snapshot);
                }
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
                ConversationInput::Feedback {
                    id,
                    action,
                    value,
                    revision,
                    generation,
                } => {
                    let mut effects = self
                        .feedback
                        .input(id, action, value, revision, generation, snapshot);
                    effects.insert(0, self.render());
                    return effects;
                }
                ConversationInput::Action {
                    action,
                    input_id,
                    generation,
                } => return self.action(action, input_id, generation, snapshot),
                ConversationInput::ProgressToggle {
                    input_id,
                    generation,
                } => {
                    let current = snapshot.is_some_and(|snapshot| {
                        snapshot.active_user_message_id.as_deref() == Some(input_id.as_str())
                    });
                    if generation != self.view.operation_generation
                        || self.progress_input_id.as_deref() != Some(input_id.as_str())
                        || !current
                    {
                        return vec![UiEffect::Log(
                            "ui: presenter=Conversation event=ProgressToggle ignored=true reason=stale-or-unavailable".into(),
                        )];
                    }
                    self.progress_expanded = !self.progress_expanded;
                    return vec![self.render()];
                }
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
                    return self.cancel_current(snapshot);
                }
                _ => {}
            },
        }
        vec![self.render()]
    }

    fn cancel_current(&mut self, snapshot: Option<&Arc<AppSnapshot>>) -> Vec<UiEffect> {
        let Some(id) = snapshot.and_then(|s| s.active_user_message_id.clone()) else {
            return vec![self.render()];
        };
        self.action(
            ConversationAction::Cancel,
            id,
            self.view.operation_generation,
            snapshot,
        )
    }

    fn action(
        &mut self,
        action: ConversationAction,
        input_id: String,
        generation: u64,
        snapshot: Option<&Arc<AppSnapshot>>,
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
        let snapshot = snapshot.expect("row actions exist only after observing a snapshot");
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
        self.refresh_actions(Some(snapshot));
        self.refresh_progress(snapshot);
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

    fn refresh_progress(&mut self, snapshot: &Arc<AppSnapshot>) {
        let Some(input_id) = snapshot.active_user_message_id.as_deref() else {
            self.progress_input_id = None;
            self.progress_generation = None;
            self.progress_started_at = None;
            self.progress_history.clear();
            self.progress_seen_messages.clear();
            self.progress_work = None;
            self.progress_expanded = false;
            self.work_snapshot = WorkSnapshot::default();
            self.view.progress = if self.pending_sends > 0 {
                ProgressView {
                    visible: true,
                    status: Some(crate::status_presenter::UiText::message(
                        "conversation.progress.accepted",
                    )),
                    ..ProgressView::default()
                }
            } else {
                ProgressView::default()
            };
            return;
        };
        let conversation_generation = snapshot.selected_conversation_generation;
        if self.progress_input_id.as_deref() != Some(input_id)
            || self.progress_generation != Some(conversation_generation)
        {
            let started_at = snapshot
                .conversation
                .iter()
                .find(|entry| entry.id == input_id)
                .map(|entry| entry.created_at.clone())
                .unwrap_or_else(progress_now);
            self.progress_input_id = Some(input_id.to_owned());
            self.progress_generation = Some(conversation_generation);
            self.progress_started_at = Some(started_at.clone());
            self.progress_history.clear();
            self.progress_seen_messages.clear();
            self.progress_work = None;
            self.progress_expanded = false;
            self.work_snapshot = WorkSnapshot::default();
            self.add_progress_history(
                crate::status_presenter::UiText::message("conversation.progress.responseWait"),
                started_at,
            );
        }
        self.add_persisted_progress(snapshot, input_id);
        let work = self.work_snapshot.activity_for(input_id);
        if self.progress_work != work {
            if let Some((phase, approval_pending)) = work {
                self.add_progress_history(
                    work_history_text(phase, approval_pending),
                    progress_now(),
                );
            }
            self.progress_work = work;
        }
        let status = if snapshot.companion_draft.is_some() {
            crate::status_presenter::UiText::message("conversation.progress.responseGenerating")
        } else {
            match work {
                Some((WorkPhase::Running, true)) => {
                    crate::status_presenter::UiText::message("conversation.progress.approval")
                }
                Some((WorkPhase::Running, false)) => {
                    crate::status_presenter::UiText::message("conversation.progress.operation")
                }
                Some((WorkPhase::Cancelled, _)) => crate::status_presenter::UiText::message(
                    "conversation.progress.operationCancelled",
                ),
                Some((WorkPhase::Denied, _)) => crate::status_presenter::UiText::message(
                    "conversation.progress.operationDenied",
                ),
                Some((WorkPhase::Interrupted, _)) => crate::status_presenter::UiText::message(
                    "conversation.progress.operationInterrupted",
                ),
                Some((WorkPhase::Succeeded | WorkPhase::Failed, _)) | None => {
                    crate::status_presenter::UiText::message("conversation.progress.responseWait")
                }
            }
        };
        self.view.progress = ProgressView {
            visible: true,
            input_id: Some(input_id.to_owned()),
            status: Some(status),
            started_at: self.progress_started_at.clone(),
            history: self.progress_history.clone(),
            expanded: self.progress_expanded,
            can_cancel: self
                .view
                .actions
                .get(input_id)
                .is_some_and(|actions| actions.cancel),
        };
    }

    fn add_persisted_progress(&mut self, snapshot: &AppSnapshot, input_id: &str) {
        for entry in snapshot.conversation.iter().filter(|entry| {
            entry.role == ConversationRole::Companion
                && entry.message_kind
                    == Some(coosenpai_core::state::ConversationMessageKind::Progress)
                && entry.caused_by_ids.iter().any(|id| id == input_id)
        }) {
            if self.progress_seen_messages.insert(entry.id.clone()) {
                self.add_progress_history(
                    crate::status_presenter::UiText::literal(&entry.message),
                    entry.created_at.clone(),
                );
            }
        }
    }

    fn add_progress_history(&mut self, text: crate::status_presenter::UiText, at: String) {
        self.progress_history
            .push(ProgressHistoryEntry { at, text });
        self.progress_history
            .sort_by(|left, right| left.at.cmp(&right.at));
    }

    fn refresh_actions(&mut self, snapshot: Option<&Arc<AppSnapshot>>) {
        self.view.busy = self.is_busy();
        let Some(snapshot) = snapshot else {
            return;
        };
        let active = snapshot.active_user_message_id.as_deref();
        let terminal = snapshot
            .last_error
            .as_ref()
            .and_then(|error| error.terminal_user_input_id());
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
                let history_failure = snapshot.conversation.iter().rev().find_map(|failure| {
                    if !failure.is_response_failure()
                        || !failure.caused_by_ids.iter().any(|cause| cause == id)
                    {
                        return None;
                    }
                    let (attempts, provider) = failure.response_failure()?;
                    let kind = if provider.as_ref().is_some_and(|failure| {
                        failure.kind == coosenpai_core::provider::ProviderErrorKind::Timeout
                    }) {
                        coosenpai_core::runtime::RuntimeErrorKind::ProviderTimeout
                    } else {
                        coosenpai_core::runtime::RuntimeErrorKind::Provider
                    };
                    Some((
                        coosenpai_core::runtime::RuntimeUserResponseFailure {
                            input_id: id.to_owned(),
                            attempts,
                            provider,
                        },
                        kind,
                    ))
                });
                let current_failure = snapshot
                    .last_error
                    .as_ref()
                    .filter(|error| {
                        error.attachment_ocr.is_none()
                            && snapshot.companion_retry_in_seconds.is_none()
                    })
                    .and_then(|error| {
                        error
                            .user_response
                            .as_ref()
                            .filter(|failure| failure.input_id == id)
                            .map(|failure| (failure.clone(), error.kind))
                    });
                let user_failure = current_failure.or(history_failure);
                let cancelled = snapshot.cancelled_user_message_ids.contains(&entry.id);
                let failure = (!cancelled).then_some(user_failure.as_ref()).flatten().map(
                    |(failure, kind)| {
                        crate::status_presenter::user_response_failure_text(failure, *kind)
                    },
                );
                let terminal_for_row = terminal == Some(id) || user_failure.is_some();
                let row = RowActions {
                    failure,
                    cancel: available
                        && !cancelled
                        && (active == Some(id) || (terminal_for_row && recovery_idle)),
                    retry: can_send && !cancelled && terminal_for_row && recovery_idle,
                    resend: can_send
                        && active.is_none()
                        && !snapshot.user_work_pending
                        && cancelled
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
        let mut view = self.view.clone();
        view.progress.expanded = self.progress_expanded;
        view.feedback = self.feedback.rows.clone();
        UiEffect::ConversationRender(Box::new(view))
    }
}

fn progress_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn work_history_text(phase: WorkPhase, approval_pending: bool) -> crate::status_presenter::UiText {
    if approval_pending {
        return crate::status_presenter::UiText::message("conversation.progress.approval");
    }
    crate::status_presenter::UiText::message(match phase {
        WorkPhase::Running => "conversation.progress.operation",
        WorkPhase::Succeeded => "conversation.progress.operationSucceeded",
        WorkPhase::Failed => "conversation.progress.operationFailed",
        WorkPhase::Cancelled => "conversation.progress.operationCancelled",
        WorkPhase::Denied => "conversation.progress.operationDenied",
        WorkPhase::Interrupted => "conversation.progress.operationInterrupted",
    })
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

