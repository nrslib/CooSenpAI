use crate::commands::IpcResult;
use crate::snapshot::AppSnapshot;
use crate::ui_events::{UiEffect, UiEvent, UiTask};
use crate::work::WorkSnapshot;
use coosenpai_core::config::Config;
use coosenpai_core::work::{
    AllowedRoot, ApprovalDecision, ApprovalMode, ApprovalRequest, ApprovalStatus, WorkConfig,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum WorkApprovalInput {
    Mounted {
        input_id: String,
    },
    Unmounted {
        input_id: String,
    },
    Action {
        input_id: String,
        approval_id: Option<String>,
        action: WorkAction,
    },
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WorkAction {
    Allow,
    Deny,
    AddRootAndAllow,
    ToggleMode,
    Cancel,
}
#[derive(Debug)]
pub(crate) enum WorkApprovalEvent {
    Input(WorkApprovalInput),
    Changed(Box<WorkSnapshot>),
    Decided {
        generation: u64,
        result: Box<IpcResult<()>>,
    },
    Configured {
        generation: u64,
        result: Box<IpcResult<Config>>,
    },
}
#[derive(Debug)]
pub(crate) enum WorkApprovalTask {
    Decide {
        generation: u64,
        id: String,
        decision: ApprovalDecision,
    },
    Configure {
        generation: u64,
        patch: serde_json::Value,
        revision: u64,
    },
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkApprovalView {
    pub input_id: Option<String>,
    pub visible: bool,
    pub approval: Option<ApprovalRequest>,
    pub awaiting: bool,
    pub busy: bool,
    pub manual: bool,
    pub error: Option<String>,
    pub layout_request: u64,
}
#[derive(Default)]
pub(crate) struct WorkApprovalPresenter {
    view: WorkApprovalView,
    generation: u64,
    snapshot: WorkSnapshot,
    config: WorkConfig,
    config_revision: u64,
    allow_after_save: Option<String>,
}
impl WorkApprovalPresenter {
    pub(crate) fn observe(&mut self, snapshot: &AppSnapshot) -> Vec<UiEffect> {
        self.config = snapshot.config.work.clone();
        self.config_revision = snapshot.config_revision;
        let manual = self.config.approval_mode == ApprovalMode::Manual;
        let changed = self.view.manual != manual;
        self.view.manual = manual;
        if changed && self.view.input_id.is_some() {
            vec![self.render()]
        } else {
            vec![]
        }
    }
    pub(crate) fn handle(&mut self, event: WorkApprovalEvent) -> Vec<UiEffect> {
        let mut effects = match event {
            WorkApprovalEvent::Input(WorkApprovalInput::Mounted { input_id }) => {
                if self.view.input_id.as_ref() == Some(&input_id) {
                    return vec![self.render()];
                }
                self.generation += 1;
                self.view = WorkApprovalView {
                    input_id: Some(input_id),
                    manual: self.config.approval_mode == ApprovalMode::Manual,
                    ..Default::default()
                };
                self.allow_after_save = None;
                self.update_view();
                vec![]
            }
            WorkApprovalEvent::Input(WorkApprovalInput::Unmounted { input_id }) => {
                if self.view.input_id.as_ref() != Some(&input_id) {
                    return vec![];
                }
                self.generation += 1;
                self.view = WorkApprovalView::default();
                self.allow_after_save = None;
                vec![]
            }
            WorkApprovalEvent::Input(WorkApprovalInput::Action {
                input_id,
                approval_id,
                action,
            }) => {
                if self.view.input_id.as_ref() != Some(&input_id) || self.view.busy {
                    return vec![];
                }
                if matches!(action, WorkAction::Cancel) {
                    let (reply, _) = tokio::sync::oneshot::channel();
                    return vec![UiEffect::Deliver {
                        child: crate::ui_events::PresenterId::Chat,
                        event: UiEvent::UserCommand(crate::ui_commands::UserCommand::ChatCancel(
                            reply,
                        )),
                    }];
                }
                if !self.view.visible
                    || !self.view.awaiting
                    || self.view.approval.as_ref().map(|a| &a.id) != approval_id.as_ref()
                {
                    return vec![];
                }
                let Some(approval) = self.view.approval.clone() else {
                    return vec![];
                };
                self.generation += 1;
                self.view.busy = true;
                self.view.error = None;
                let task = match action {
                    WorkAction::Allow | WorkAction::Deny => WorkApprovalTask::Decide {
                        generation: self.generation,
                        id: approval.id,
                        decision: if matches!(action, WorkAction::Allow) {
                            ApprovalDecision::Allow
                        } else {
                            ApprovalDecision::Deny
                        },
                    },
                    WorkAction::ToggleMode => WorkApprovalTask::Configure {
                        generation: self.generation,
                        revision: self.config_revision,
                        patch: serde_json::json!({"work":{"approvalMode":if self.view.manual {"auto"} else {"manual"}}}),
                    },
                    WorkAction::AddRootAndAllow => {
                        let mut roots = self.config.allowed_roots.clone();
                        if let Some(root) = roots
                            .iter_mut()
                            .find(|root| root.path == std::path::Path::new(&approval.target))
                        {
                            root.read = true;
                            root.write |= approval.kind.requires_write();
                        } else {
                            roots.push(AllowedRoot {
                                path: approval.target.into(),
                                read: true,
                                write: approval.kind.requires_write(),
                            });
                        }
                        self.allow_after_save = Some(approval.id);
                        WorkApprovalTask::Configure {
                            generation: self.generation,
                            revision: self.config_revision,
                            patch: serde_json::json!({"work":{"allowedRoots":roots}}),
                        }
                    }
                    WorkAction::Cancel => unreachable!(),
                };
                vec![spawn(task)]
            }
            WorkApprovalEvent::Changed(snapshot) => {
                if self.snapshot == *snapshot {
                    return vec![];
                }
                self.snapshot = *snapshot;
                if self.view.input_id.is_none() {
                    return vec![];
                }
                self.update_view();
                vec![]
            }
            WorkApprovalEvent::Decided { generation, result } => {
                if generation != self.generation || self.view.input_id.is_none() {
                    return vec![];
                }
                self.view.busy = false;
                match *result {
                    IpcResult::Success { .. } => self.view.error = None,
                    IpcResult::Failure { error, .. } => self.view.error = Some(error.message),
                }
                vec![]
            }
            WorkApprovalEvent::Configured { generation, result } => {
                if generation != self.generation || self.view.input_id.is_none() {
                    return vec![];
                }
                match *result {
                    IpcResult::Success { value, .. } => {
                        self.config = value.work;
                        self.view.manual = self.config.approval_mode == ApprovalMode::Manual;
                        if let Some(id) = self.allow_after_save.take() {
                            vec![spawn(WorkApprovalTask::Decide {
                                generation,
                                id,
                                decision: ApprovalDecision::Allow,
                            })]
                        } else {
                            self.view.busy = false;
                            vec![]
                        }
                    }
                    IpcResult::Failure { error, .. } => {
                        self.allow_after_save = None;
                        self.view.busy = false;
                        self.view.error = Some(error.message);
                        vec![]
                    }
                }
            }
        };
        effects.insert(0, self.render());
        effects
    }

    fn update_view(&mut self) {
        let (visible, approval, error) = self.snapshot.approval_view(
            self.view
                .input_id
                .as_deref()
                .expect("mounted approval input"),
        );
        if self.view.approval != approval || self.view.visible != visible {
            self.view.layout_request += 1;
        }
        self.view.visible = visible;
        self.view.approval = approval;
        if error.is_some() {
            self.view.error = error;
        }
        self.view.awaiting = self.view.approval.as_ref().is_some_and(|approval| {
            matches!(
                approval.status,
                ApprovalStatus::AwaitingUser | ApprovalStatus::Reviewing
            )
        });
    }

    fn render(&self) -> UiEffect {
        UiEffect::WorkApprovalRender(Box::new(self.view.clone()))
    }
}
fn spawn(task: WorkApprovalTask) -> UiEffect {
    UiEffect::Spawn(UiTask::WorkApproval(task))
}
pub(crate) async fn run(
    state: std::sync::Arc<crate::state::DesktopState>,
    task: WorkApprovalTask,
) -> UiEvent {
    let event = match task {
        WorkApprovalTask::Configure {
            generation,
            patch,
            revision,
        } => WorkApprovalEvent::Configured {
            generation,
            result: Box::new(
                crate::commands::update_config_for_source(
                    state,
                    patch,
                    None,
                    Some(revision),
                    crate::command_guard::CommandSource::IpcMain,
                )
                .await,
            ),
        },
        WorkApprovalTask::Decide {
            generation,
            id,
            decision,
        } => {
            let worker = state.clone();
            let result = crate::commands::dispatch_result(
                state,
                crate::command_guard::CommandSource::IpcMain,
                crate::command_guard::DesktopCommand::WorkApprove,
                move |_| async move {
                    match worker.work.approvals.decide(&id, decision) {
                        Ok(()) => IpcResult::success(()),
                        Err(error) => IpcResult::failure(error),
                    }
                },
            )
            .await;
            WorkApprovalEvent::Decided {
                generation,
                result: Box::new(result),
            }
        }
    };
    UiEvent::WorkApproval(event)
}

