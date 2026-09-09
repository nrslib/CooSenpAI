use crate::capture::CaptureKind;
use crate::presentation::{PresentationEvent, PresentationState};
use crate::ui_events::{
    EffectResult, Handling, PresenterId, UiEffect, UiEvent, UiTask, UiView, ViewCommand,
    VoiceAction,
};
use crate::ui_load::WindowRequest;
use crate::ui_presenters::{BubblePresenter, ChatPresenter, WindowPresenter};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub(crate) fn application_input(app: &tauri::AppHandle, event: UiEvent) {
    use tauri::Manager;
    if let Some(state) = app.try_state::<Arc<crate::state::DesktopState>>() {
        state.ui.input(UiView::Application, event);
    }
}

#[async_trait::async_trait]
pub(crate) trait UiPort: Send + Sync + 'static {
    async fn execute(&self, effect: UiEffect) -> Result<EffectResult, String>;
    async fn run(&self, task: UiTask) -> Result<EffectResult, String>;
}

struct Envelope {
    presenter: PresenterId,
    event: UiEvent,
    reply: Option<oneshot::Sender<Result<Option<String>, String>>>,
    completion: mpsc::UnboundedSender<RootMessage>,
}

enum RootMessage {
    Input(Envelope),
    Completed {
        pipeline: Pipeline,
        result: Result<EffectResult, String>,
    },
}

#[derive(Clone)]
pub(crate) struct UiHandle {
    sender: mpsc::UnboundedSender<RootMessage>,
}

impl UiHandle {
    pub(crate) async fn query<T>(
        &self,
        view: UiView,
        event: impl FnOnce(oneshot::Sender<T>) -> UiEvent,
    ) -> Result<T, String> {
        let (reply, response) = oneshot::channel();
        self.input(view, event(reply));
        response
            .await
            .map_err(|_| "UIの応答がありません".to_owned())
    }

    pub(crate) fn input(&self, view: UiView, event: UiEvent) {
        let _ = self.sender.send(RootMessage::Input(Envelope {
            presenter: view.presenter(),
            event,
            reply: None,
            completion: self.sender.clone(),
        }));
    }

    pub(crate) async fn request(
        &self,
        view: UiView,
        event: UiEvent,
    ) -> Result<Option<String>, String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(RootMessage::Input(Envelope {
                presenter: view.presenter(),
                event,
                reply: Some(reply),
                completion: self.sender.clone(),
            }))
            .map_err(|_| "UIの受付は終了しています".to_owned())?;
        result
            .await
            .map_err(|_| "UIの処理が完了しませんでした".to_owned())?
    }
}

