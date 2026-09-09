use super::*;
use std::sync::Arc;

struct ImageOperation {
    reply: SelectionReply,
    contents: Option<ClipboardContents>,
    image: Option<Vec<u8>>,
    cancelled: bool,
    pids: Vec<i32>,
}

struct TextOperation {
    generation: u64,
    reply: SelectionReply,
    cancellation: CancellationToken,
}

pub(super) struct SessionRuntime<P: SelectionSessionPort> {
    port: Arc<P>,
    model: SelectionSession,
    generation: u64,
    image: Option<ImageOperation>,
    text: Option<TextOperation>,
    text_tasks: tokio::task::JoinSet<Result<Option<SelectionData>, String>>,
    close_replies: Vec<oneshot::Sender<Result<(), String>>>,
    last_observation: Option<(Instant, Vec<crate::platform::SelectionWindowCandidate>)>,
}

impl<P: SelectionSessionPort> SessionRuntime<P> {
    pub(super) fn new(port: P, timeouts_enabled: bool) -> Self {
        Self {
            port: Arc::new(port),
            model: SelectionSession {
                timeouts_enabled,
                ..SelectionSession::default()
            },
            generation: 0,
            image: None,
            text: None,
            text_tasks: tokio::task::JoinSet::new(),
            close_replies: Vec::new(),
            last_observation: None,
        }
    }

    pub(super) async fn run(mut self, mut receiver: mpsc::UnboundedReceiver<Request>) {
        loop {
            let wake = self.model.wake_at();
            tokio::select! { biased;
                () = wait_until(wake), if wake.is_some() => self.observe().await,
                request = receiver.recv() => {
                    let Some(request) = request else { break };
                    self.request(request).await;
                }
                Some(result) = self.text_tasks.join_next(), if !self.text_tasks.is_empty() => {
                    let result = result.unwrap_or_else(|error| Err(error.to_string()));
                    let text = self.text.take().expect("text session");
                    let closed = text.cancellation.is_cancelled();
                    self.port.log(&format!("selection-session: generation={} text completed cancelled={closed}", text.generation));
                    let completion = result.as_ref().map(|_| ()).map_err(Clone::clone);
                    text.reply.send(if closed { completion.clone().map(|()| None) } else { result });
                    self.closed(completion);
                }
            }
        }
        if let Some(text) = &self.text {
            text.cancellation.cancel();
        }
        while self.text_tasks.join_next().await.is_some() {}
        if let Some(image) = &mut self.image {
            image.cancelled = true;
        }
        if self.model.state != SessionState::Closed {
            self.input(SessionInput::Shutdown).await;
        }
    }

    async fn request(&mut self, request: Request) {
        let generation = match &request {
            Request::Open { generation, .. } | Request::Close { generation, .. } => *generation,
        };
        if generation < self.generation {
            self.port.log(&format!(
                "selection-session: generation={generation} ignored=true reason=stale-generation"
            ));
            match request {
                Request::Open { reply, .. } => {
                    reply.send(Ok(None));
                }
                Request::Close { reply, .. } => {
                    let _ = reply.send(Ok(()));
                }
            }
            return;
        }
        self.generation = generation;
        match request {
            Request::Open { kind, reply, .. } => {
                if self.image.is_some() || self.text.is_some() {
                    self.port
                        .log("selection-session: Open rejected reason=not-closed");
                    reply.send(Err("選択セッションを閉じてから開いてください".into()));
                } else if kind == CaptureKind::Text {
                    let cancellation = CancellationToken::new();
                    self.text = Some(TextOperation {
                        generation,
                        reply,
                        cancellation: cancellation.clone(),
                    });
                    let port = self.port.clone();
                    self.text_tasks
                        .spawn(async move { port.selected_text(cancellation).await });
                } else if kind == CaptureKind::Image {
                    self.image = Some(ImageOperation {
                        reply,
                        contents: None,
                        image: None,
                        cancelled: false,
                        pids: Vec::new(),
                    });
                    self.input(SessionInput::Open).await;
                } else {
                    reply.send(Err("音声入力はOS選択セッションの対象外です".into()));
                }
            }
            Request::Close {
                reply, shutdown, ..
            } => {
                self.close_replies.push(reply);
                if let Some(text) = &self.text {
                    text.cancellation.cancel();
                    self.port.log(&format!(
                        "selection-session: generation={generation} text close requested"
                    ));
                } else {
                    if let Some(image) = &mut self.image {
                        image.cancelled = true;
                    }
                    self.input(if shutdown {
                        SessionInput::Shutdown
                    } else {
                        SessionInput::Close
                    })
                    .await;
                }
            }
        }
    }

