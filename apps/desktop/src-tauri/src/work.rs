use crate::factory::DesktopRuntimeFactory;
use coosenpai_core::persistence::atomic_write_json;
use coosenpai_core::process::TokioProcessRunner;
use coosenpai_core::work::{
    execute, AllowedRoot, ApprovalMode, ApprovalRequest, Harness, RootPolicy, RootStatus,
    WorkExecution, WorkRequest, WorkResult, WORK_TIME_LIMIT,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WorkPhase {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

/// 実行ごとの記録。cwd、許可ルート、承認の種類、出力の要約、変更ファイルを残す。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkRecord {
    id: String,
    phase: WorkPhase,
    request: WorkRequest,
    harness: Harness,
    allowed_roots: Vec<AllowedRoot>,
    resolved_cwd: Option<PathBuf>,
    root: Option<RootStatus>,
    answer: Option<String>,
    changed_files: Vec<String>,
    stderr_summary: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkSnapshot {
    task: Option<WorkRecord>,
    approval: Option<ApprovalRequest>,
    error: Option<String>,
}

impl WorkSnapshot {
    pub(crate) fn approval_view(
        &self,
        input_id: &str,
    ) -> (bool, Option<ApprovalRequest>, Option<String>) {
        let visible = self
            .task
            .as_ref()
            .is_some_and(|task| task.id == input_id && task.phase == WorkPhase::Running);
        let approval = self
            .approval
            .clone()
            .filter(|approval| visible && approval.task_id == input_id);
        (visible, approval, self.error.clone())
    }
}

struct State {
    record: Option<WorkRecord>,
    active: Option<CancellationToken>,
    worker: Option<tokio::task::JoinHandle<()>>,
    roots: Vec<AllowedRoot>,
}

pub(crate) struct WorkController {
    state: Mutex<State>,
    path: PathBuf,
    home: PathBuf,
    pub approvals: coosenpai_core::work::WorkApprovals,
    completion: watch::Sender<bool>,
    load_error: Option<String>,
}

impl WorkController {
    pub fn new(path: PathBuf, home: PathBuf, mode: ApprovalMode) -> Self {
        let (record, load_error) = match Self::restore(&path) {
            Ok(record) => (record, None),
            Err(error) => (None, Some(error)),
        };
        let approvals =
            coosenpai_core::work::WorkApprovals::new(mode, path.with_file_name("approval.json"));
        Self {
            state: Mutex::new(State {
                record,
                active: None,
                worker: None,
                roots: vec![],
            }),
            path,
            home,
            approvals,
            completion: watch::channel(true).0,
            load_error,
        }
    }

    pub fn set_roots(&self, roots: Vec<AllowedRoot>) {
        self.state.lock().expect("work state").roots = roots;
    }

    pub fn roots(&self) -> Vec<AllowedRoot> {
        self.state.lock().expect("work state").roots.clone()
    }

    fn restore(path: &std::path::Path) -> Result<Option<WorkRecord>, String> {
        use std::io::Read;
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err("作業記録を読み取れません。作業機能を停止しています".into()),
        };
        let mut bytes = Vec::new();
        file.take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "作業記録を読み取れません")?;
        if bytes.len() > 256 * 1024 {
            return Err("保存された作業記録が大きすぎます。作業機能を停止しています".into());
        }
        let mut record: WorkRecord = serde_json::from_slice(&bytes)
            .map_err(|_| "作業記録が破損しています。作業機能を停止しています")?;
        if record.phase == WorkPhase::Running {
            record.phase = WorkPhase::Interrupted;
            record.error = Some("前回の作業は中断されました。自動で再実行していません".into());
            atomic_write_json(path, &record).map_err(|_| "作業記録を保存できません")?;
        }
        Ok(Some(record))
    }

    pub fn snapshot(&self) -> WorkSnapshot {
        WorkSnapshot {
            task: self.state.lock().expect("work state").record.clone(),
            approval: self.approvals.snapshot(),
            error: self.load_error.clone(),
        }
    }

    fn start(
        self: &Arc<Self>,
        input_id: &str,
        request: WorkRequest,
        harness: Harness,
        factory: Arc<DesktopRuntimeFactory>,
        parent: CancellationToken,
    ) -> Result<WorkSnapshot, String> {
        if let Some(error) = &self.load_error {
            return Err(error.clone());
        }
        if request.brief.proposal.trim().is_empty() || request.cwd.as_os_str().len() > 4096 {
            return Err("作業ディレクトリと作業内容を確認してください".into());
        }
        let mut state = self.state.lock().expect("work state");
        if state.active.is_some() {
            return Err("先に実行中の作業を停止してください".into());
        }
        if parent.is_cancelled() {
            return Err("終了処理中です".into());
        }
        if input_id.is_empty()
            || input_id.len() > 128
            || !input_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("会話入力IDが不正です".into());
        }
        let id = input_id.to_owned();
        if let Some(record) = Self::restore(&self.task_path(&id))? {
            if record.id != id || record.request != request {
                return Err(
                    "同じ会話入力の作業内容が変わりました。新しいメッセージで依頼してください"
                        .into(),
                );
            }
            return Ok(WorkSnapshot {
                task: Some(record),
                approval: None,
                error: None,
            });
        }
        let roots = state.roots.clone();
        let record = WorkRecord {
            id: id.clone(),
            phase: WorkPhase::Running,
            request: request.clone(),
            harness,
            allowed_roots: roots.clone(),
            resolved_cwd: None,
            root: None,
            answer: None,
            changed_files: vec![],
            stderr_summary: None,
            error: None,
        };
        self.save_record(&record)?;
        let cancellation = parent.child_token();
        state.active = Some(cancellation.clone());
        state.record = Some(record.clone());
        self.completion.send_replace(false);
        let controller = self.clone();
        let policy = RootPolicy::new(self.home.clone(), roots);
        state.worker = Some(tokio::spawn(async move {
            let timeout_token = cancellation.clone();
            let timer = tokio::spawn(async move {
                tokio::time::sleep(WORK_TIME_LIMIT).await;
                timeout_token.cancel();
            });
            let result = match factory.work_session(harness, cancellation.clone()).await {
                Ok(session) => {
                    let (bridge, reviewer) = match session.reviewer {
                        Some((bridge, reviewer)) => (Some(bridge), Some(reviewer)),
                        None => (None, None),
                    };
                    let result = execute(
                        &id,
                        request,
                        WorkExecution {
                            approvals: &controller.approvals,
                            policy: &policy,
                            launch: session.launch,
                            runner: &TokioProcessRunner,
                            reviewer,
                        },
                        cancellation.clone(),
                    )
                    .await;
                    if let Some(bridge) = bridge {
                        bridge.shutdown().await;
                    }
                    result
                }
                Err(error) => Err(error.to_string()),
            };
            timer.abort();
            controller.finish(&id, result);
        }));
        Ok(WorkSnapshot {
            task: Some(record),
            approval: None,
            error: None,
        })
    }

    fn finish(&self, id: &str, result: Result<WorkResult, String>) {
        let mut state = self.state.lock().expect("work state");
        let cancelled = state
            .active
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled);
        let Some(record) = state.record.as_mut().filter(|r| r.id == id) else {
            return;
        };
        if cancelled {
            self.approvals.cancel(id);
            record.phase = WorkPhase::Cancelled;
            record.error = Some(
                "作業を停止しました（手動停止または15分の上限）。ハーネスに送信済みの内容は取り消せません"
                    .into(),
            );
        } else {
            match result {
                Ok(result) => {
                    record.phase = WorkPhase::Succeeded;
                    record.answer = Some(result.answer);
                    record.resolved_cwd = Some(result.cwd);
                    record.root = Some(result.root);
                    record.changed_files = result.changed_files;
                    record.stderr_summary = Some(result.stderr_summary);
                }
                Err(error) => {
                    record.phase = WorkPhase::Failed;
                    record.error = Some(error);
                }
            }
        }
        if self.save_record(record).is_err() {
            record.phase = WorkPhase::Failed;
            record.error = Some("作業結果を保存できませんでした".into());
        }
        self.approvals.finish(id);
        state.active = None;
        self.completion.send_replace(true);
    }

    fn task_path(&self, id: &str) -> PathBuf {
        self.path.with_file_name(format!("{id}.json"))
    }

    fn save_record(&self, record: &WorkRecord) -> Result<(), String> {
        atomic_write_json(&self.task_path(&record.id), record)
            .and_then(|_| atomic_write_json(&self.path, record))
            .map_err(|_| "作業記録を保存できません".into())
    }

    pub fn stop(&self, id: &str) -> Result<(), String> {
        let state = self.state.lock().expect("work state");
        if state.record.as_ref().is_none_or(|r| r.id != id) {
            return Err("作業が見つかりません".into());
        }
        if let Some(token) = &state.active {
            token.cancel();
            self.approvals.cancel(id);
        }
        Ok(())
    }

    async fn await_completion(&self, id: &str) -> Result<(), String> {
        let mut completion = self.completion.subscribe();
        while !*completion.borrow_and_update() {
            completion
                .changed()
                .await
                .map_err(|_| "作業の終了状態を取得できません")?;
        }
        let worker = {
            let mut state = self.state.lock().expect("work state");
            if state.record.as_ref().is_some_and(|record| record.id == id) {
                state.worker.take()
            } else {
                None
            }
        };
        if let Some(worker) = worker {
            worker.await.map_err(|_| "作業の終了処理に失敗しました")?;
        }
        Ok(())
    }

    pub async fn shutdown(&self) {
        let mut completion = self.completion.subscribe();
        {
            let state = self.state.lock().expect("work state");
            if let Some(token) = &state.active {
                token.cancel();
                if let Some(record) = &state.record {
                    self.approvals.cancel(&record.id);
                }
            }
        }
        while !*completion.borrow_and_update() {
            if completion.changed().await.is_err() {
                break;
            }
        }
        let worker = self.state.lock().expect("work state").worker.take();
        if let Some(worker) = worker {
            let _ = worker.await;
        }
    }
}