#[derive(Default)]
pub(crate) struct UiModel {
    active_operation: Option<ActiveOperation>,
    main_focused: bool,
    speech_phase: String,
    completed_generation: u64,
    shutdown_signals: u32,
    protected_popup: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ActiveOperation {
    kind: CaptureKind,
    generation: u64,
}

pub(crate) struct UiRoot<P: UiPort> {
    model: UiModel,
    activation: crate::activation_policy::ActivationPolicy,
    tray: crate::tray_presenter::TrayPresenter,
    snapshot: Option<crate::snapshot_presenter::SnapshotPresenter>,
    chat: ChatPresenter,
    tutorial: crate::ui_commands::TutorialPresenter,
    capture: crate::capture::CapturePresenter,
    avatar: crate::avatar_presenter::AvatarPresenter,
    bubble: BubblePresenter,
    windows: Vec<(PresenterId, WindowPresenter)>,
    port: Arc<P>,
    operation_busy: bool,
    deferred: std::collections::VecDeque<Pipeline>,
    receiver: mpsc::UnboundedReceiver<RootMessage>,
}

pub(crate) fn channel() -> (
    UiHandle,
    impl FnOnce(
            Arc<crate::state::DesktopState>,
            crate::snapshot::AppSnapshot,
            crate::capture::CapturePresenter,
        ) + Send,
) {
    let (handle, receiver) = mpsc::unbounded_channel();
    (
        UiHandle { sender: handle },
        move |state, initial, capture| {
            let config = state.runtime_config();
            let avatar = crate::avatar_presenter::AvatarState {
                emotions: state.runtime_snapshot().companion_emotions,
                emotions_enabled: config.companion.emotions_enabled,
                language: config.ui.language.clone(),
                ..Default::default()
            };
            let popup = crate::capture::window::CapturePopupView::new(state.clone());
            let activation =
                crate::activation_policy::ActivationPolicy::native(state.clone(), popup.clone());
            let mut root = UiRoot::with_activation(
                crate::ui_port::DesktopUiPort::new(state.clone(), popup),
                receiver,
                activation,
            );
            root.snapshot = Some(crate::snapshot_presenter::SnapshotPresenter::new(
                state.snapshot.clone(),
                state.speech.lifecycle.clone(),
                state.shortcut_coordinator.clone(),
            ));
            root.capture = capture;
            root.bubble = BubblePresenter::new(state.bubbles.clone(), config);
            root.bubble.tutorial = Some(state.tutorial.clone());
            root.bubble.initialize(&initial);
            root.avatar = crate::avatar_presenter::AvatarPresenter::new(avatar);
            tauri::async_runtime::spawn(root.run());
        },
    )
}

#[cfg(test)]
pub(crate) fn test_channel_with_bubbles<P: UiPort>(
    port: P,
    model: Arc<tokio::sync::Mutex<crate::bubbles::BubbleState>>,
    config: coosenpai_core::config::Config,
) -> (UiHandle, tokio::task::JoinHandle<()>) {
    test_channel_with_bubble_tutorial(port, model, config, None)
}

#[cfg(test)]
pub(crate) fn test_channel_with_bubble_tutorial<P: UiPort>(
    port: P,
    model: Arc<tokio::sync::Mutex<crate::bubbles::BubbleState>>,
    config: coosenpai_core::config::Config,
    tutorial: Option<Arc<tokio::sync::Mutex<crate::tutorial::TutorialController>>>,
) -> (UiHandle, tokio::task::JoinHandle<()>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let mut root = UiRoot::new(port, receiver);
    let snapshot = crate::snapshot::AppSnapshot::initial(
        config.clone(),
        Vec::new(),
        coosenpai_core::ports::ScreenCapturePermission::from_preflight(false, false),
        Default::default(),
        0,
        0,
        false,
    );
    root.snapshot = Some(crate::snapshot_presenter::SnapshotPresenter::new(
        Arc::new(std::sync::Mutex::new(snapshot)),
        Arc::new(std::sync::Mutex::new(
            crate::speech_lifecycle::SpeechLifecycle::default(),
        )),
        Arc::new(crate::capture::ShortcutCoordinator::default()),
    ));
    root.bubble = BubblePresenter::new(model, config);
    root.bubble.tutorial = tutorial;
    (UiHandle { sender }, tokio::spawn(root.run()))
}

#[cfg(test)]
pub(crate) fn test_capture_channel<P: UiPort>(
    make_port: impl FnOnce(crate::capture::CaptureHandle) -> P,
    activation: crate::activation_policy::ActivationPolicy,
) -> (
    crate::capture::CaptureHandle,
    UiHandle,
    tokio::task::JoinHandle<()>,
) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let ui = UiHandle { sender };
    let (handle, capture) = crate::capture::channel(ui.clone());
    let mut root = UiRoot::with_activation(make_port(handle.clone()), receiver, activation);
    root.capture = capture;
    (handle, ui, tokio::spawn(root.run()))
}

impl<P: UiPort> UiRoot<P> {

    fn with_activation(
        port: P,
        receiver: mpsc::UnboundedReceiver<RootMessage>,
        activation: crate::activation_policy::ActivationPolicy,
    ) -> Self {
        let windows = [
            PresenterId::Details,
            PresenterId::Settings,
            PresenterId::ModelPicker,
        ]
        .into_iter()
        .map(|id| (id, WindowPresenter::new(id)))
        .collect();
        Self {
            model: UiModel::default(),
            activation,
            tray: Default::default(),
            snapshot: None,
            chat: ChatPresenter::default(),
            tutorial: crate::ui_commands::TutorialPresenter::default(),
            capture: crate::capture::CapturePresenter::default(),
            avatar: crate::avatar_presenter::AvatarPresenter::default(),
            bubble: BubblePresenter::default(),
            windows,
            port: Arc::new(port),
            receiver,
            operation_busy: false,
            deferred: std::collections::VecDeque::new(),
        }
    }

    async fn run(mut self) {
        let mut jobs = tokio::task::JoinSet::new();
        let mut ready = std::collections::VecDeque::new();
        while let Some(message) = self.receiver.recv().await {
            match message {
                RootMessage::Input(envelope) => {
                    let pipeline = Pipeline::new(envelope);
                    if pipeline.operation && self.operation_busy {
                        self.deferred.push_back(pipeline);
                    } else {
                        self.operation_busy |= pipeline.operation;
                        ready.push_back(pipeline);
                    }
                }
                RootMessage::Completed {
                    mut pipeline,
                    result,
                } => {
                    pipeline.pending.push_front(Pending::Event(
                        PresenterId::Root,
                        UiEvent::EffectCompleted(result),
                    ));
                    ready.push_back(pipeline);
                }
            }
            while let Some(pipeline) = ready.pop_front() {
                if let Some((pipeline, result)) = self.advance(pipeline, &mut jobs).await {
                    self.complete(pipeline, result, &mut ready).await;
                }
            }
            while let Some(result) = jobs.try_join_next() {
                result.expect("UI model task");
            }
        }
    }

    async fn complete(
        &mut self,
        pipeline: Pipeline,
        result: Result<Option<String>, String>,
        ready: &mut std::collections::VecDeque<Pipeline>,
    ) {
        if let Some(reply) = pipeline.reply {
            let _ = reply.send(result);
        } else if let Err(error) = result {
            let _ = self
                .port
                .execute(UiEffect::Log(format!("ui: failed error={error}")))
                .await;
        }
        if pipeline.operation {
            self.operation_busy = false;
            if let Some(next) = self.deferred.pop_front() {
                self.operation_busy = true;
                ready.push_back(next);
            }
        }
    }