    async fn input(&mut self, mut input: SessionInput) {
        loop {
            let before = self.model.state;
            let label = format!("{input:?}");
            let observation_input =
                matches!(input, SessionInput::Timeout | SessionInput::Windows(_));
            let now = Instant::now();
            let action = self.model.transition(input, now);
            let diagnostics = if observation_input && matches!(action, SessionAction::Failed(_)) {
                self.timeout_diagnostics(now)
            } else {
                String::new()
            };
            self.port.log(&format!("selection-session: generation={} state={before:?} event={label} action={action:?}{diagnostics} -> {:?}", self.generation, self.model.state));
            let result = match action {
                SessionAction::Start => self
                    .open_image()
                    .await
                    .map(|()| SessionInput::ShortcutPosted),
                SessionAction::Escape => {
                    let result = if let Some(deadline) = self.model.deadline {
                        tokio::time::timeout_at(deadline, self.port.escape())
                            .await
                            .unwrap_or_else(|_| {
                                Err("選択窓への Esc 送出が1秒以内に完了しませんでした".into())
                            })
                    } else {
                        self.port.escape().await
                    };
                    result.map(|()| SessionInput::EscapePosted)
                }
                SessionAction::ReadImage => match self.port.image().await {
                    Ok(data) => {
                        let found = data.is_some();
                        self.image.as_mut().expect("image session").image = data;
                        Ok(SessionInput::ImageRead(found))
                    }
                    Err(error) => Err(error),
                },
                SessionAction::Opened => {
                    let image = self.image.as_ref().expect("image session");
                    self.port
                        .log(&crate::e2e_logs::selection_observed(&image.pids));
                    image.reply.opened();
                    return;
                }
                SessionAction::CloseAccepted => {
                    self.closed(Ok(()));
                    return;
                }
                SessionAction::Captured | SessionAction::Cancelled => {
                    self.finish_image(Ok(()), false).await;
                    return;
                }
                SessionAction::Drained => {
                    self.port.log(&crate::e2e_logs::selection_closed(
                        self.model.image_captured,
                    ));
                    return;
                }
                SessionAction::Closed => {
                    self.finish_image(Ok(()), true).await;
                    return;
                }
                SessionAction::Failed(error) => {
                    self.port.log_error(&format!(
                        "selection-session: generation={} error={error}",
                        self.generation
                    ));
                    if before == SessionState::Draining {
                        return;
                    }
                    self.finish_image(Err(error), false).await;
                    return;
                }
                SessionAction::Observe
                | SessionAction::Ignore
                | SessionAction::RejectOpen
                | SessionAction::TrackClipboard
                | SessionAction::UpdateClipboardBaseline => return,
            };
            input = result.unwrap_or_else(SessionInput::Failed);
        }
    }

    async fn open_image(&mut self) -> Result<(), String> {
        self.last_observation = None;
        let setup = self.port.prepare_image().await?;
        let image = self.image.as_mut().expect("image session");
        image.contents = setup.contents;
        self.model.clipboard_baseline = setup.count;
        self.model.clipboard_changed = setup.count;
        self.model.image_captured = false;
        self.model.baseline_windows = setup.windows;
        self.model.tracked_window = None;
        self.port.post_image(setup.shortcut).await
    }

    async fn observe(&mut self) {
        if self
            .model
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.input(SessionInput::Timeout).await;
            return;
        }
        let observed = if let Some(deadline) = self.model.deadline {
            match tokio::time::timeout_at(deadline, self.port.observe()).await {
                Ok(result) => result,
                Err(_) => {
                    self.input(SessionInput::Timeout).await;
                    return;
                }
            }
        } else {
            self.port.observe().await
        };
        let observed = match observed {
            Ok(observed) => observed,
            Err(error) => {
                self.fail_observation(error).await;
                return;
            }
        };
        self.last_observation = Some((Instant::now(), observed.candidates));
        let present = self
            .model
            .windows_present(&observed.windows, &observed.onscreen_window_ids);
        if self.model.state == SessionState::AwaitingOpenToClose {
            self.input(SessionInput::ClipboardChanged(observed.change_count))
                .await;
        }
        if matches!(
            self.model.state,
            SessionState::Opening | SessionState::AwaitingOpenToClose
        ) && present
        {
            let image = self.image.as_mut().expect("image session");
            image.pids = observed
                .windows
                .iter()
                .filter(|window| self.model.tracked_window == Some(window.id))
                .map(|window| window.owner_pid)
                .collect();
            image.pids.sort_unstable();
            image.pids.dedup();
            self.input(SessionInput::Windows(true)).await;
            if self.model.state == SessionState::Closed {
                return;
            }
        }
        self.input(SessionInput::ClipboardChanged(observed.change_count))
            .await;
        if self.model.state != SessionState::Closed {
            self.input(SessionInput::Windows(present)).await;
        }
    }

    fn timeout_diagnostics(&self, now: Instant) -> String {
        match self.last_observation.as_ref() {
            Some((observed_at, candidates)) => format!(
                " observed-age-ms={} candidates={candidates:?}",
                now.duration_since(*observed_at).as_millis()
            ),
            None => " observation=unavailable candidates=[]".into(),
        }
    }

    async fn fail_observation(&mut self, error: String) {
        self.port.log(&format!(
            "selection-session: observation failed error={error}"
        ));
        self.input(SessionInput::ObservationFailed(error)).await;
    }

    async fn finish_image(&mut self, mut result: Result<(), String>, window_closed: bool) {
        if let Some(mut image) = self.image.take() {
            if let Some(changed) = self.model.restoration_count() {
                if let Some(contents) = image.contents.take() {
                    if let Err(error) = self.port.restore(contents, changed).await {
                        self.port.log(&format!(
                            "selection-session: clipboard restore failed error={error}"
                        ));
                        result = Err(error);
                    }
                }
            }
            if result.is_ok() {
                let data = if image.cancelled {
                    None
                } else {
                    image.image.as_deref()
                };
                if window_closed {
                    self.port
                        .log(&crate::e2e_logs::selection_closed(data.is_some()));
                }
                if !image.cancelled && (data.is_some() || window_closed) {
                    self.port.log(&crate::e2e_logs::selection_complete(data));
                }
            }
            let completion = result.clone().map(|()| {
                if image.cancelled {
                    None
                } else {
                    image.image.map(SelectionData::Image)
                }
            });
            image.reply.send(completion);
        }
        self.closed(result);
    }

    fn closed(&mut self, result: Result<(), String>) {
        for reply in self.close_replies.drain(..) {
            let _ = reply.send(result.clone());
        }
    }
}

async fn wait_until(wake: Option<Instant>) {
    match wake {
        Some(wake) => tokio::time::sleep_until(wake).await,
        None => std::future::pending().await,
    }
}
