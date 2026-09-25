use crate::commands::IpcResult;
use crate::snapshot::AppSnapshot;
use crate::status_presenter::{RecoveryAction, StatusPresenter};
use crate::ui_commands::UserCommand;
use crate::ui_events::{PresenterId, UiEffect, UiEvent, UiTask, UiView};
use crate::ui_load::WindowContent;
use coosenpai_core::config::Config;
use coosenpai_core::ports::RuntimeLogger;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum AppInput {
    Mounted,
    Retry,
    StartupSettings,
    OpenSettings,
    CloseSettings,
    ToggleHistory,
    ResetOpen,
    ResetCancel,
    ResetConfirm,
    MenuToggle,
    MenuDismiss,
    MenuModel,
    MenuReset,
    ToggleWatch,
    ToggleAudio,
    TutorialNext,
    TutorialFinish,
    Recover,
    DismissBanner,
    SettingsPresented { generation: u64 },
    Report { error: Option<String> },
}
#[derive(Debug)]
pub(crate) enum AppEvent {
    Input(AppInput),
    SettingsShown(Option<&'static str>),
    SettingsClosed,
    FocusComposer,
    ComposerReady,
    ConversationSelected,
    Loaded {
        generation: u64,
        result: Result<WindowContent, String>,
    },
    Completed {
        token: u64,
        result: Result<Arc<AppSnapshot>, String>,
    },
    SelectPersona {
        persona: String,
        reply: crate::ui_commands::Reply<Config>,
    },
    PersonaSelected {
        result: Box<IpcResult<Config>>,
        reply: crate::ui_commands::Reply<Config>,
    },
    PersonasChanged,
    PersonasLoaded {
        generation: u64,
        result: Result<Vec<crate::factory::PersonaOption>, String>,
    },
}
#[derive(Clone, Copy, Debug)]
pub(crate) enum AppOperation {
    Watch(bool),
    Audio(bool),
    Next,
    RetryChat,
    Finish,
    SettingsAck,
    Reset,
    Recover(RecoveryAction),
}
#[derive(Debug)]
pub(crate) enum AppTask {
    Load(u64),
    Operation {
        token: u64,
        operation: AppOperation,
        config_revision: u64,
    },
    SelectPersona {
        setup: bool,
        persona: String,
        reply: crate::ui_commands::Reply<Config>,
    },
    Personas(u64),
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppView {
    pub screen: &'static str,
    pub loading: bool,
    pub error: Option<String>,
    pub finish_busy: bool,
    pub settings: &'static str,
    pub settings_focus: Option<&'static str>,
    pub settings_generation: u64,
    pub focus_request: u64,
    pub history_open: bool,
    pub reset_confirm_open: bool,
    pub menu_open: bool,
    pub can_reset: bool,
    pub watch_changing: bool,
    pub audio_changing: bool,
    pub tutorial: Option<crate::tutorial_ui::TutorialUi>,
}
// 配信専用の描画フレーム。snapshot は観測値の投影で、Presenter は保持しない。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppFrame {
    #[serde(flatten)]
    pub view: AppView,
    pub snapshot: Option<AppSnapshot>,
}
#[derive(Clone, Copy, Default)]
struct QueuedToggle {
    target: bool,
    base_config_revision: u64,
}
#[derive(Default)]
struct ToggleIntent {
    desired: Option<QueuedToggle>,
    running: bool,
    running_target: Option<bool>,
}
pub(crate) struct AppPresenter {
    view: AppView,
    status: StatusPresenter,
    pending_focus: bool,
    mounted: bool,
    load_generation: u64,
    personas_generation: u64,
    acked_generation: Option<u64>,
    next_token: u64,
    operations: BTreeMap<u64, AppOperation>,
    watch: ToggleIntent,
    audio: ToggleIntent,
}
impl Default for AppPresenter {
    fn default() -> Self {
        Self {
            view: AppView {
                screen: "loading",
                loading: false,
                error: None,
                finish_busy: false,
                settings: "closed",
                settings_focus: None,
                settings_generation: 0,
                focus_request: 0,
                history_open: false,
                reset_confirm_open: false,
                menu_open: false,
                can_reset: false,
                watch_changing: false,
                audio_changing: false,
                tutorial: None,
            },
            status: Default::default(),
            pending_focus: false,
            mounted: false,
            load_generation: 0,
            personas_generation: 0,
            acked_generation: None,
            next_token: 0,
            operations: BTreeMap::new(),
            watch: Default::default(),
            audio: Default::default(),
        }
    }
}
impl AppPresenter {
    pub(crate) fn observe(
        &mut self,
        snapshot: &Arc<AppSnapshot>,
        highlight_changed: bool,
    ) -> Vec<UiEffect> {
        if highlight_changed && self.view.settings != "closed" {
            self.view.settings_generation += 1;
        }
        self.render(Some(snapshot))
    }
    pub(crate) fn deadline(
        &mut self,
        deadline: crate::status_presenter::StatusDeadline,
    ) -> Vec<UiEffect> {
        self.status.deadline(deadline)
    }
    pub(crate) fn handle(
        &mut self,
        event: AppEvent,
        snapshot: Option<&Arc<AppSnapshot>>,
    ) -> Vec<UiEffect> {
        let mut completed_snapshot = None;
        let mut effects = match event {
            AppEvent::Input(input) => self.input(input, snapshot),
            AppEvent::SettingsShown(section) => {
                self.view.settings = "open";
                self.view.settings_focus = section;
                self.view.settings_generation += 1;
                self.view.menu_open = false;
                vec![]
            }
            AppEvent::SettingsClosed => {
                let open = self.view.settings != "closed";
                self.view.settings = "closed";
                self.view.settings_focus = None;
                if open {
                    self.focus();
                }
                vec![]
            }
            AppEvent::ComposerReady => {
                if self.pending_focus && self.view.settings == "closed" {
                    self.view.focus_request += 1;
                    self.pending_focus = false;
                }
                vec![]
            }
            AppEvent::FocusComposer => {
                self.focus();
                vec![]
            }
            AppEvent::ConversationSelected => {
                self.view.history_open = false;
                self.view.menu_open = false;
                vec![]
            }
            AppEvent::Loaded { generation, result } => {
                if generation != self.load_generation {
                    return vec![];
                }
                self.view.loading = false;
                match result {
                    Ok(mut content) => {
                        if let WindowContent::Main(main) = &mut content {
                            if let Some(latest) =
                                snapshot.filter(|s| s.revision > main.snapshot.revision)
                            {
                                Arc::make_mut(main).snapshot = latest.clone();
                            }
                        }
                        self.view.error = None;
                        let mut effects = vec![];
                        if let WindowContent::Main(main) = &content {
                            effects.push(root(UiEvent::ChatLoaded(main.snapshot.clone())));
                        }
                        effects.push(UiEffect::RenderWindow(content));
                        effects
                    }
                    Err(error) => {
                        self.view.error = Some(error);
                        vec![]
                    }
                }
            }
            AppEvent::Completed { token, result } => {
                let Some(operation) = self.operations.remove(&token) else {
                    return vec![];
                };
                let mut effects = vec![];
                match &result {
                    Ok(completed) => {
                        self.view.error = None;
                        if snapshot.is_none_or(|s| s.revision < completed.revision) {
                            completed_snapshot = Some(completed.clone());
                        }
                        effects.push(root(UiEvent::ChatLoaded(completed.clone())));
                    }
                    Err(error) => self.view.error = Some(error.clone()),
                }
                match operation {
                    AppOperation::Watch(_) | AppOperation::Audio(_) => {
                        let watch = matches!(operation, AppOperation::Watch(_));
                        let desired = {
                            let lane = if watch {
                                &mut self.watch
                            } else {
                                &mut self.audio
                            };
                            lane.running = false;
                            lane.running_target = None;
                            lane.desired.take()
                        };
                        if let Some(next) = desired {
                            match &result {
                                Ok(completed) => {
                                    let latest: &AppSnapshot = match snapshot {
                                        Some(current) if current.revision > completed.revision => {
                                            current.as_ref()
                                        }
                                        _ => completed.as_ref(),
                                    };
                                    match can_start_queued_toggle(watch, next, latest) {
                                        Ok(()) => effects.push(self.start_toggle(
                                            watch,
                                            next.target,
                                            next.base_config_revision,
                                        )),
                                        Err(reason) => effects.push(toggle_drop_log(
                                            watch,
                                            reason,
                                            Some(next),
                                            Some(latest),
                                        )),
                                    }
                                }
                                Err(_) => {
                                    effects.push(toggle_drop_log(
                                        watch,
                                        "operation-failed",
                                        Some(next),
                                        snapshot.map(|s| s.as_ref()),
                                    ));
                                }
                            }
                        }
                    }
                    AppOperation::Finish => self.view.finish_busy = false,
                    _ => {}
                }
                effects
            }
            AppEvent::SelectPersona { persona, reply } => {
                let Some(snapshot) = snapshot else {
                    let _ = reply.send(IpcResult::failure("画面の読込が完了していません"));
                    return vec![];
                };
                vec![spawn(AppTask::SelectPersona {
                    setup: snapshot.onboarding.setup_required,
                    persona,
                    reply,
                })]
            }
            AppEvent::PersonaSelected { result, reply } => {
                self.view.error = match result.as_ref() {
                    IpcResult::Failure { error, .. } => Some(error.message.clone()),
                    _ => None,
                };
                let _ = reply.send(*result);
                self.reload_personas()
            }
            AppEvent::PersonasChanged => self.reload_personas(),
            AppEvent::PersonasLoaded { generation, result } => {
                if generation != self.personas_generation {
                    return vec![];
                }
                match result {
                    Ok(personas) => vec![UiEffect::PersonasRender(personas)],
                    Err(error) => {
                        self.view.error = Some(error);
                        vec![]
                    }
                }
            }
        };
        let mut render = self.render(latest_snapshot(completed_snapshot.as_ref(), snapshot));
        render.append(&mut effects);
        render
    }
    fn input(&mut self, input: AppInput, snapshot: Option<&Arc<AppSnapshot>>) -> Vec<UiEffect> {
        use AppInput::*;
        match input {
            Report { error } => {
                self.view.error = error;
                vec![]
            }
            Mounted => {
                self.mounted = true;
                if self.view.loading {
                    vec![]
                } else {
                    self.load()
                }
            }
            Retry => {
                if self.view.loading {
                    vec![]
                } else {
                    self.load()
                }
            }
            StartupSettings => {
                let mut effects = if self.view.loading {
                    vec![]
                } else {
                    self.load()
                };
                effects.push(root(UiEvent::OpenSettings));
                effects
            }
            OpenSettings => {
                self.view.menu_open = false;
                vec![root(UiEvent::OpenSettings)]
            }
            CloseSettings => vec![root(UiEvent::Window {
                view: PresenterId::Settings,
                event: crate::presentation::PresentationEvent::Hide,
            })],
            ToggleHistory => {
                self.view.history_open = !self.view.history_open;
                self.view.menu_open = false;
                if !self.view.history_open {
                    self.focus();
                }
                vec![]
            }
            ResetOpen | MenuReset if self.view.can_reset => {
                self.view.menu_open = false;
                self.view.reset_confirm_open = true;
                vec![]
            }
            ResetCancel => {
                self.view.reset_confirm_open = false;
                self.focus();
                vec![]
            }
            ResetConfirm if self.view.can_reset && self.view.reset_confirm_open => {
                self.view.reset_confirm_open = false;
                self.focus();
                vec![self.operation(AppOperation::Reset, config_revision(snapshot))]
            }
            MenuToggle => {
                self.view.menu_open = !self.view.menu_open;
                vec![]
            }
            MenuDismiss => {
                self.view.menu_open = false;
                vec![]
            }
            MenuModel => {
                self.view.menu_open = false;
                vec![root(UiEvent::OpenSettingsAt("providers"))]
            }
            ToggleWatch => self.toggle(true, snapshot),
            ToggleAudio => self.toggle(false, snapshot),
            TutorialNext => match self.view.tutorial.as_ref().and_then(|t| t.next) {
                Some("retry") => {
                    vec![self.operation(AppOperation::RetryChat, config_revision(snapshot))]
                }
                Some("next") => vec![self.operation(AppOperation::Next, config_revision(snapshot))],
                _ => vec![],
            },
            TutorialFinish if self.view.tutorial.is_some() && !self.view.finish_busy => {
                self.view.finish_busy = true;
                vec![self.operation(AppOperation::Finish, config_revision(snapshot))]
            }
            SettingsPresented { generation }
                if self.view.settings != "closed"
                    && generation == self.view.settings_generation
                    && self.acked_generation != Some(generation)
                    && snapshot.is_some_and(|s| crate::tutorial_ui::settings_ack(s)) =>
            {
                self.acked_generation = Some(generation);
                vec![self.operation(AppOperation::SettingsAck, config_revision(snapshot))]
            }
            DismissBanner => self.status.dismiss_banner(),
            Recover => match self.status.recovery() {
                Some(RecoveryAction::Settings) => vec![root(UiEvent::OpenSettings)],
                Some(RecoveryAction::Relaunch) => vec![root(UiEvent::NativeShutdown(
                    crate::shutdown::ExitKind::Restart,
                ))],
                Some(action) => {
                    vec![self.operation(AppOperation::Recover(action), config_revision(snapshot))]
                }
                None => vec![],
            },
            _ => vec![],
        }
    }
    fn focus(&mut self) {
        self.pending_focus = true;
        self.view.history_open = false;
        if self.view.settings == "closed" {
            self.view.focus_request += 1;
        }
    }
    fn load(&mut self) -> Vec<UiEffect> {
        self.load_generation += 1;
        self.view.loading = true;
        self.view.error = None;
        vec![spawn(AppTask::Load(self.load_generation))]
    }
    fn reload_personas(&mut self) -> Vec<UiEffect> {
        self.personas_generation += 1;
        vec![spawn(AppTask::Personas(self.personas_generation))]
    }
    fn toggle(&mut self, watch: bool, snapshot: Option<&Arc<AppSnapshot>>) -> Vec<UiEffect> {
        let Some(snapshot) = snapshot else {
            return vec![toggle_drop_log(watch, "no-snapshot", None, None)];
        };
        let target = !confirmed_toggle_target(snapshot, watch);
        let base_config_revision = snapshot.config_revision;
        let lane = if watch {
            &mut self.watch
        } else {
            &mut self.audio
        };
        if lane.running {
            if lane.running_target == Some(target) {
                lane.desired = None;
                return vec![toggle_drop_log(
                    watch,
                    "same-target-running",
                    Some(QueuedToggle {
                        target,
                        base_config_revision,
                    }),
                    Some(snapshot),
                )];
            }
            lane.desired = Some(QueuedToggle {
                target,
                base_config_revision,
            });
            return vec![];
        }
        vec![self.start_toggle(watch, target, base_config_revision)]
    }
    fn start_toggle(&mut self, watch: bool, target: bool, config_revision: u64) -> UiEffect {
        let lane = if watch {
            &mut self.watch
        } else {
            &mut self.audio
        };
        lane.running = true;
        lane.running_target = Some(target);
        lane.desired = None;
        self.operation(
            if watch {
                AppOperation::Watch(target)
            } else {
                AppOperation::Audio(target)
            },
            config_revision,
        )
    }
    fn operation(&mut self, operation: AppOperation, config_revision: u64) -> UiEffect {
        self.next_token += 1;
        self.operations.insert(self.next_token, operation);
        spawn(AppTask::Operation {
            token: self.next_token,
            operation,
            config_revision,
        })
    }
    fn render(&mut self, snapshot: Option<&Arc<AppSnapshot>>) -> Vec<UiEffect> {
        if !self.mounted {
            return vec![];
        }
        self.view.watch_changing = self.watch.running;
        self.view.audio_changing = self.audio.running
            || snapshot.is_some_and(|snapshot| snapshot.audio.phase == "starting");
        let mut effects = vec![];
        if let Some(snapshot) = snapshot {
            let snapshot = snapshot.as_ref();
            self.view.screen = if snapshot.onboarding.finish_pending {
                "finish"
            } else if snapshot.onboarding.setup_required {
                "setup"
            } else {
                "chat"
            };
            self.view.can_reset = !snapshot.onboarding.tutorial_active;
            self.view.tutorial = crate::tutorial_ui::view(snapshot);
            if self.view.settings != "closed" {
                self.view.settings = if crate::tutorial_ui::settings_available(snapshot) {
                    "open"
                } else {
                    "locked"
                };
            }
            effects.extend(self.status.observe(
                snapshot,
                self.watch.running,
                self.view.error.as_deref(),
            ));
        } else {
            self.view.screen = if self.view.error.is_some() {
                "startupError"
            } else {
                "loading"
            };
        }
        if self.mounted {
            effects.insert(
                0,
                UiEffect::AppRender(Box::new(AppFrame {
                    view: self.view_copy(),
                    snapshot: snapshot.map(|snapshot| snapshot.as_ref().clone()),
                })),
            );
        }
        effects
    }
    fn view_copy(&self) -> AppView {
        AppView {
            screen: self.view.screen,
            loading: self.view.loading,
            error: self.view.error.clone(),
            finish_busy: self.view.finish_busy,
            settings: if self.view.screen == "finish" {
                "closed"
            } else {
                self.view.settings
            },
            settings_focus: self.view.settings_focus,
            settings_generation: self.view.settings_generation,
            focus_request: self.view.focus_request,
            history_open: self.view.history_open,
            reset_confirm_open: self.view.reset_confirm_open,
            menu_open: self.view.menu_open,
            can_reset: self.view.can_reset,
            watch_changing: self.view.watch_changing,
            audio_changing: self.view.audio_changing,
            tutorial: self.view.tutorial.clone(),
        }
    }
}
fn config_revision(snapshot: Option<&Arc<AppSnapshot>>) -> u64 {
    snapshot.map_or(0, |s| s.config_revision)
}
fn confirmed_toggle_target(snapshot: &AppSnapshot, watch: bool) -> bool {
    if watch {
        snapshot.watch_intent_active
    } else {
        snapshot.config.audio.enabled
    }
}
fn can_start_queued_toggle(
    watch: bool,
    request: QueuedToggle,
    snapshot: &AppSnapshot,
) -> Result<(), &'static str> {
    if snapshot.config_revision != request.base_config_revision {
        return Err("stale-config-revision");
    }
    if confirmed_toggle_target(snapshot, watch) == request.target {
        return Err("already-confirmed");
    }
    Ok(())
}
fn latest_snapshot<'a>(
    completed: Option<&'a Arc<AppSnapshot>>,
    current: Option<&'a Arc<AppSnapshot>>,
) -> Option<&'a Arc<AppSnapshot>> {
    completed.or(current)
}
fn toggle_drop_log(
    watch: bool,
    reason: &'static str,
    request: Option<QueuedToggle>,
    snapshot: Option<&AppSnapshot>,
) -> UiEffect {
    let lane = if watch { "watch" } else { "audio" };
    let request = request.map_or(String::new(), |request| {
        format!(
            " target={} base-config-revision={}",
            request.target, request.base_config_revision
        )
    });
    let current = snapshot.map_or(String::new(), |snapshot| {
        format!(" current-config-revision={}", snapshot.config_revision)
    });
    UiEffect::Log(format!(
        "ui: presenter=App event=toggle lane={lane} outcome=dropped reason={reason}{request}{current}"
    ))
}
fn spawn(task: AppTask) -> UiEffect {
    UiEffect::Spawn(UiTask::App(task))
}
fn root(event: UiEvent) -> UiEffect {
    UiEffect::Deliver {
        child: PresenterId::Root,
        event,
    }
}
fn ipc_value<T: Serialize>(result: IpcResult<T>) -> Result<T, String> {
    match result {
        IpcResult::Success { value, .. } => Ok(value),
        IpcResult::Failure { error, .. } => Err(error.message),
    }
}

