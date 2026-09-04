use super::bridge_io::read_line_bounded;
use super::bridge_validation::{
    invalid_output, parse_error_kind, remove_null_fields, retryable, session_json, validate_call,
};
use super::provider_output::validate_json_shape;
use super::{
    BridgeProvider, ProviderCall, ProviderCapabilities, ProviderError, ProviderErrorKind,
    ProviderEventSink, ProviderMidTurnInput, ProviderName, ProviderResult, ProviderSession,
    ProviderUsage,
};
use crate::process::{cleanup_process_group, terminate_process_group, ActiveProcessGroup};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio_util::sync::CancellationToken;

#[path = "bridge_completion.rs"]
mod completion;
#[path = "bridge_request.rs"]
mod request;
use completion::{
    complete_pending, fail_generation_once, fail_pending, mark_reaping, wait_for_reap, BridgeReply,
};
use request::{send_request_value, serialize_request_line};

const RESPONSE_LINE_LIMIT: usize = 2 * 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;
const MAX_PENDING: usize = 32;
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);
const SHUTDOWN_REAP_LIMIT: Duration = Duration::from_secs(3);
const CANCEL_GRACE: Duration = Duration::from_millis(100);
const BRIDGE_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
const PROTOCOL_VERSION: u64 = 2;

#[derive(Clone)]
pub struct BridgeLaunch {
    pub node: PathBuf,
    pub script: PathBuf,
    pub env: Vec<(String, String)>,
}

impl fmt::Debug for BridgeLaunch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BridgeLaunch")
            .field("node", &self.node)
            .field("script", &self.script)
            .field(
                "env_keys",
                &self.env.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Clone)]
pub struct ProviderBridge {
    inner: Arc<BridgeInner>,
}

struct BridgeInner {
    launch: RwLock<BridgeLaunch>,
    state: Mutex<BridgeState>,
    write_lock: Mutex<()>,
    capabilities: RwLock<HashMap<ProviderName, ProviderCapabilities>>,
    #[cfg(test)]
    reap_gate: std::sync::Mutex<Option<ReapGate>>,
}