pub(crate) struct DesktopChatWork {
    pub controller: Arc<WorkController>,
    pub factory: Arc<DesktopRuntimeFactory>,
    /// 会話用providerから境界で決めたハーネス。実行層は設定を読まない。
    pub harness: Harness,
}

fn recorded_result(record: WorkRecord) -> Result<WorkResult, String> {
    match (
        record.phase,
        record.answer,
        record.resolved_cwd,
        record.root,
    ) {
        (WorkPhase::Succeeded, Some(answer), Some(cwd), Some(root)) => Ok(WorkResult {
            answer,
            cwd,
            root,
            changed_files: record.changed_files,
            stderr_summary: record.stderr_summary.unwrap_or_default(),
        }),
        _ => Err(record.error.unwrap_or_else(|| {
            "前回の作業は未完了です。自動で再実行しません。新しいメッセージで依頼してください"
                .into()
        })),
    }
}

#[async_trait::async_trait]
impl coosenpai_core::work::ChatWorkExecutor for DesktopChatWork {
    async fn execute(
        &self,
        input_id: &str,
        request: WorkRequest,
        cancellation: CancellationToken,
    ) -> Result<WorkResult, String> {
        let id = input_id.to_owned();
        if cancellation.is_cancelled() {
            return Err("作業を停止しました".into());
        }
        let started = self.controller.start(
            &id,
            request,
            self.harness,
            self.factory.clone(),
            cancellation.clone(),
        )?;
        if let Some(record) = started.task.filter(|task| task.phase != WorkPhase::Running) {
            return recorded_result(record);
        }
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                self.controller.stop(&id)?;
                self.controller.await_completion(&id).await?;
            }
            result = self.controller.await_completion(&id) => { result?; }
        }
        let record = self
            .controller
            .snapshot()
            .task
            .filter(|task| task.id == id)
            .ok_or("作業結果が見つかりません")?;
        recorded_result(record)
    }
}

