use crate::presentation::{Presentation, PresentationAction, PresentationEvent, PresentationState};
use crate::ui_events::{Handling, PresenterId, UiEffect, UiEvent, UiTask, ViewCommand};
use crate::ui_load::{WindowContent, WindowRequest};

pub(crate) struct ChatPresenter {
    placement: crate::placement_presenter::PlacementPresenter,
    composer: crate::composer_presenter::ComposerPresenter,
    app: crate::app_presenter::AppPresenter,
    work_approval: crate::work_approval_presenter::WorkApprovalPresenter,
    conversation: crate::conversation_presenter::ConversationPresenter,
    snapshot: Option<std::sync::Arc<crate::snapshot::AppSnapshot>>,
    window: WindowPresenter,
}

impl Default for ChatPresenter {
    fn default() -> Self {
        Self {
            window: WindowPresenter::new(PresenterId::Chat),
            placement: Default::default(),
            composer: Default::default(),
            app: Default::default(),
            work_approval: Default::default(),
            conversation: Default::default(),
            snapshot: None,
        }
    }
}

impl ChatPresenter {
    pub(crate) fn is_visible(&self) -> bool {
        matches!(
            self.presentation(),
            PresentationState::Shown | PresentationState::CloseFailed
        )
    }

    pub(crate) fn presentation(&self) -> PresentationState {
        self.window.presentation.state()
    }

    pub(crate) fn handle(&mut self, event: UiEvent) -> Handling {
        match event {
            UiEvent::App(event) => Handling::Handled(self.app.handle(event)),
            UiEvent::StatusDeadline(deadline) => Handling::Handled(self.app.deadline(deadline)),
            UiEvent::Mounted(_) => Handling::Handled(self.app.handle(
                crate::app_presenter::AppEvent::Input(crate::app_presenter::AppInput::Mounted),
            )),
            UiEvent::Present(ViewCommand::FocusInput) => {
                let mut effects = self
                    .app
                    .handle(crate::app_presenter::AppEvent::FocusComposer);
                if let Handling::Handled(window) = self
                    .window
                    .handle(UiEvent::Present(ViewCommand::FocusInput))
                {
                    effects.extend(window);
                }
                Handling::Handled(effects)
            }
            UiEvent::WorkApproval(event) => Handling::Handled(self.work_approval.handle(event)),
            UiEvent::Composer(event) => {
                let mut effects = self.composer.handle(event);
                if let Some(snapshot) = &self.snapshot {
                    effects.extend(
                        self.conversation
                            .observe(snapshot.clone(), self.composer.pending_sends()),
                    );
                }
                Handling::Handled(effects)
            }
            UiEvent::Conversation(event) => {
                let mut effects = if matches!(
                    &event,
                    crate::conversation_presenter::ConversationEvent::Selected(_)
                ) {
                    self.app
                        .handle(crate::app_presenter::AppEvent::ConversationSelected)
                } else {
                    vec![]
                };
                let mounted = matches!(
                    &event,
                    crate::conversation_presenter::ConversationEvent::Input(
                        crate::conversation_presenter::ConversationInput::Mounted
                    )
                );
                effects.extend(self.conversation.handle(event));
                self.composer
                    .set_operation_busy(self.conversation.is_busy());
                effects.push(self.composer.render());
                if mounted {
                    effects.extend(
                        self.app
                            .handle(crate::app_presenter::AppEvent::ComposerReady),
                    );
                }
                Handling::Handled(effects)
            }
            UiEvent::ChatLoaded(snapshot) => Handling::Handled(self.observe(snapshot.clone())),
            UiEvent::SnapshotUpdated(snapshot) => {
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|current| current.revision >= snapshot.revision)
                {
                    return Handling::Handled(vec![]);
                }
                let mut effects = self.observe(snapshot.clone());
                if let Handling::Handled(render) =
                    self.window.handle(UiEvent::SnapshotUpdated(snapshot))
                {
                    effects.extend(render);
                }
                Handling::Handled(effects)
            }
            UiEvent::PresenceTickCompleted { date, result } => {
                Handling::Handled(crate::presence_presenter::tick_completed(date, result))
            }
            UiEvent::Placement(event) => Handling::Handled(self.placement.handle(event)),
            UiEvent::UserCommand(command) if command.owner() == PresenterId::Chat => {
                Handling::Handled(vec![UiEffect::Spawn(UiTask::UserCommand(command))])
            }
            UiEvent::CommandFinished {
                owner: PresenterId::Chat,
                completion,
            } => {
                completion.reply();
                Handling::Handled(Vec::new())
            }
            UiEvent::ChatInputActive(active) => {
                Handling::Handled(vec![UiEffect::ChatInputActive(active)])
            }
            UiEvent::UnreadRead => Handling::Handled(vec![UiEffect::Deliver {
                child: PresenterId::Root,
                event: UiEvent::SnapshotCompleted(Box::new(
                    crate::snapshot_presenter::SnapshotEvent::UnreadRead,
                )),
            }]),
            UiEvent::ChatProjection(projection) => {
                Handling::Handled(vec![UiEffect::ChatProjection(projection)])
            }
            UiEvent::SpeechTranscript { generation, text } => {
                Handling::Handled(self.composer.transcript(generation, text))
            }
            UiEvent::SubmitInput(input) => {
                Handling::Handled(vec![UiEffect::Run(UiTask::SubmitInput(input))])
            }
            UiEvent::SubmitChat(message) => {
                Handling::Handled(vec![UiEffect::Run(UiTask::SubmitChat(message))])
            }
            event => self.window.handle(event),
        }
    }
}