#[cfg(test)]
#[derive(Clone)]
struct ReapGate {
    reaper_waiting: Arc<tokio::sync::Notify>,
    request_waiting: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

struct BridgeState {
    generation: u64,
    stdin: Option<ChildStdin>,
    pid: Option<u32>,
    wait_completion: Option<(u64, watch::Receiver<bool>)>,
    reaping: Option<(u64, watch::Receiver<bool>)>,
    pending: HashMap<String, Pending>,
    restart_failures: u32,
    restart_not_before: Option<Instant>,
    failed_generation: Option<u64>,
    shutting_down: bool,
}

struct Pending {
    generation: u64,
    provider: ProviderName,
    model: Option<String>,
    kind: PendingKind,
    session_id: Option<String>,
    usage: Option<ProviderUsage>,
    schema: Option<Value>,
}

enum PendingKind {
    Open(oneshot::Sender<Result<ProviderCapabilities, ProviderError>>),
    Resolve(oneshot::Sender<Result<ProviderCapabilities, ProviderError>>),
    Send {
        events: Arc<dyn ProviderEventSink>,
        response: oneshot::Sender<Result<ProviderResult, ProviderError>>,
    },
    Ack(oneshot::Sender<Result<(), ProviderError>>),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgeEvent {
    id: String,
    event: String,
    provider: Option<ProviderName>,
    protocol_version: Option<u64>,
    capabilities: Option<ProviderCapabilities>,
    session: Option<String>,
    text: Option<String>,
    reset: Option<bool>,
    value: Option<Value>,
    kind: Option<String>,
    message: Option<String>,
    detail: Option<String>,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

impl ProviderBridge {
    pub fn new(launch: BridgeLaunch) -> Self {
        Self {
            inner: Arc::new(BridgeInner {
                launch: RwLock::new(launch),
                state: Mutex::new(BridgeState {
                    generation: 0,
                    stdin: None,
                    pid: None,
                    wait_completion: None,
                    reaping: None,
                    pending: HashMap::new(),
                    restart_failures: 0,
                    restart_not_before: None,
                    failed_generation: None,
                    shutting_down: false,
                }),
                write_lock: Mutex::new(()),
                capabilities: RwLock::new(HashMap::new()),
                #[cfg(test)]
                reap_gate: std::sync::Mutex::new(None),
            }),
        }
    }

    pub fn provider(&self, provider: ProviderName, executable: Option<PathBuf>) -> BridgeProvider {
        BridgeProvider::new(provider, executable, self.clone())
    }

    pub async fn shutdown(&self) {
        let (pid, stdin, wait_completion) = {
            let mut state = self.inner.state.lock().await;
            state.shutting_down = true;
            mark_reaping(&mut state);
            fail_pending(&mut state, retryable("provider bridge を終了しました。"));
            (
                state.pid.take(),
                state.stdin.take(),
                state.wait_completion.as_ref().map(|(_, wait)| wait.clone()),
            )
        };
        self.stop_process(pid, stdin, wait_completion).await;
    }

    pub async fn update_environment(
        &self,
        environment: Vec<(String, String)>,
    ) -> Result<(), ProviderError> {
        let (pid, stdin, wait_completion) = {
            let mut state = self.inner.state.lock().await;
            if state.shutting_down {
                return Err(retryable("provider bridge は終了処理中です。"));
            }
            let mut launch = self
                .inner
                .launch
                .write()
                .map_err(|_| retryable("provider bridge の設定を更新できません。"))?;
            if launch.env == environment {
                return Ok(());
            }
            launch.env = environment;
            state.restart_failures = 0;
            state.restart_not_before = None;
            state.failed_generation = None;
            mark_reaping(&mut state);
            fail_pending(
                &mut state,
                retryable("provider bridge の認証設定を更新しました。"),
            );
            state.generation = state.generation.wrapping_add(1);
            (
                state.pid.take(),
                state.stdin.take(),
                state.wait_completion.as_ref().map(|(_, wait)| wait.clone()),
            )
        };
        if let Ok(mut capabilities) = self.inner.capabilities.write() {
            capabilities.clear();
        }
        self.stop_process(pid, stdin, wait_completion).await;
        Ok(())
    }

    pub(super) fn cached_capabilities(
        &self,
        provider: ProviderName,
    ) -> Option<ProviderCapabilities> {
        self.inner
            .capabilities
            .read()
            .map(|values| values.get(&provider).cloned())
            .unwrap_or_default()
    }

    pub(super) async fn open(
        &self,
        provider: ProviderName,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Result<ProviderCapabilities, ProviderError> {
        if let Some(capabilities) = self.cached_capabilities(provider) {
            return Ok(capabilities);
        }
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, mut rx) = oneshot::channel();
        self.inner
            .request(
                id.clone(),
                json!({"id": id, "op": "open", "provider": provider}),
                Pending {
                    generation: 0,
                    provider,
                    model: None,
                    kind: PendingKind::Open(tx),
                    session_id: None,
                    usage: None,
                    schema: None,
                },
            )
            .await?;
        tokio::select! {
            result = &mut rx => result.unwrap_or_else(|_| Err(retryable("provider bridge が終了しました。"))),
            () = cancellation.cancelled() => {
                cancel_request(&self.inner, &id, &mut rx).await;
                Err(retryable("provider bridge の起動待ちをキャンセルしました。"))
            }
            () = tokio::time::sleep(timeout) => {
                cancel_request(&self.inner, &id, &mut rx).await;
                Err(retryable("provider bridge の起動確認が timeout しました。"))
            }
        }
    }

    pub(super) async fn resolve_model_capabilities(
        &self,
        provider: ProviderName,
        executable: Option<&PathBuf>,
        model: Option<&str>,
        cancellation: CancellationToken,
        timeout: Duration,
    ) -> Result<ProviderCapabilities, ProviderError> {
        let temporary_cwd = tempfile::Builder::new()
            .prefix("coosenpai-provider-capabilities-")
            .tempdir()
            .map_err(|_| retryable("provider の作業ディレクトリを作成できません。"))?;
        let id = uuid::Uuid::new_v4().to_string();
        let mut request = json!({
            "id": id,
            "op": "resolve",
            "provider": provider,
            "model": model,
            "executable": executable,
            "cwd": temporary_cwd.path(),
        });
        remove_null_fields(&mut request);
        let (tx, mut rx) = oneshot::channel();
        self.inner
            .request(
                id.clone(),
                request,
                Pending {
                    generation: 0,
                    provider,
                    model: model.map(str::to_owned),
                    kind: PendingKind::Resolve(tx),
                    session_id: None,
                    usage: None,
                    schema: None,
                },
            )
            .await?;
        tokio::select! {
            result = &mut rx => result.unwrap_or_else(|_| Err(retryable("provider bridge が終了しました。"))),
            () = cancellation.cancelled() => {
                cancel_request(&self.inner, &id, &mut rx).await;
                Err(retryable("provider の能力確認をキャンセルしました。"))
            }
            () = tokio::time::sleep(timeout) => {
                cancel_request(&self.inner, &id, &mut rx).await;
                Err(retryable("provider の能力確認が timeout しました。"))
            }
        }
    }

    pub(super) async fn send(
        &self,
        provider: ProviderName,
        executable: Option<&PathBuf>,
        input: ProviderCall,
        cancellation: CancellationToken,
        events: Arc<dyn ProviderEventSink>,
        mut additional_inputs: Option<mpsc::UnboundedReceiver<ProviderMidTurnInput>>,
    ) -> Result<ProviderResult, ProviderError> {
        if cancellation.is_cancelled() {
            return Err(retryable("provider の呼び出しをキャンセルしました。"));
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(input.timeout)
            .ok_or_else(|| retryable("provider の timeout が範囲外です。"))?;
        let capabilities = self
            .open(provider, cancellation.clone(), input.timeout)
            .await?;
        validate_call(&capabilities, &input)?;
        let temporary_cwd = tempfile::Builder::new()
            .prefix("coosenpai-provider-")
            .tempdir()
            .map_err(|_| retryable("provider の作業ディレクトリを作成できません。"))?;
        let id = uuid::Uuid::new_v4().to_string();
        let (session_mode, session_id) =
            session_json(provider, input.model.as_deref(), &input.session)?;
        let request = send_request_value(
            &id,
            provider,
            executable.map(PathBuf::as_path),
            temporary_cwd.path(),
            &input,
            session_mode,
            session_id,
        );
        let (tx, mut rx) = oneshot::channel();
        self.inner
            .request(
                id.clone(),
                request,
                Pending {
                    generation: 0,
                    provider,
                    model: input.model.filter(|value| value != "default"),
                    kind: PendingKind::Send {
                        events: events.clone(),
                        response: tx,
                    },
                    session_id: None,
                    usage: None,
                    schema: input.output_schema.clone(),
                },
            )
            .await?;
        let mut additions_open = additional_inputs.is_some();
        let deadline_sleep = tokio::time::sleep_until(deadline);
        tokio::pin!(deadline_sleep);
        loop {
            tokio::select! {
                biased;
                additional = receive_additional(&mut additional_inputs), if additions_open => {
                    let Some(additional) = additional else {
                        additions_open = false;
                        continue;
                    };
                    if !capabilities.mid_turn_input {
                        cancel_request(&self.inner, &id, &mut rx).await;
                        return Err(ProviderError {
                            kind: ProviderErrorKind::Unsupported,
                            message: "provider は実行中の言い足しに対応していません。".to_owned(),
                        });
                    }
                    let appended = tokio::select! {
                        result = self.inner.append(provider, &id, &additional) => result,
                        () = cancellation.cancelled() => {
                            cancel_request(&self.inner, &id, &mut rx).await;
                            return Err(retryable("provider の呼び出しをキャンセルしました。"));
                        }
                        () = &mut deadline_sleep => {
                            cancel_request(&self.inner, &id, &mut rx).await;
                            return Err(retryable("provider の呼び出しが timeout しました。"));
                        }
                    };
                    if let Err(error) = appended {
                        cancel_request(&self.inner, &id, &mut rx).await;
                        return Err(error);
                    }
                    events.mid_turn_accepted(&additional.source_id);
                }
                result = &mut rx => {
                    return result.unwrap_or_else(|_| Err(retryable("provider bridge が終了しました。")));
                }
                () = cancellation.cancelled() => {
                    cancel_request(&self.inner, &id, &mut rx).await;
                    return Err(retryable("provider の呼び出しをキャンセルしました。"));
                }
                () = &mut deadline_sleep => {
                    cancel_request(&self.inner, &id, &mut rx).await;
                    return Err(retryable("provider の呼び出しが timeout しました。"));
                }
            }
        }
    }

    async fn stop_process(
        &self,
        pid: Option<u32>,
        mut stdin: Option<ChildStdin>,
        mut wait_completion: Option<watch::Receiver<bool>>,
    ) {
        if let Some(stdin) = stdin.as_mut() {
            let close = json!({"id": uuid::Uuid::new_v4().to_string(), "op": "close"});
            if let Ok(mut line) = serde_json::to_vec(&close) {
                line.push(b'\n');
                let _ = tokio::time::timeout(BRIDGE_WRITE_TIMEOUT, async {
                    stdin.write_all(&line).await?;
                    stdin.flush().await
                })
                .await;
            }
        }
        let Some(pid) = pid else {
            return;
        };
        terminate_process_group(Some(pid), false);
        let reaped = match wait_completion.as_mut() {
            Some(wait) => wait_for_reap(wait, SHUTDOWN_GRACE).await,
            None => false,
        };
        if !reaped {
            terminate_process_group(Some(pid), true);
            if let Some(wait) = wait_completion.as_mut() {
                let remaining = SHUTDOWN_REAP_LIMIT.saturating_sub(SHUTDOWN_GRACE);
                let _ = wait_for_reap(wait, remaining).await;
            }
        }
    }
}

async fn receive_additional(
    inputs: &mut Option<mpsc::UnboundedReceiver<ProviderMidTurnInput>>,
) -> Option<ProviderMidTurnInput> {
    match inputs {
        Some(inputs) => inputs.recv().await,
        None => std::future::pending().await,
    }
}

pub(super) fn send_request_fits(input: &ProviderCall) -> bool {
    request::send_request_fits(input)
}

impl BridgeInner {
    async fn append(
        self: &Arc<Self>,
        provider: ProviderName,
        target_id: &str,
        input: &ProviderMidTurnInput,
    ) -> Result<(), ProviderError> {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.request(
            id.clone(),
            json!({
                "id": id,
                "op": "append",
                "targetId": target_id,
                "message": input.message,
                "images": input.images.iter().map(|image| &image.path).collect::<Vec<_>>(),
            }),
            Pending {
                generation: 0,
                provider,
                model: None,
                kind: PendingKind::Ack(tx),
                session_id: None,
                usage: None,
                schema: None,
            },
        )
        .await?;
        rx.await
            .unwrap_or_else(|_| Err(retryable("provider bridge が終了しました。")))
    }

    async fn request(
        self: &Arc<Self>,
        id: String,
        request: Value,
        mut pending: Pending,
    ) -> Result<(), ProviderError> {
        let line = serialize_request_line(&request)?;
        let _write_guard = self.write_lock.lock().await;
        loop {
            self.wait_for_reap_before_start().await?;
            let generation = {
                let mut state = self.state.lock().await;
                if state.reaping.is_some() {
                    continue;
                }
                self.ensure_started(&mut state).await?;
                if state.pending.len() >= MAX_PENDING {
                    return Err(retryable("provider bridge の待機要求が上限に達しました。"));
                }
                pending.generation = state.generation;
                let generation = state.generation;
                state.pending.insert(id.clone(), pending);
                generation
            };
            if let Err(error) = self.write_line_locked(generation, line.clone()).await {
                let mut state = self.state.lock().await;
                if let Some(pending) = state.pending.remove(&id) {
                    complete_pending(pending, Err(retryable(&error.message)));
                }
                drop(state);
                self.protocol_failure(generation).await;
                return Err(error);
            }
            return Ok(());
        }
    }

    async fn wait_for_reap_before_start(&self) -> Result<(), ProviderError> {
        loop {
            let wait = {
                let state = self.state.lock().await;
                if state.shutting_down {
                    return Err(retryable("provider bridge は終了処理中です。"));
                }
                state
                    .reaping
                    .as_ref()
                    .map(|(generation, wait)| (*generation, wait.clone()))
            };
            let Some((generation, mut wait)) = wait else {
                return Ok(());
            };
            #[cfg(test)]
            if let Some(gate) = self.reap_gate.lock().expect("reap gate").clone() {
                gate.request_waiting.notify_one();
            }
            if !wait_for_reap(&mut wait, SHUTDOWN_REAP_LIMIT).await {
                return Err(retryable("provider bridge の終了待ちが timeout しました。"));
            }
            let mut state = self.state.lock().await;
            if state
                .reaping
                .as_ref()
                .is_some_and(|(current, _)| *current == generation)
            {
                state.reaping = None;
            }
        }
    }

    async fn write_line(&self, generation: u64, line: Vec<u8>) -> Result<(), ProviderError> {
        let _write_guard = self.write_lock.lock().await;
        self.write_line_locked(generation, line).await
    }

    async fn write_line_locked(&self, generation: u64, line: Vec<u8>) -> Result<(), ProviderError> {
        let mut stdin = {
            let mut state = self.state.lock().await;
            if state.generation != generation {
                return Err(retryable("provider bridge の世代が切り替わりました。"));
            }
            state
                .stdin
                .take()
                .ok_or_else(|| retryable("provider bridge の stdin がありません。"))?
        };
        let write = tokio::time::timeout(BRIDGE_WRITE_TIMEOUT, async {
            stdin
                .write_all(&line)
                .await
                .map_err(|_| "provider bridge への送信に失敗しました。")?;
            stdin
                .flush()
                .await
                .map_err(|_| "provider bridge の flush に失敗しました。")
        })
        .await
        .map_err(|_| "provider bridge への送信が timeout しました。")
        .and_then(|result| result);
        let lease_is_current = {
            let mut state = self.state.lock().await;
            let lease_is_current = state.generation == generation
                && state.failed_generation != Some(generation)
                && !state.shutting_down
                && state.stdin.is_none();
            if write.is_ok() && lease_is_current {
                state.stdin = Some(stdin);
            }
            lease_is_current
        };
        if write.is_ok() && !lease_is_current {
            return Err(retryable("provider bridge の世代が切り替わりました。"));
        }
        write.map_err(retryable)
    }

    async fn ensure_started(
        self: &Arc<Self>,
        state: &mut BridgeState,
    ) -> Result<(), ProviderError> {
        if state.shutting_down {
            return Err(retryable("provider bridge は終了処理中です。"));
        }
        if state.stdin.is_some() {
            return Ok(());
        }
        if state
            .restart_not_before
            .is_some_and(|deadline| Instant::now() < deadline)
        {
            return Err(retryable("provider bridge の再起動待ちです。"));
        }
        let launch = self
            .launch
            .read()
            .map_err(|_| retryable("provider bridge の設定を読み込めません。"))?
            .clone();
        if !launch.node.is_file() || !launch.script.is_file() {
            return Err(retryable("Node または provider bridge が見つかりません。"));
        }
        let mut command = Command::new(&launch.node);
        command
            .arg(&launch.script)
            .envs(launch.env.iter().map(|(key, value)| (key, value)))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|_| retryable("provider bridge を起動できません。"))?;
        let pid = child.id();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| retryable("provider bridge の stdin を取得できません。"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| retryable("provider bridge の stdout を取得できません。"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| retryable("provider bridge の stderr を取得できません。"))?;
        state.generation = state.generation.wrapping_add(1);
        state.stdin = Some(stdin);
        state.pid = pid;
        let (wait_tx, wait_rx) = watch::channel(false);
        state.wait_completion = Some((state.generation, wait_rx));
        let generation = state.generation;
        let inner = Arc::clone(self);
        tokio::spawn(async move { inner.read_stdout(stdout, generation).await });
        let inner = Arc::clone(self);
        tokio::spawn(async move { inner.read_stderr(stderr, generation).await });
        let inner = Arc::clone(self);
        #[cfg(test)]
        let reap_gate = self.reap_gate.lock().expect("reap gate").clone();
        tokio::spawn(async move {
            let mut process_group = ActiveProcessGroup::register(pid);
            #[cfg(test)]
            if let Some(gate) = reap_gate {
                gate.reaper_waiting.notify_one();
                gate.release.notified().await;
            }
            let _ = child.wait().await;
            cleanup_process_group(pid).await;
            process_group.disarm();
            inner.process_exited(generation).await;
            let _ = wait_tx.send(true);
        });
        Ok(())
    }

    async fn read_stdout<R>(self: Arc<Self>, reader: R, generation: u64)
    where
        R: AsyncRead + Unpin,
    {
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            match read_line_bounded(&mut reader, &mut line, RESPONSE_LINE_LIMIT).await {
                Ok(0) => return,
                Ok(_) => {
                    while matches!(line.last(), Some(b'\n' | b'\r')) {
                        line.pop();
                    }
                    match serde_json::from_slice::<BridgeEvent>(&line) {
                        Ok(event) => self.handle_event(generation, event).await,
                        Err(_) => {
                            self.protocol_failure(generation).await;
                            return;
                        }
                    }
                }
                Err(_) => {
                    self.protocol_failure(generation).await;
                    return;
                }
            }
        }
    }

    async fn read_stderr<R>(self: Arc<Self>, mut reader: R, generation: u64)
    where
        R: AsyncRead + Unpin,
    {
        let mut total = 0usize;
        let mut buffer = [0u8; 4096];
        loop {
            match tokio::io::AsyncReadExt::read(&mut reader, &mut buffer).await {
                Ok(0) => return,
                Ok(read) => {
                    total = total.saturating_add(read);
                    if total > STDERR_LIMIT {
                        self.protocol_failure(generation).await;
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    }

    async fn handle_event(&self, generation: u64, event: BridgeEvent) {
        let mut state = self.state.lock().await;
        if state.generation != generation {
            return;
        }
        let Some(mut pending) = state.pending.remove(&event.id) else {
            return;
        };
        if pending.generation != generation {
            return;
        }
        match event.event.as_str() {
            "session" => {
                if let Some(capabilities) = event.capabilities {
                    if event.protocol_version != Some(PROTOCOL_VERSION)
                        || event.provider != Some(pending.provider)
                    {
                        complete_pending(
                            pending,
                            Err(invalid_output(
                                "provider bridge の handshake が現在の契約と一致しません。",
                            )),
                        );
                        return;
                    }
                    if matches!(pending.kind, PendingKind::Open(_)) {
                        if let Ok(mut values) = self.capabilities.write() {
                            values.insert(pending.provider, capabilities.clone());
                        }
                    }
                    complete_pending(pending, Ok(BridgeReply::Capabilities(capabilities)));
                    return;
                } else {
                    pending.session_id = event.session;
                }
            }
            "delta" => {
                if let PendingKind::Send { events, .. } = &pending.kind {
                    if event.reset == Some(true) {
                        events.reset();
                    } else if let Some(text) = event.text {
                        events.delta(&text);
                    }
                }
            }
            "usage" => {
                let usage = ProviderUsage {
                    call_id: Some(event.id.clone()),
                    provider: Some(pending.provider),
                    model: pending.model.clone(),
                    input_tokens: event.input_tokens,
                    cached_input_tokens: event.cached_input_tokens,
                    output_tokens: event.output_tokens,
                    total_tokens: event.total_tokens,
                };
                if let PendingKind::Send { events, .. } = &pending.kind {
                    events.usage(&usage);
                }
                pending.usage = Some(usage);
            }
            "final" => {
                let text = event.text.unwrap_or_default();
                if pending.schema.as_ref().is_some_and(|schema| {
                    event
                        .value
                        .as_ref()
                        .is_none_or(|value| validate_json_shape(value, schema).is_err())
                }) {
                    complete_pending(
                        pending,
                        Err(invalid_output(
                            "provider の structured output が schema と一致しません。",
                        )),
                    );
                    return;
                }
                let session = pending.session_id.clone().map(|id| ProviderSession {
                    provider: pending.provider,
                    model: pending.model.clone(),
                    id,
                });
                if matches!(pending.kind, PendingKind::Send { .. }) {
                    state.restart_failures = 0;
                    state.restart_not_before = None;
                }
                complete_pending(
                    pending,
                    Ok(BridgeReply::Result(ProviderResult {
                        text,
                        value: event.value,
                        session,
                    })),
                );
                return;
            }
            "error" => {
                let message = event
                    .message
                    .unwrap_or_else(|| "provider の呼び出しに失敗しました。".to_owned());
                let error = ProviderError {
                    kind: parse_error_kind(event.kind.as_deref()),
                    message: diagnostic_message(message, event.detail),
                };
                complete_pending(pending, Err(error));
                return;
            }
            "closed" => {
                if matches!(pending.kind, PendingKind::Ack(_)) {
                    complete_pending(pending, Ok(BridgeReply::Ack));
                    return;
                }
                complete_pending(
                    pending,
                    Err(retryable("provider bridge が応答前に要求を閉じました。")),
                );
                return;
            }
            _ => {
                complete_pending(
                    pending,
                    Err(invalid_output("provider bridge の event が不正です。")),
                );
                return;
            }
        }
        state.pending.insert(event.id, pending);
    }

    async fn cancel(
        &self,
        target_id: &str,
    ) -> Option<oneshot::Receiver<Result<(), ProviderError>>> {
        let mut state = self.state.lock().await;
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        let provider = state
            .pending
            .get(target_id)
            .map(|pending| pending.provider)
            .unwrap_or(ProviderName::Codex);
        let generation = state.generation;
        state.pending.insert(
            id.clone(),
            Pending {
                generation,
                provider,
                model: None,
                kind: PendingKind::Ack(tx),
                session_id: None,
                usage: None,
                schema: None,
            },
        );
        let request = json!({
            "id": id,
            "op": "cancel",
            "targetId": target_id,
        });
        drop(state);
        let write = match serde_json::to_vec(&request) {
            Ok(mut line) => {
                line.push(b'\n');
                self.write_line(generation, line).await
            }
            Err(_) => Err(retryable("provider bridge の cancel を作成できません。")),
        };
        if let Err(error) = write {
            let mut state = self.state.lock().await;
            if let Some(pending) = state.pending.remove(&id) {
                complete_pending(pending, Err(retryable(&error.message)));
            }
            drop(state);
            self.protocol_failure(generation).await;
        }
        Some(rx)
    }

    async fn abort_generation(&self) {
        let (pid, mut wait_completion) = {
            let mut state = self.state.lock().await;
            let generation = state.generation;
            let pid = state.pid;
            let wait_completion = state.wait_completion.as_ref().map(|(_, wait)| wait.clone());
            if !fail_generation_once(&mut state, generation) {
                return;
            }
            // A forced termination caused by cancellation is not a provider
            // failure.  The process has been reaped below, so the next lease
            // may start a fresh generation without exponential backoff.
            state.restart_failures = 0;
            state.restart_not_before = None;
            (pid, wait_completion)
        };
        if let Ok(mut capabilities) = self.capabilities.write() {
            capabilities.clear();
        }
        terminate_process_group(pid, true);
        if let Some(wait) = wait_completion.as_mut() {
            let _ = wait_for_reap(wait, SHUTDOWN_REAP_LIMIT).await;
        }
    }

    async fn protocol_failure(&self, generation: u64) {
        let pid = {
            let mut state = self.state.lock().await;
            if state.generation != generation {
                return;
            }
            let pid = state.pid;
            if !fail_generation_once(&mut state, generation) {
                return;
            }
            pid
        };
        if let Ok(mut capabilities) = self.capabilities.write() {
            capabilities.clear();
        }
        terminate_process_group(pid, true);
    }

    async fn process_exited(&self, generation: u64) {
        let mut state = self.state.lock().await;
        if state.generation == generation && fail_generation_once(&mut state, generation) {
            if let Ok(mut capabilities) = self.capabilities.write() {
                capabilities.clear();
            }
        }
    }
}

async fn cancel_request<T>(
    inner: &Arc<BridgeInner>,
    target_id: &str,
    response: &mut oneshot::Receiver<Result<T, ProviderError>>,
) {
    let mut cancel_ack = inner.cancel(target_id).await;
    let mut response_completed = false;
    let completed = tokio::time::timeout(CANCEL_GRACE, async {
        let _ = (&mut *response).await;
        response_completed = true;
        if let Some(ack) = cancel_ack.as_mut() {
            let _ = (&mut *ack).await;
        }
    })
    .await
    .is_ok();
    if !completed {
        inner.abort_generation().await;
        if !response_completed {
            let _ = response.await;
        }
    }
}

fn diagnostic_message(message: String, detail: Option<String>) -> String {
    match detail.filter(|value| !value.is_empty()) {
        Some(detail) => format!("{message}: {detail}"),
        None => message,
    }
}

impl Drop for BridgeInner {
    fn drop(&mut self) {
        if let Ok(state) = self.state.try_lock() {
            terminate_process_group(state.pid, true);
        }
    }
}