    async fn advance(
        &mut self,
        mut pipeline: Pipeline,
        jobs: &mut tokio::task::JoinSet<()>,
    ) -> Option<(Pipeline, Result<Option<String>, String>)> {
        while let Some(next) = pipeline.pending.pop_front() {
            let (mut presenter, mut event) = match next {
                Pending::Effect(UiEffect::Deliver { child, event }) => {
                    pipeline.pending.push_front(Pending::Event(child, event));
                    continue;
                }
                Pending::Effect(UiEffect::Activation(input)) => {
                    for effect in self.activation.transition(input).into_iter().rev() {
                        pipeline.pending.push_front(Pending::Effect(effect));
                    }
                    continue;
                }
                Pending::Effect(UiEffect::View {
                    view: PresenterId::Chat,
                    command: command @ (ViewCommand::Show | ViewCommand::Front),
                }) => {
                    for effect in self
                        .activation
                        .transition(crate::activation_policy::ActivationInput::MainWindow(
                            command,
                        ))
                        .into_iter()
                        .rev()
                    {
                        pipeline.pending.push_front(Pending::Effect(effect));
                    }
                    continue;
                }
                Pending::Effect(UiEffect::Spawn(task)) => {
                    let port = self.port.clone();
                    let ui = UiHandle {
                        sender: pipeline.completion.clone(),
                    };
                    jobs.spawn(async move {
                        let result = run_task(port.as_ref(), task).await;
                        ui.input(UiView::Application, UiEvent::EffectCompleted(result));
                    });
                    continue;
                }
                Pending::Effect(UiEffect::Run(task)) => {
                    let port = self.port.clone();
                    let sender = pipeline.completion.clone();
                    jobs.spawn(async move {
                        let result = run_task(port.as_ref(), task).await;
                        let _ = sender.send(RootMessage::Completed { pipeline, result });
                    });
                    return None;
                }
                Pending::Effect(UiEffect::Complete(result)) => {
                    match result {
                        Ok(result) => {
                            for event in result.events.into_iter().rev() {
                                pipeline
                                    .pending
                                    .push_front(Pending::Event(PresenterId::Root, event));
                            }
                            if result.value.is_some() {
                                pipeline.value = result.value;
                            }
                        }
                        Err(error) => return Some((pipeline, Err(error))),
                    }
                    continue;
                }
                Pending::Effect(UiEffect::Fail(error)) => return Some((pipeline, Err(error))),
                Pending::Effect(effect) => {
                    let avatar_revision = match &effect {
                        UiEffect::AvatarRender(state) => Some(state.revision),
                        _ => None,
                    };
                    let failed_hide = match &effect {
                        UiEffect::View {
                            view,
                            command: ViewCommand::Hide,
                        } if *view == PresenterId::Chat
                            || navigation_children().any(|child| child == *view) =>
                        {
                            Some(*view)
                        }
                        _ => None,
                    };
                    let presentation_view = match &effect {
                        UiEffect::ActivationView(view) if view.presents_main() => {
                            Some(PresenterId::Chat)
                        }
                        UiEffect::RenderWindow(content) => Some(content.view()),
                        UiEffect::BubbleRender { .. } => Some(PresenterId::Bubble),
                        UiEffect::View {
                            view,
                            command: ViewCommand::Show,
                        } if matches!(
                            view,
                            PresenterId::Chat
                                | PresenterId::Settings
                                | PresenterId::Details
                                | PresenterId::ModelPicker
                                | PresenterId::Bubble
                        ) =>
                        {
                            Some(*view)
                        }
                        _ => None,
                    };
                    let result = match effect {
                        UiEffect::ActivationView(view) => view.apply(self.port.as_ref()).await,
                        effect => self.port.execute(effect).await,
                    };
                    match result {
                        Ok(result) => {
                            for event in result.events.into_iter().rev() {
                                pipeline
                                    .pending
                                    .push_front(Pending::Event(PresenterId::Root, event));
                            }
                            if result.value.is_some() {
                                pipeline.value = result.value;
                            }
                        }
                        Err(error) => {
                            if let Some(revision) = avatar_revision {
                                pipeline.pending.clear();
                                pipeline.pending.push_back(Pending::Event(
                                    PresenterId::Avatar,
                                    UiEvent::AvatarRenderFailed { revision, error },
                                ));
                            } else if let Some(view) = failed_hide {
                                pipeline.pending.clear();
                                for effect in self.activation.transition(
                                    crate::activation_policy::ActivationInput::NavigationFailed(
                                        error.clone(),
                                    ),
                                ) {
                                    pipeline.pending.push_back(Pending::Effect(effect));
                                }
                                pipeline.pending.push_back(Pending::Event(
                                    view,
                                    UiEvent::WindowHideFailed { view, error },
                                ));
                            } else if let Some(view) = presentation_view {
                                pipeline.pending.clear();
                                pipeline.pending.push_back(Pending::Event(
                                    PresenterId::Root,
                                    if view == PresenterId::Bubble {
                                        UiEvent::BubbleWindow(PresentationEvent::Hide)
                                    } else {
                                        UiEvent::Window {
                                            view,
                                            event: PresentationEvent::Hide,
                                        }
                                    },
                                ));
                                pipeline
                                    .pending
                                    .push_back(Pending::Effect(UiEffect::Fail(error)));
                            } else {
                                return Some((pipeline, Err(error)));
                            }
                        }
                    }
                    continue;
                }
                Pending::Event(presenter, event) => (presenter, event),
            };
            let source = presenter;
            let mut notices = Vec::new();
            let effects = loop {
                let label = event.label();
                let handling = match presenter {
                    PresenterId::Root => self.handle(source, event),
                    PresenterId::Chat => self.chat.handle(event),
                    PresenterId::Tutorial => self.tutorial.handle(event, self.model.main_focused),
                    PresenterId::Tray => self.tray.handle(event),
                    PresenterId::Avatar => self.avatar.handle(event),
                    PresenterId::Bubble => {
                        self.bubble
                            .handle_input(event, self.model.main_focused, self.chat.is_visible())
                            .await
                    }
                    PresenterId::Capture => self.capture.handle(event),
                    id => self
                        .windows
                        .iter_mut()
                        .find(|(registered, _)| *registered == id)
                        .expect("registered presenter")
                        .1
                        .handle(event),
                };
                match handling {
                    Handling::Handled(effects) => {
                        {
                            notices.push(UiEffect::Log(format!(
                                "ui: presenter={presenter:?} event={label} handled"
                            )));
                        }
                        break effects;
                    }
                    Handling::Bubble(next) => {
                        notices.push(UiEffect::Log(format!(
                            "ui: presenter={presenter:?} event={label} bubble"
                        )));
                        let Some(parent) = presenter.parent() else {
                            notices.push(UiEffect::Log(format!("ui: unhandled event={label}")));
                            break Vec::new();
                        };
                        presenter = parent;
                        event = next;
                    }
                }
            };
            for effect in notices
                .into_iter()
                .chain(effects)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
            {
                pipeline.pending.push_front(Pending::Effect(effect));
            }
        }
        let value = pipeline.value.take();
        Some((pipeline, Ok(value)))
    }

