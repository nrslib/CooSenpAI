use super::{RootStatus, WorkKind};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalMode {
    #[default]
    Manual,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalStatus {
    AwaitingUser,
    Reviewing,
    Approved,
    Denied,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub id: String,
    pub task_id: String,
    pub kind: WorkKind,
    pub target: String,
    pub root: RootStatus,
    pub reason: String,
    pub status: ApprovalStatus,
    pub decision_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

struct State {
    mode: ApprovalMode,
    revision: u64,
    request: Option<ApprovalRequest>,
    auto_approved: bool,
    operation: Option<CancellationToken>,
}

#[derive(Clone)]
pub struct WorkApprovals {
    state: Arc<Mutex<State>>,
    changes: watch::Sender<u64>,
    journal: std::path::PathBuf,
}

impl WorkApprovals {
    pub fn new(mode: ApprovalMode, journal: std::path::PathBuf) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                mode,
                revision: 0,
                request: None,
                auto_approved: false,
                operation: None,
            })),
            changes: watch::channel(0).0,
            journal,
        }
    }

    pub fn snapshot(&self) -> Option<ApprovalRequest> {
        self.state.lock().expect("work approvals").request.clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    pub fn mode(&self) -> ApprovalMode {
        self.state.lock().expect("work approvals").mode
    }

    pub fn set_mode(&self, mode: ApprovalMode) {
        let mut state = self.state.lock().expect("work approvals");
        if state.mode == mode {
            return;
        }
        let revoke_auto = mode == ApprovalMode::Manual && state.auto_approved;
        state.auto_approved = false;
        if revoke_auto {
            if let Some(operation) = state.operation.take() {
                operation.cancel();
            }
        }
        state.mode = mode;
        state.revision += 1;
        if let Some(request) = &mut state.request {
            if matches!(
                request.status,
                ApprovalStatus::AwaitingUser | ApprovalStatus::Reviewing
            ) || (revoke_auto && request.status == ApprovalStatus::Approved)
            {
                request.status = ApprovalStatus::AwaitingUser;
                request.decision_reason = None;
                if self.persist(request).is_err() {
                    request.decision_reason =
                        Some("承認記録を保存できません。操作は実行していません".into());
                }
            }
        }
        self.changes.send_replace(state.revision);
    }

    pub fn request(
        &self,
        task_id: &str,
        kind: WorkKind,
        target: &str,
        root: RootStatus,
        reason: &str,
    ) -> Result<String, String> {
        let mut state = self.state.lock().expect("work approvals");
        if state.request.as_ref().is_some_and(|r| {
            matches!(
                r.status,
                ApprovalStatus::AwaitingUser | ApprovalStatus::Reviewing
            )
        }) {
            return Err("別の操作が承認待ちです".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let request = ApprovalRequest {
            id: id.clone(),
            task_id: task_id.into(),
            kind,
            target: target.into(),
            root,
            reason: reason.into(),
            status: ApprovalStatus::AwaitingUser,
            decision_reason: None,
        };
        self.persist(&request)?;
        state.auto_approved = false;
        state.request = Some(request);
        state.revision += 1;
        self.changes.send_replace(state.revision);
        Ok(id)
    }

    pub fn decide(&self, id: &str, decision: ApprovalDecision) -> Result<(), String> {
        let mut state = self.state.lock().expect("work approvals");
        let mut request = state
            .request
            .as_ref()
            .filter(|r| r.id == id)
            .ok_or("承認要求が見つかりません")?
            .clone();
        if !matches!(
            request.status,
            ApprovalStatus::AwaitingUser | ApprovalStatus::Reviewing
        ) {
            return Err("この承認要求は既に終了しています".into());
        }
        request.status = match decision {
            ApprovalDecision::Allow => ApprovalStatus::Approved,
            ApprovalDecision::Deny => ApprovalStatus::Denied,
        };
        request.decision_reason = Some("ユーザーの判断".into());
        self.persist(&request)?;
        state.auto_approved = false;
        state.request = Some(request);
        state.revision += 1;
        self.changes.send_replace(state.revision);
        Ok(())
    }

    pub fn cancel(&self, task_id: &str) {
        self.end(task_id, true);
    }

    pub fn finish(&self, task_id: &str) {
        self.end(task_id, false);
    }

    fn end(&self, task_id: &str, cancel_approved: bool) {
        let mut state = self.state.lock().expect("work approvals");
        let Some(request) = state.request.as_mut().filter(|r| r.task_id == task_id) else {
            return;
        };
        if matches!(
            request.status,
            ApprovalStatus::AwaitingUser | ApprovalStatus::Reviewing
        ) || (cancel_approved && request.status == ApprovalStatus::Approved)
        {
            request.status = ApprovalStatus::Cancelled;
            request.decision_reason =
                Some("作業を停止しました。送信済みの資料は取り消せません".into());
            if self.persist(request).is_err() {
                request.decision_reason =
                    Some("作業は停止しましたが、承認記録を保存できません".into());
            }
        }
        state.auto_approved = false;
        if let Some(operation) = state.operation.take() {
            operation.cancel();
        }
        state.revision += 1;
        self.changes.send_replace(state.revision);
    }

    pub(super) fn begin_call(
        &self,
        id: &str,
        parent: &CancellationToken,
    ) -> Result<CancellationToken, String> {
        let mut state = self.state.lock().expect("work approvals");
        if parent.is_cancelled()
            || !state
                .request
                .as_ref()
                .is_some_and(|r| r.id == id && r.status == ApprovalStatus::Approved)
        {
            return Err("承認状態が変更されたため、操作を停止しました".into());
        }
        let token = parent.child_token();
        state.operation = Some(token.clone());
        Ok(token)
    }

    pub(super) fn run_authorized<T>(
        &self,
        id: &str,
        cancellation: &CancellationToken,
        operation: impl FnOnce() -> T,
    ) -> Result<T, String> {
        let state = self.state.lock().expect("work approvals");
        if cancellation.is_cancelled()
            || !state
                .request
                .as_ref()
                .is_some_and(|r| r.id == id && r.status == ApprovalStatus::Approved)
        {
            return Err("承認状態が変更されたため、操作を停止しました".into());
        }
        Ok(operation())
    }

    pub(super) async fn wait(
        &self,
        id: &str,
        reviewer: &dyn super::execution::ApprovalReviewer,
        cancellation: CancellationToken,
    ) -> Result<(), String> {
        let mut changes = self.changes.subscribe();
        loop {
            if cancellation.is_cancelled() {
                return Err("作業を停止しました".into());
            }
            let (request, mode, revision) = {
                let state = self.state.lock().expect("work approvals");
                let request = state
                    .request
                    .as_ref()
                    .filter(|r| r.id == id)
                    .ok_or("承認要求が失効しました")?
                    .clone();
                (request, state.mode, state.revision)
            };
            match request.status {
                ApprovalStatus::Approved => return Ok(()),
                ApprovalStatus::Denied => return Err("操作が拒否されました".into()),
                ApprovalStatus::Cancelled => return Err("作業を停止しました".into()),
                // 許可ルート外はAuto審査に委ねず、人の判断を待つ。
                ApprovalStatus::AwaitingUser
                    if mode == ApprovalMode::Auto
                        && request.root.is_inside()
                        && request.decision_reason.is_none() =>
                {
                    {
                        let mut state = self.state.lock().expect("work approvals");
                        if state.revision != revision {
                            continue;
                        }
                        state.request.as_mut().expect("request").status = ApprovalStatus::Reviewing;
                        self.changes.send_replace(state.revision);
                        // 自分の Reviewing 通知で審査を中断しない。外部変更はロック解放後に受け取る。
                        changes.borrow_and_update();
                    }
                    let review_cancel = cancellation.child_token();
                    let review = reviewer.review(&request, review_cancel.clone());
                    tokio::pin!(review);
                    let result = tokio::select! {
                        result = &mut review => Some(result),
                        _ = changes.changed() => None,
                        _ = cancellation.cancelled() => None,
                    };
                    review_cancel.cancel();
                    let Some(result) = result else {
                        let _ = review.await;
                        continue;
                    };
                    if cancellation.is_cancelled() {
                        self.cancel(&request.task_id);
                        return Err("作業を停止しました".into());
                    }
                    let mut state = self.state.lock().expect("work approvals");
                    if state.revision != revision {
                        continue;
                    }
                    let mut request = state.request.as_ref().expect("request").clone();
                    match result {
                        Ok((status, reason)) => {
                            request.status = status;
                            request.decision_reason = Some(reason);
                        }
                        Err(_) => {
                            request.status = ApprovalStatus::AwaitingUser;
                            request.decision_reason = Some(
                                "AI審査を完了できませんでした。操作内容を確認してください".into(),
                            );
                        }
                    }
                    if self.persist(&request).is_err() {
                        request.status = ApprovalStatus::AwaitingUser;
                        request.decision_reason =
                            Some("承認記録を保存できません。操作は実行していません".into());
                    }
                    state.auto_approved = request.status == ApprovalStatus::Approved;
                    state.request = Some(request);
                    state.revision += 1;
                    self.changes.send_replace(state.revision);
                    changes.borrow_and_update();
                    continue;
                }
                _ => {}
            }
            tokio::select! {
                _ = changes.changed() => {},
                _ = cancellation.cancelled() => return Err("作業を停止しました".into()),
            }
        }
    }

    fn persist(&self, request: &ApprovalRequest) -> Result<(), String> {
        crate::persistence::atomic_write_json(&self.journal, request)
            .map_err(|_| "承認記録を保存できません".into())
    }
}