pub(crate) async fn run(state: Arc<crate::state::DesktopState>, task: AppTask) -> UiEvent {
    let event = match task {
        AppTask::Load(generation) => AppEvent::Loaded {
            generation,
            result: crate::ui_load::refresh(&state, PresenterId::Chat).await,
        },
        AppTask::Personas(generation) => AppEvent::PersonasLoaded {
            generation,
            result: crate::factory::persona_options(&state.paths),
        },
        AppTask::SelectPersona {
            setup,
            persona,
            reply,
        } => AppEvent::PersonaSelected {
            result: Box::new(crate::commands::select_persona_for_view(state, persona, setup).await),
            reply,
        },
        AppTask::Operation {
            token,
            operation,
            config_revision,
        } => {
            let result = operation_result(&state, operation, config_revision).await;
            AppEvent::Completed { token, result }
        }
    };
    UiEvent::App(event)
}
async fn operation_result(
    state: &Arc<crate::state::DesktopState>,
    operation: AppOperation,
    config_revision: u64,
) -> Result<Arc<AppSnapshot>, String> {
    use crate::command_guard::CommandSource;
    match operation {
        AppOperation::Audio(enabled) => {
            let _ = state.logger.write("INFO", &format!("audio-toggle origin=ui-toggle desired-enabled={enabled} base-config-revision={config_revision}"));
            ipc_value(
                crate::commands::update_config_for_source(
                    state.clone(),
                    serde_json::json!({"audio":{"enabled":enabled}}),
                    None,
                    Some(config_revision),
                    CommandSource::IpcMain,
                )
                .await,
            )?;
        }
        AppOperation::RetryChat => {
            ipc_value(
                state
                    .ui
                    .query(UiView::Chat, |reply| {
                        UiEvent::UserCommand(UserCommand::ChatRetry(reply))
                    })
                    .await?,
            )?;
        }
        AppOperation::Recover(action) => {
            use coosenpai_core::ports::SystemSettingsPane;
            let pane = match action {
                RecoveryAction::Microphone => SystemSettingsPane::Microphone,
                RecoveryAction::Recognition => SystemSettingsPane::SpeechRecognition,
                RecoveryAction::ScreenCapture => SystemSettingsPane::ScreenCapture,
                RecoveryAction::SystemAudio => SystemSettingsPane::SystemAudio,
                _ => return Err("復旧操作が不正です".into()),
            };
            ipc_value(
                state
                    .ui
                    .query(UiView::Chat, |reply| {
                        UiEvent::UserCommand(UserCommand::SystemSettings {
                            pane,
                            failure: coosenpai_core::locale::TextKey::SystemSettingsOpenFailed,
                            reply,
                        })
                    })
                    .await?,
            )?;
        }
        operation => {
            let snapshot = ipc_value(
                state
                    .ui
                    .query(UiView::Chat, |reply| {
                        UiEvent::UserCommand(match operation {
                            AppOperation::Watch(true) => UserCommand::WatchStart {
                                source: CommandSource::IpcMain,
                                reply,
                            },
                            AppOperation::Watch(false) => UserCommand::WatchStop {
                                source: CommandSource::IpcMain,
                                reply,
                            },
                            AppOperation::Next => UserCommand::TutorialNext(reply),
                            AppOperation::Finish => UserCommand::TutorialFinish(reply),
                            AppOperation::SettingsAck => {
                                UserCommand::TutorialSettingsPresented(reply)
                            }
                            AppOperation::Reset => UserCommand::ConversationReset(reply),
                            _ => unreachable!(),
                        })
                    })
                    .await?,
            )?;
            return Ok(Arc::new(snapshot));
        }
    }
    Ok(Arc::new(state.snapshot().await))
}