impl ChatPresenter {
    fn observe(&mut self, snapshot: std::sync::Arc<crate::snapshot::AppSnapshot>) -> Vec<UiEffect> {
        if self
            .snapshot
            .as_ref()
            .is_some_and(|current| current.revision >= snapshot.revision)
        {
            return vec![];
        }
        self.snapshot = Some(snapshot.clone());
        self.work_approval.observe(&snapshot);
        let mut effects = self
            .composer
            .observe(&snapshot, chrono::Local::now().date_naive());
        effects.extend(
            self.conversation
                .observe(snapshot.clone(), self.composer.pending_sends()),
        );
        effects.extend(self.app.observe(snapshot));
        effects
    }
}

pub(crate) struct WindowPresenter {
    id: PresenterId,
    presentation: Presentation,
    panels: crate::panels::PanelPresenters,
    preview: crate::settings_preview_presenter::SettingsPreviewPresenter,
    model_picker: Option<crate::model_picker_presenter::ModelPickerPresenter>,
    motion: Option<crate::motion_settings_presenter::MotionSettingsPresenter>,
}

impl WindowPresenter {
    pub(crate) fn new(id: PresenterId) -> Self {
        Self {
            id,
            presentation: Presentation::default(),
            panels: Default::default(),
            preview: Default::default(),
            model_picker: (id == PresenterId::ModelPicker).then(Default::default),
            motion: (id == PresenterId::Settings).then(Default::default),
        }
    }

    pub(crate) fn handle(&mut self, event: UiEvent) -> Handling {
        let effects = match event {
            UiEvent::SettingsPreview { preview, reply } if self.id == PresenterId::Settings => {
                self.preview.request(preview, reply)
            }
            UiEvent::SettingsPreviewCompleted(result) if self.id == PresenterId::Settings => {
                self.preview.completed(result)
            }
            UiEvent::Panel {
                owner,
                request,
                reply,
            } if owner == self.id => {
                vec![UiEffect::PanelOutput {
                    reply,
                    output: self.panels.handle(request),
                }]
            }
            UiEvent::MotionSettings(event) if self.id == PresenterId::Settings => self
                .motion
                .as_mut()
                .unwrap()
                .handle(event, std::time::Instant::now()),
            UiEvent::ModelPicker(event) if self.id == PresenterId::ModelPicker => {
                self.model_picker.as_mut().unwrap().handle(event)
            }
            UiEvent::Mounted(_) if self.id == PresenterId::ModelPicker => {
                self.model_picker.as_mut().unwrap().mount()
            }
            UiEvent::UserCommand(crate::ui_commands::UserCommand::ModelSave { patch, reply })
                if self.id == PresenterId::ModelPicker =>
            {
                vec![UiEffect::Spawn(UiTask::SaveModel {
                    patch,
                    generation: self.presentation.generation(),
                    reply,
                })]
            }
            UiEvent::UserCommand(command) if command.owner() == self.id => {
                vec![UiEffect::Spawn(UiTask::UserCommand(command))]
            }
            UiEvent::CommandFinished { owner, completion } if owner == self.id => {
                completion.reply();
                Vec::new()
            }
            UiEvent::ModelSaved {
                generation,
                result,
                reply,
            } if self.id == PresenterId::ModelPicker => {
                let reload = matches!(*result, crate::commands::IpcResult::Success { .. })
                    && generation == self.presentation.generation()
                    && self.presentation.state() != PresentationState::Hidden;
                let _ = reply.send(*result);
                if reload {
                    self.present(PresentationEvent::Open(WindowRequest::ModelPicker))
                } else {
                    Vec::new()
                }
            }

            UiEvent::WindowHideFailed { view, error } if view == self.id => {
                self.presentation.close_failed();
                vec![UiEffect::Fail(error)]
            }
            UiEvent::Window { view, event } if view == self.id => self.present(event),
            UiEvent::Refreshed { view, result } if view == self.id => match result {
                Ok(content) => {
                    let mut effects = Vec::new();
                    if let WindowContent::Main(main) = &content {
                        effects.push(UiEffect::Deliver {
                            child: PresenterId::Chat,
                            event: UiEvent::ChatLoaded(main.snapshot.clone()),
                        });
                    }
                    effects.push(UiEffect::RenderWindow(content));
                    effects
                }
                Err(error) => vec![UiEffect::Fail(error)],
            },
            UiEvent::SnapshotUpdated(snapshot) => {
                let mut effects = Vec::new();
                if let Some(model) = &mut self.model_picker {
                    model.observe(snapshot.clone());
                    effects.push(model.render());
                }
                effects.push(UiEffect::RenderSnapshot {
                    view: self.id,
                    snapshot,
                });
                effects
            }
            UiEvent::Mounted(_) => {
                vec![UiEffect::Run(UiTask::Refresh(self.id))]
            }
            UiEvent::Present(command) if command == ViewCommand::FocusInput => {
                vec![self.command(command)]
            }
            event => return Handling::Bubble(event),
        };
        Handling::Handled(effects)
    }