    fn start_capture(
        &self,
        kind: CaptureKind,
        source: crate::command_guard::CommandSource,
    ) -> Vec<UiEffect> {
        let mut effects = Vec::new();
        if let Some(active) = self.model.active_operation {
            if active.kind != kind {
                effects.push(UiEffect::Run(UiTask::StopOperation {
                    kind: active.kind,
                    generation: active.generation,
                }));
            }
        }
        effects.push(UiEffect::Run(UiTask::StartOperation { kind, source }));
        effects
    }

    fn handle(&mut self, source: PresenterId, event: UiEvent) -> Handling {
        if matches!(
            event,
            UiEvent::NativeShutdown(crate::shutdown::ExitKind::Signal)
        ) {
            self.model.shutdown_signals = self.model.shutdown_signals.saturating_add(1);
            if self.model.shutdown_signals > 1 {
                return Handling::Handled(vec![UiEffect::ForceShutdown]);
            }
        }
        let effects = match event {
            event @ UiEvent::Tutorial(_) => vec![UiEffect::Deliver {
                child: PresenterId::Tutorial,
                event,
            }],
            event @ UiEvent::PresenceTickCompleted { .. } => vec![UiEffect::Deliver {
                child: PresenterId::Chat,
                event,
            }],
            event @ UiEvent::FactCandidateLoaded(_) => vec![UiEffect::Deliver {
                child: PresenterId::Bubble,
                event,
            }],
            event @ (UiEvent::BubbleExpiry(_)
            | UiEvent::ThoughtObserved { .. }
            | UiEvent::ThoughtFlushExpired(_)
            | UiEvent::ThoughtClear { .. }) => vec![UiEffect::Deliver {
                child: PresenterId::Bubble,
                event,
            }],
            UiEvent::TrayMounted => {
                let snapshot = self
                    .snapshot
                    .as_ref()
                    .expect("snapshot presenter is initialized before tray mount")
                    .current();
                vec![UiEffect::Deliver {
                    child: PresenterId::Tray,
                    event: UiEvent::TrayReady(Arc::new(snapshot)),
                }]
            }
            event @ UiEvent::TrayReady(_) => vec![UiEffect::Deliver {
                child: PresenterId::Tray,
                event,
            }],
            UiEvent::WindowHideFailed { view, error } => vec![UiEffect::Deliver {
                child: view,
                event: UiEvent::WindowHideFailed { view, error },
            }],
            UiEvent::SpeechFailureExpired {
                generation,
                failure_id,
            } => self
                .snapshot
                .as_mut()
                .expect("snapshot presenter is initialized before accepting input")
                .expire_speech(generation, failure_id),
            UiEvent::SnapshotCompleted(event) => self
                .snapshot
                .as_mut()
                .expect("snapshot presenter is initialized before accepting input")
                .complete(*event),
            UiEvent::SnapshotResult(input) => self
                .snapshot
                .as_mut()
                .expect("snapshot presenter is initialized before accepting input")
                .handle(*input),
            UiEvent::UserCommand(command) => vec![UiEffect::Deliver {
                child: command.owner(),
                event: UiEvent::UserCommand(command),
            }],
            UiEvent::CommandFinished { owner, completion } => vec![UiEffect::Deliver {
                child: owner,
                event: UiEvent::CommandFinished { owner, completion },
            }],
            event @ UiEvent::ModelSaved { .. } => vec![UiEffect::Deliver {
                child: PresenterId::ModelPicker,
                event,
            }],
            event @ (UiEvent::Placement(_) | UiEvent::ChatInputActive(_) | UiEvent::UnreadRead) => {
                vec![UiEffect::Deliver {
                    child: PresenterId::Chat,
                    event,
                }]
            }
            event @ (UiEvent::BubbleAck { .. } | UiEvent::BubbleSnapshot(_)) => {
                vec![UiEffect::Deliver {
                    child: PresenterId::Bubble,
                    event,
                }]
            }

            event @ UiEvent::ChatProjection(_) => vec![UiEffect::Deliver {
                child: PresenterId::Chat,
                event,
            }],
            event @ (UiEvent::CaptureCompleted(_) | UiEvent::PopupWindow { .. }) => {
                let mut effects = Vec::new();
                if let UiEvent::CaptureCompleted(completed) = &event {
                    if let crate::capture::CaptureEvent::Applied(applied) = completed.as_ref() {
                        if let crate::capture::effects::CaptureOutput::Presented {
                            view: UiView::CapturePopup,
                            generation,
                        } = &applied.completion
                        {
                            if applied.result.is_err() {
                                effects.extend(self.activation.transition(
                                    crate::activation_policy::ActivationInput::PopupFailed(
                                        *generation,
                                    ),
                                ));
                            }
                        }
                    }
                }
                effects.push(UiEffect::Deliver {
                    child: PresenterId::Capture,
                    event,
                });
                effects
            }
            UiEvent::EffectCompleted(result) => vec![UiEffect::Complete(result)],
            UiEvent::Window { view, event } => {
                let mut effects = Vec::new();
                match &event {
                    PresentationEvent::Open(WindowRequest::PreparedMain(_)) => {}
                    PresentationEvent::Open(_) => {
                        effects.extend(
                            navigation_children()
                                .filter(|other| *other != view)
                                .map(|other| window(other, PresentationEvent::Hide)),
                        );
                    }
                    PresentationEvent::Hide => {
                        effects.extend(
                            view.attached_windows()
                                .iter()
                                .map(|child| window(*child, PresentationEvent::Hide)),
                        );
                    }
                    PresentationEvent::Loaded { .. } | PresentationEvent::CancelLoad { .. } => {}
                }
                effects.push(UiEffect::Deliver {
                    child: view,
                    event: UiEvent::Window { view, event },
                });
                effects
            }
            UiEvent::Refreshed { view, result } => vec![UiEffect::Deliver {
                child: view,
                event: UiEvent::Refreshed { view, result },
            }],
            UiEvent::PopupProtection { generation, active } => {
                if active {
                    self.model.protected_popup = Some(generation);
                } else if self.model.protected_popup == Some(generation) {
                    self.model.protected_popup = None;
                }
                Vec::new()
            }
            UiEvent::InterruptCapture(false) if self.model.protected_popup.is_some() => {
                vec![UiEffect::RejectInput]
            }
            UiEvent::InterruptCapture(selection_only) => {
                let mut effects = if selection_only {
                    Vec::new()
                } else {
                    hide_navigation()
                };
                effects.push(UiEffect::Run(UiTask::InterruptCapture(selection_only)));
                effects
            }
            UiEvent::GlobalShortcut { shortcut, pressed } => {
                vec![UiEffect::Run(UiTask::ResolveShortcut { shortcut, pressed })]
            }
            UiEvent::ShortcutResolved {
                action,
                pressed,
                voice_mode,
                popup_generation,
            } => {
                use crate::capture::ShortcutAction;
                let event = match action {
                    Some(ShortcutAction::CaptureRegion) if pressed => {
                        UiEvent::Shortcut(CaptureKind::Image)
                    }
                    Some(ShortcutAction::SendText) if pressed => {
                        UiEvent::Shortcut(CaptureKind::Text)
                    }
                    Some(ShortcutAction::TogglePanel) if pressed => UiEvent::ToggleMain,
                    Some(ShortcutAction::Microphone) if pressed => {
                        if voice_mode == "toggle" && self.model.speech_phase == "recording" {
                            UiEvent::Voice(VoiceAction::Finish)
                        } else {
                            UiEvent::Shortcut(CaptureKind::Voice)
                        }
                    }
                    Some(ShortcutAction::Microphone) if voice_mode != "toggle" => {
                        UiEvent::Voice(VoiceAction::Finish)
                    }
                    Some(ShortcutAction::PopupCancel) if pressed => {
                        if let Some(generation) = popup_generation {
                            return Handling::Handled(vec![UiEffect::Deliver {
                                child: PresenterId::Capture,
                                event: UiEvent::CaptureCancel {
                                    generation,
                                    source: crate::capture::CancelSource::Esc,
                                },
                            }]);
                        }
                        return Handling::Handled(Vec::new());
                    }
                    Some(ShortcutAction::SpeechCancel) if pressed => {
                        UiEvent::Voice(VoiceAction::Cancel)
                    }
                    Some(ShortcutAction::ToggleAvatar) if pressed => {
                        UiEvent::AvatarVisibility(None)
                    }
                    Some(ShortcutAction::ToggleWatch) if pressed => {
                        return Handling::Handled(vec![UiEffect::Run(UiTask::ToggleWatch)])
                    }
                    Some(ShortcutAction::CopyLastReply) if pressed => {
                        return Handling::Handled(vec![UiEffect::Run(UiTask::CopyLastReply)])
                    }
                    _ => return Handling::Handled(Vec::new()),
                };
                return self.handle(source, event);
            }
            UiEvent::Shortcut(kind)
                if self.model.protected_popup.is_some()
                    && self
                        .model
                        .active_operation
                        .is_none_or(|active| active.kind != kind) =>
            {
                vec![UiEffect::RejectInput]
            }
            UiEvent::Voice(VoiceAction::Start(_)) if self.model.protected_popup.is_some() => {
                vec![UiEffect::RejectInput]
            }
            UiEvent::Shortcut(kind) => {
                if let Some(snapshot) = self.snapshot.as_mut() {
                    snapshot.input_started();
                }
                let source = if source == PresenterId::Chat {
                    crate::command_guard::CommandSource::IpcMain
                } else {
                    crate::command_guard::CommandSource::GlobalShortcut
                };
                self.start_capture(kind, source)
            }
            UiEvent::Activation(input) => self.activation.transition(input),
            UiEvent::OtherApplicationActivated => self
                .activation
                .transition(crate::activation_policy::ActivationInput::OtherApplicationActivated),
            UiEvent::ApplicationDeactivated => self
                .activation
                .transition(crate::activation_policy::ActivationInput::Deactivated),
            UiEvent::Voice(VoiceAction::Start(source)) => {
                if let Some(snapshot) = self.snapshot.as_mut() {
                    snapshot.input_started();
                }
                let mut effects = if source == crate::speech::SpeechSource::Composer {
                    hide_navigation_children()
                } else {
                    hide_navigation()
                };
                if let Some(active) = self.model.active_operation {
                    if active.kind != CaptureKind::Voice {
                        effects.push(UiEffect::Run(UiTask::StopOperation {
                            kind: active.kind,
                            generation: active.generation,
                        }));
                    }
                }
                effects.push(UiEffect::Run(UiTask::Voice(VoiceAction::Start(source))));
                effects
            }
            UiEvent::Voice(action) => {
                let mut effects = Vec::new();
                if action == VoiceAction::Cancel {
                    effects.push(UiEffect::Deliver {
                        child: PresenterId::Capture,
                        event: UiEvent::PopupWindow {
                            view: crate::ui_events::UiView::SpeechPopup,
                            event: PresentationEvent::Hide,
                        },
                    });
                }
                effects.push(UiEffect::Run(UiTask::Voice(action)));
                effects
            }
            UiEvent::OperationStarted { kind, generation } => {
                if generation > self.model.completed_generation {
                    self.model.active_operation = Some(ActiveOperation { kind, generation });
                }
                Vec::new()
            }
            UiEvent::OperationEnded { kind, generation } => {
                self.model.completed_generation = self.model.completed_generation.max(generation);
                if self.model.active_operation == Some(ActiveOperation { kind, generation }) {
                    self.model.active_operation = None;
                }
                Vec::new()
            }
            UiEvent::SpeechTranscript { generation, text } => vec![UiEffect::Deliver {
                child: PresenterId::Chat,
                event: UiEvent::SpeechTranscript { generation, text },
            }],
            UiEvent::SubmitInput(input) => vec![UiEffect::Deliver {
                child: PresenterId::Chat,
                event: UiEvent::SubmitInput(input),
            }],
            UiEvent::BubbleTypingTick(epoch) => vec![UiEffect::Deliver {
                child: PresenterId::Bubble,
                event: UiEvent::BubbleTypingTick(epoch),
            }],
            UiEvent::BubbleHideExpired(generation) => vec![UiEffect::Deliver {
                child: PresenterId::Bubble,
                event: UiEvent::BubbleHideExpired(generation),
            }],
            UiEvent::RequestFocus if self.model.active_operation.is_none() => match source {
                PresenterId::Chat => {
                    vec![window(source, PresentationEvent::Open(WindowRequest::Main))]
                }
                PresenterId::Details => vec![window(
                    source,
                    PresentationEvent::Open(WindowRequest::Details),
                )],
                PresenterId::Settings => vec![window(
                    source,
                    PresentationEvent::Open(WindowRequest::Settings { section: None }),
                )],
                PresenterId::ModelPicker => vec![window(
                    source,
                    PresentationEvent::Open(WindowRequest::ModelPicker),
                )],
                _ => vec![deliver(source, ViewCommand::Front)],
            },
            UiEvent::RequestFocus => Vec::new(),
            UiEvent::PointerPassthrough
                if self.model.active_operation.is_none() && !self.operation_busy =>
            {
                vec![
                    UiEffect::Pointer(false),
                    UiEffect::Run(UiTask::Delay {
                        duration: std::time::Duration::from_millis(300),
                        event: UiEvent::RestorePointer,
                    }),
                ]
            }
            UiEvent::PointerPassthrough => Vec::new(),
            UiEvent::RestorePointer
                if self.model.active_operation.is_none() && !self.operation_busy =>
            {
                vec![UiEffect::Deliver {
                    child: PresenterId::Bubble,
                    event: UiEvent::RestorePointer,
                }]
            }
            UiEvent::RestorePointer => vec![UiEffect::Run(UiTask::Delay {
                duration: std::time::Duration::from_millis(300),
                event: UiEvent::RestorePointer,
            })],
            event @ (UiEvent::OsNotificationPrepared { .. }
            | UiEvent::BubbleClickPrepared { .. }
            | UiEvent::BubbleClickCompleted(_)
            | UiEvent::BubbleMutation { .. }
            | UiEvent::BubbleWindow(_)
            | UiEvent::NotificationFinished(_)
            | UiEvent::ThoughtRequested { .. }
            | UiEvent::NotificationRequested { .. }
            | UiEvent::BubbleRequested { .. }
            | UiEvent::ResetPromptRequested
            | UiEvent::BubbleRendererReady { .. }
            | UiEvent::BubbleRefresh) => vec![UiEffect::Deliver {
                child: PresenterId::Bubble,
                event,
            }],
            event @ (UiEvent::AvatarVisibility(_)
            | UiEvent::AvatarWindow(_)
            | UiEvent::AvatarRead { .. }
            | UiEvent::AvatarRenderFailed { .. }
            | UiEvent::AvatarApplied { .. }) => vec![UiEffect::Deliver {
                child: PresenterId::Avatar,
                event,
            }],
            UiEvent::EmptyClipboard => vec![UiEffect::Run(UiTask::EmptyClipboardNotice(
                if self.chat.is_visible() && self.model.main_focused {
                    crate::capture_notice::NoticeTarget::Status
                } else {
                    crate::capture_notice::NoticeTarget::Bubble
                },
            ))],
            UiEvent::FocusComposer => vec![deliver(PresenterId::Chat, ViewCommand::FocusInput)],
            UiEvent::SelectConversation(id) => {
                let mut effects = hide_navigation_children();
                effects.push(UiEffect::SelectConversation(id.clone()));
                effects.push(UiEffect::Deliver {
                    child: PresenterId::Chat,
                    event: UiEvent::Conversation(
                        crate::conversation_presenter::ConversationEvent::Selected(id),
                    ),
                });
                effects
            }
            UiEvent::OpenMain => vec![window(
                PresenterId::Chat,
                PresentationEvent::Open(WindowRequest::Main),
            )],
            UiEvent::ToggleMain => {
                if self.chat.presentation() == PresentationState::Hidden {
                    vec![window(
                        PresenterId::Chat,
                        PresentationEvent::Open(WindowRequest::Main),
                    )]
                } else {
                    vec![window(PresenterId::Chat, PresentationEvent::Hide)]
                }
            }
            UiEvent::MainVisibility(visible) => {
                if !visible {
                    self.model.main_focused = false;
                }
                vec![UiEffect::MainFocus(self.model.main_focused)]
            }
            UiEvent::MainFocused(focused) => {
                self.model.main_focused = focused;
                let mut effects = vec![UiEffect::MainFocus(focused)];
                if focused {
                    effects.push(deliver(PresenterId::Bubble, ViewCommand::Hide));
                }
                effects
            }
            UiEvent::SnapshotUpdated(snapshot) => {
                let focus_after_speech =
                    self.model.speech_phase == "sending" && snapshot.speech.phase == "idle";
                self.model.speech_phase = snapshot.speech.phase.clone();
                let mut effects: Vec<_> = [
                    PresenterId::Chat,
                    PresenterId::Capture,
                    PresenterId::Details,
                    PresenterId::ModelPicker,
                    PresenterId::Avatar,
                    PresenterId::Bubble,
                ]
                .into_iter()
                .map(|child| UiEffect::Deliver {
                    child,
                    event: UiEvent::SnapshotUpdated(snapshot.clone()),
                })
                .collect();
                effects.insert(
                    0,
                    UiEffect::Deliver {
                        child: PresenterId::Tray,
                        event: UiEvent::SnapshotUpdated(snapshot.clone()),
                    },
                );
                if focus_after_speech {
                    effects.push(deliver(PresenterId::Chat, ViewCommand::FocusInput));
                }
                effects
            }
            UiEvent::OpenSettings => vec![window(
                PresenterId::Settings,
                PresentationEvent::Open(WindowRequest::Settings { section: None }),
            )],
            UiEvent::OpenSettingsAt(section) => vec![window(
                PresenterId::Settings,
                PresentationEvent::Open(WindowRequest::Settings {
                    section: Some(section),
                }),
            )],
            UiEvent::OpenDetails => vec![window(
                PresenterId::Details,
                PresentationEvent::Open(WindowRequest::Details),
            )],
            UiEvent::OpenModelPicker => vec![window(
                PresenterId::ModelPicker,
                PresentationEvent::Open(WindowRequest::ModelPicker),
            )],
            UiEvent::Close if source != PresenterId::Root => {
                vec![window(source, PresentationEvent::Hide)]
            }
            event @ (UiEvent::Shutdown | UiEvent::NativeShutdown(_)) => {
                vec![match event {
                    UiEvent::NativeShutdown(kind) => UiEffect::Run(UiTask::NativeShutdown(kind)),
                    _ => UiEffect::Run(UiTask::Shutdown),
                }]
            }
            UiEvent::Panel {
                owner,
                request,
                reply,
            } => vec![UiEffect::Deliver {
                child: owner,
                event: UiEvent::Panel {
                    owner,
                    request,
                    reply,
                },
            }],
            event @ (UiEvent::SettingsPreview { .. } | UiEvent::SettingsPreviewCompleted(_)) => {
                vec![UiEffect::Deliver {
                    child: PresenterId::Settings,
                    event,
                }]
            }
            event @ (UiEvent::AvatarScene(_)
            | UiEvent::AvatarMotionsChanged
            | UiEvent::AvatarPreview(_)) => vec![UiEffect::Deliver {
                child: PresenterId::Avatar,
                event,
            }],
            event @ UiEvent::MotionSettings(_) => vec![UiEffect::Deliver {
                child: PresenterId::Settings,
                event,
            }],
            event @ (UiEvent::Composer(_)
            | UiEvent::Conversation(_)
            | UiEvent::ChatLoaded(_)
            | UiEvent::WorkApproval(_)
            | UiEvent::App(_)
            | UiEvent::StatusDeadline(_)) => vec![UiEffect::Deliver {
                child: PresenterId::Chat,
                event,
            }],
            event @ UiEvent::BubbleView(_) => vec![UiEffect::Deliver {
                child: PresenterId::Bubble,
                event,
            }],
            event @ UiEvent::ModelPicker(_) => vec![UiEffect::Deliver {
                child: PresenterId::ModelPicker,
                event,
            }],
            event => return Handling::Bubble(event),
        };
        Handling::Handled(effects)
    }
}

