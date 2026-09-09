use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ui_events::PresenterId;

mod attachment;
mod details;
mod persona;
mod settings;
mod status;
mod vrm;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PanelKind {
    Settings,
    Persona,
    Vrm,
    Update,
    Voice,
    Details,
    Dataflow,
    Attachment,
    History,
}

impl PanelKind {
    pub(crate) fn owner(self, details_window: bool) -> PresenterId {
        match self {
            Self::Settings | Self::Persona | Self::Vrm => PresenterId::Settings,
            Self::Details | Self::Dataflow => PresenterId::Details,
            Self::Attachment if details_window => PresenterId::Details,
            _ => PresenterId::Chat,
        }
    }

    pub(crate) fn allowed(self, window: &str) -> bool {
        match window {
            "main" => !matches!(self, Self::Details | Self::Dataflow),
            "details" => matches!(self, Self::Details | Self::Dataflow | Self::Attachment),
            _ => false,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct PanelRequest {
    pub session: String,
    pub kind: PanelKind,
    pub event: PanelEvent,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum PanelEvent {
    Mount { value: Value },
    Change { value: Value },
    Action { name: String, value: Value },
    Completed { id: u64, result: IoResult },
    Unmount,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct IoResult {
    pub ok: bool,
    #[serde(default)]
    pub value: Value,
    pub error: Option<IoError>,
    pub issues: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct IoError {
    pub message: String,
    pub issues: Option<Value>,
}

impl IoResult {
    fn validate(&self) -> Result<(), String> {
        if !self.ok && self.error.is_none() {
            return Err("失敗結果にエラーがありません".into());
        }
        Ok(())
    }

    fn message(&self) -> &str {
        self.error
            .as_ref()
            .expect("failed I/O has an error")
            .message
            .as_str()
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct PanelOutput {
    pub revision: u64,
    pub state: Value,
    pub commands: Vec<PanelCommand>,
    pub updates: Vec<PanelUpdate>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PanelUpdate {
    pub session: String,
    pub revision: u64,
    pub state: Value,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PanelCommand {
    pub id: u64,
    pub kind: String,
    pub payload: Value,
}

#[derive(Default)]
struct PanelIo {
    next: u64,
    pending: HashMap<u64, PanelCommand>,
    commands: Vec<PanelCommand>,
}

impl PanelIo {
    fn command(&mut self, kind: &str, payload: impl Serialize) -> u64 {
        self.next += 1;
        let command = PanelCommand {
            id: self.next,
            kind: kind.into(),
            payload: serde_json::to_value(payload).expect("serializable panel command"),
        };
        self.pending.insert(command.id, command.clone());
        self.commands.push(command);
        self.next
    }

    fn completed(&mut self, id: u64) -> Option<PanelCommand> {
        self.pending.remove(&id)
    }
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("パネル入力が不正です: {error}"))
}

fn action_error(name: &str) -> String {
    format!("未登録のパネル操作です: {name}")
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct ChildActivity {
    persona: bool,
    vrm: bool,
}
impl ChildActivity {
    fn busy(self) -> bool {
        self.persona || self.vrm
    }
}

enum Panel {
    Settings(Box<settings::SettingsPresenter>),
    Persona(persona::PersonaPresenter),
    Vrm(vrm::VrmPresenter),
    Status(status::StatusPresenter),
    Details(Box<details::DetailsPresenter>),
    Dataflow(details::DataFlowPresenter),
    Attachment(attachment::AttachmentPresenter),
    History(attachment::HistoryPresenter),
}

impl Panel {
    fn new(kind: PanelKind) -> Self {
        match kind {
            PanelKind::Settings => Self::Settings(Default::default()),
            PanelKind::Persona => Self::Persona(Default::default()),
            PanelKind::Vrm => Self::Vrm(Default::default()),
            PanelKind::Update | PanelKind::Voice => {
                Self::Status(status::StatusPresenter::new(kind))
            }
            PanelKind::Details => Self::Details(Default::default()),
            PanelKind::Dataflow => Self::Dataflow(Default::default()),
            PanelKind::Attachment => Self::Attachment(Default::default()),
            PanelKind::History => Self::History(Default::default()),
        }
    }

    fn handle(
        &mut self,
        event: PanelEvent,
        io: &mut PanelIo,
        activity: ChildActivity,
    ) -> Result<Value, String> {
        match self {
            Self::Settings(p) => p.handle(event, io, activity),
            Self::Persona(p) => p.handle(event, io),
            Self::Vrm(p) => p.handle(event, io),
            Self::Status(p) => p.handle(event, io),
            Self::Details(p) => p.handle(event, io),
            Self::Dataflow(p) => p.handle(event, io),
            Self::Attachment(p) => p.handle(event, io),
            Self::History(p) => p.handle(event, io),
        }
    }
}

struct Session {
    kind: PanelKind,
    panel: Panel,
    io: PanelIo,
    revision: u64,
}

#[derive(Default)]
pub(crate) struct PanelPresenters {
    settings_hidden: bool,
    sessions: BTreeMap<String, Session>,
}

impl PanelPresenters {
    fn child_activity(&self) -> ChildActivity {
        ChildActivity {
            persona: self
                .sessions
                .values()
                .any(|s| matches!(&s.panel, Panel::Persona(p) if p.busy())),
            vrm: self
                .sessions
                .values()
                .any(|s| matches!(&s.panel, Panel::Vrm(p) if p.busy())),
        }
    }

    fn refresh_parents(&mut self, previous: ChildActivity) -> Vec<PanelUpdate> {
        let activity = self.child_activity();
        if activity == previous {
            return Vec::new();
        }
        self.sessions
            .iter_mut()
            .filter_map(|(session, current)| {
                if let Panel::Settings(parent) = &mut current.panel {
                    current.revision += 1;
                    Some(PanelUpdate {
                        session: session.clone(),
                        revision: current.revision,
                        state: parent.refresh_children(activity),
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    pub(crate) fn details_hidden(&mut self) {
        for session in self.sessions.values_mut() {
            if let Panel::Details(panel) = &mut session.panel {
                panel.hidden(&mut session.io);
            }
        }
    }

    pub(crate) fn settings_hidden(&mut self) {
        self.settings_hidden = true;
    }

    pub(crate) fn blocks_settings_close(&self) -> bool {
        self.child_activity().busy()
            || self
                .sessions
                .values()
                .any(|s| matches!(&s.panel, Panel::Settings(p) if p.blocks_close()))
    }

    pub(crate) fn handle(&mut self, request: PanelRequest) -> Result<PanelOutput, String> {
        let PanelRequest {
            session,
            kind,
            event,
        } = request;
        if let PanelEvent::Completed { result, .. } = &event {
            result.validate()?;
        }
        let activity = self.child_activity();
        if matches!(event, PanelEvent::Unmount) {
            self.sessions.remove(&session);
            return Ok(PanelOutput {
                revision: 0,
                state: Value::Null,
                commands: vec![],
                updates: self.refresh_parents(activity),
            });
        }
        if kind == PanelKind::Settings && matches!(event, PanelEvent::Mount { .. }) {
            self.settings_hidden = false;
        }
        if matches!(event, PanelEvent::Mount { .. }) {
            if self.sessions.contains_key(&session) {
                return Err("パネルは登録済みです".into());
            }
            self.sessions.insert(
                session.clone(),
                Session {
                    kind,
                    panel: Panel::new(kind),
                    io: PanelIo::default(),
                    revision: 0,
                },
            );
        }
        if matches!(kind, PanelKind::Persona | PanelKind::Vrm)
            && matches!(&event, PanelEvent::Action { name, .. } if matches!(name.as_str(), "save" | "delete" | "restore" | "select" | "remove" | "quality" | "toggle"))
            && (self.settings_hidden
                || self
                    .sessions
                    .values()
                    .any(|s| matches!(&s.panel, Panel::Settings(p) if !p.allows_child(kind))))
        {
            return Err("閉鎖した設定の子画面では新しい操作を開始できません".into());
        }
        let Some(current) = self.sessions.get_mut(&session) else {
            return Ok(PanelOutput {
                revision: 0,
                state: Value::Null,
                commands: vec![],
                updates: self.refresh_parents(activity),
            });
        };
        if current.kind != kind {
            return Err("パネルの種類が一致しません".into());
        }
        let state = current.panel.handle(event, &mut current.io, activity)?;
        current.revision += 1;
        let revision = current.revision;
        let commands = std::mem::take(&mut current.io.commands);
        Ok(PanelOutput {
            revision,
            state,
            commands,
            updates: self.refresh_parents(activity),
        })
    }
}