    fn command(&self, command: ViewCommand) -> UiEffect {
        UiEffect::View {
            view: self.id,
            command,
        }
    }

    fn present(&mut self, event: crate::ui_load::WindowEvent) -> Vec<UiEffect> {
        if self.id == PresenterId::Settings
            && matches!(event, PresentationEvent::Hide)
            && self.panels.blocks_settings_close()
        {
            return Vec::new();
        }
        if self.id == PresenterId::Details && matches!(event, PresentationEvent::Hide) {
            self.panels.details_hidden();
        }
        if self.id == PresenterId::Settings && matches!(event, PresentationEvent::Hide) {
            self.panels.settings_hidden();
        }
        if matches!(&event, PresentationEvent::Open(_) | PresentationEvent::Hide) {
            if let Some(model) = &mut self.model_picker {
                model.invalidate();
            }
        }
        match self.presentation.transition(event) {
            PresentationAction::Load { generation, request: WindowRequest::PreparedMain(content) } => vec![UiEffect::Deliver {
                child: PresenterId::Root,
                event: UiEvent::Window {
                    view: self.id,
                    event: PresentationEvent::Loaded { generation, result: Ok(Some(WindowContent::Main(content))) },
                },
            }],
            PresentationAction::Load { generation, request: WindowRequest::BubbleClick(target) } => vec![UiEffect::Run(UiTask::BubbleClick { generation, target })],
            PresentationAction::Load { generation, request } => vec![UiEffect::Run(UiTask::Load { view: self.id, generation, request })],
            PresentationAction::Show(content) => {
                let mut effects = Vec::new();
                let mut section = None;
                let mut advance = false;
                let mut bubble_click = None;
                match &content {
                    WindowContent::Main(main) => {
                        advance = main.advance_tutorial;
                        bubble_click = main.bubble_click.clone();
                        effects.push(UiEffect::Deliver {
                            child: PresenterId::Bubble,
                            event: if bubble_click.is_some() {
                                UiEvent::BubbleWindow(PresentationEvent::Hide)
                            } else {
                                UiEvent::Present(ViewCommand::Hide)
                            },
                        });
                        effects.push(UiEffect::Deliver { child: PresenterId::Chat, event: UiEvent::ChatLoaded(main.snapshot.clone()) });
                    }
                    WindowContent::Settings { main, section: selected, .. } => {
                        effects.push(UiEffect::Deliver { child: PresenterId::Root, event: UiEvent::Window {
                            view: PresenterId::Chat,
                            event: PresentationEvent::Open(WindowRequest::PreparedMain(main.clone())),
                        }});
                        section = *selected;
                    }
                    WindowContent::ModelPicker { snapshot, catalog } => {
                        effects.extend(self.model_picker.as_mut().unwrap().loaded(snapshot.clone(), catalog.clone()));
                    }
                    WindowContent::Details { .. } => {}
                }
                effects.push(UiEffect::RenderWindow(content));
                effects.push(self.command(ViewCommand::Show));
                if let Some(target) = bubble_click {
                    effects.push(UiEffect::Deliver {
                        child: PresenterId::Bubble,
                        event: UiEvent::BubbleClickCompleted(target),
                    });
                } else if self.id == PresenterId::Chat {
                    effects.push(self.command(ViewCommand::FocusInput));
                }
                if let Some(section) = section { effects.push(UiEffect::SettingsFocus(section)); }
                if self.id == PresenterId::Settings { effects.push(UiEffect::Deliver { child: PresenterId::Chat, event: UiEvent::App(crate::app_presenter::AppEvent::SettingsShown(section)) }); }
                if advance { effects.push(UiEffect::Run(UiTask::MainOpened)); }
                effects
            }
            PresentationAction::Hide => {
                let mut effects = vec![self.command(ViewCommand::Hide)];
                if self.id == PresenterId::Settings { effects.push(UiEffect::Deliver { child: PresenterId::Chat, event: UiEvent::App(crate::app_presenter::AppEvent::SettingsClosed) }); }
                effects
            },
            PresentationAction::Unavailable => vec![self.command(ViewCommand::Hide), UiEffect::Run(UiTask::AnnounceSetup)],
            PresentationAction::Failed(error) => vec![self.command(ViewCommand::Hide), UiEffect::Fail(error)],
            PresentationAction::Ignored { received, expected } => vec![UiEffect::Log(format!(
                "ui: presenter={:?} event=Loaded({received}) ignored=true reason=stale-generation expected={expected:?}", self.id
            ))],
        }
    }
}

pub(crate) use crate::bubbles::presenter::BubblePresenter;