fn deliver(child: PresenterId, command: ViewCommand) -> UiEffect {
    UiEffect::Deliver {
        child,
        event: UiEvent::Present(command),
    }
}

fn window(view: PresenterId, event: crate::ui_load::WindowEvent) -> UiEffect {
    UiEffect::Deliver {
        child: PresenterId::Root,
        event: UiEvent::Window { view, event },
    }
}

fn navigation_children() -> impl Iterator<Item = PresenterId> {
    [
        PresenterId::Settings,
        PresenterId::Details,
        PresenterId::ModelPicker,
    ]
    .into_iter()
}

fn hide_navigation_children() -> Vec<UiEffect> {
    navigation_children()
        .map(|view| window(view, PresentationEvent::Hide))
        .collect()
}

pub(crate) fn hide_navigation() -> Vec<UiEffect> {
    vec![
        window(PresenterId::Chat, PresentationEvent::Hide),
        window(PresenterId::Details, PresentationEvent::Hide),
    ]
}

enum Pending {
    Event(PresenterId, UiEvent),
    Effect(UiEffect),
}

struct Pipeline {
    pending: std::collections::VecDeque<Pending>,
    reply: Option<oneshot::Sender<Result<Option<String>, String>>>,
    value: Option<String>,
    operation: bool,
    completion: mpsc::UnboundedSender<RootMessage>,
}

impl Pipeline {
    fn new(envelope: Envelope) -> Self {
        let operation = matches!(
            envelope.event,
            UiEvent::GlobalShortcut { .. }
                | UiEvent::Shortcut(_)
                | UiEvent::Voice(_)
                | UiEvent::RequestFocus
                | UiEvent::Shutdown
        );
        Self {
            pending: std::collections::VecDeque::from([Pending::Event(
                envelope.presenter,
                envelope.event,
            )]),
            reply: envelope.reply,
            value: None,
            operation,
            completion: envelope.completion,
        }
    }
}

async fn run_task(port: &impl UiPort, task: UiTask) -> Result<EffectResult, String> {
    match task {
        UiTask::Activation(task) => task.run().await,
        task => port.run(task).await,
    }
}
