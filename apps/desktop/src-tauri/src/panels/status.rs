use super::*;
use serde_json::json;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Options {
    enabled: bool,
    show_controls: bool,
}

pub(super) struct StatusPresenter {
    kind: PanelKind,
    enabled: bool,
    show_controls: bool,
    snapshot: Option<Value>,
    error: Option<String>,
    deferred: bool,
    latest_action: Option<u64>,
    pending: Vec<u64>,
}

impl StatusPresenter {
    pub(super) fn new(kind: PanelKind) -> Self {
        Self {
            kind,
            enabled: false,
            show_controls: false,
            snapshot: None,
            error: None,
            deferred: false,
            latest_action: None,
            pending: vec![],
        }
    }

    pub(super) fn handle(&mut self, event: PanelEvent, io: &mut PanelIo) -> Result<Value, String> {
        match event {
            PanelEvent::Mount { value } => {
                self.options(value)?;
                let load = io.command("load", ());
                self.pending.push(load);
            }
            PanelEvent::Change { value } => self.options(value)?,
            PanelEvent::Action { name, value } => match name.as_str() {
                "observed" => self.observe(value)?,
                "later"
                    if self.kind == PanelKind::Update
                        && self.phase() == Some("installed")
                        && self.pending.is_empty() =>
                {
                    self.deferred = true
                }
                "later" => {}
                "check" | "install" | "restart" | "test" | "stop" => {
                    if self.allowed(&name) {
                        self.error = None;
                        let id = io.command(&name, ());
                        self.latest_action = Some(id);
                        self.pending.push(id);
                    }
                }
                _ => return Err(action_error(&name)),
            },
            PanelEvent::Completed { id, result } => {
                if let Some(command) = io.completed(id) {
                    self.pending.retain(|pending| *pending != id);
                    if result.ok {
                        if !result.value.is_null() {
                            self.observe(result.value)?;
                        }
                    } else if (command.kind == "load"
                        && self.latest_action.is_none()
                        && self.snapshot.is_none())
                        || self.latest_action == Some(id)
                    {
                        self.error = Some(result.message().into());
                    }
                }
            }
            _ => {}
        }
        let phase = self.phase();
        let busy = self.working();
        let visible = if self.kind == PanelKind::Update {
            !(!self.show_controls
                && (self.deferred && phase == Some("installed")
                    || self.error.is_none()
                        && matches!(phase, None | Some("idle" | "checking" | "upToDate"))
                    || !self.enabled
                        && !matches!(phase, Some("installed" | "installing" | "downloading"))))
        } else {
            self.show_controls
                || self.speaking()
                || self.error.is_some()
                || self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s["message"].as_str().is_some_and(|m| !m.is_empty()))
        };
        Ok(
            json!({"snapshot":self.snapshot,"error":self.error,"busy":busy,"visible":visible,
            "canCheck":self.allowed("check"),"canInstall":self.allowed("install"),"canRestart":self.allowed("restart"),
            "canTest":self.allowed("test"),"canStop":self.allowed("stop"),
            "showCheck":(self.show_controls || phase == Some("failed")) && phase != Some("installed"),
            "showInstall":phase == Some("available"),"showRestart":phase == Some("installed"),
            "showLater":!self.show_controls && phase == Some("installed")}),
        )
    }

    fn options(&mut self, value: Value) -> Result<(), String> {
        let options: Options = decode(value)?;
        self.enabled = options.enabled;
        self.show_controls = options.show_controls;
        Ok(())
    }

    fn observe(&mut self, value: Value) -> Result<(), String> {
        let revision = value["revision"]
            .as_u64()
            .ok_or("状態に revision がありません")?;
        if self
            .snapshot
            .as_ref()
            .is_some_and(|s| s["revision"].as_u64().expect("validated revision") >= revision)
        {
            return Ok(());
        }
        if self.kind == PanelKind::Voice && !value["speaking"].is_boolean()
            || self.kind == PanelKind::Update && !value["status"]["phase"].is_string()
        {
            return Err("操作状態が不正です".into());
        }
        self.snapshot = Some(value);
        Ok(())
    }
    fn phase(&self) -> Option<&str> {
        self.snapshot
            .as_ref()
            .and_then(|s| s["status"]["phase"].as_str())
    }
    fn speaking(&self) -> bool {
        self.snapshot
            .as_ref()
            .is_some_and(|s| s["speaking"] == true)
    }
    fn working(&self) -> bool {
        !self.pending.is_empty()
            || matches!(
                self.phase(),
                Some("checking" | "downloading" | "installing")
            )
    }
    fn allowed(&self, name: &str) -> bool {
        match (self.kind, name) {
            (PanelKind::Update, "check") => {
                self.enabled && !self.working() && self.phase() != Some("installed")
            }
            (PanelKind::Update, "install") => {
                self.enabled && !self.working() && self.phase() == Some("available")
            }
            (PanelKind::Update, "restart") => !self.working() && self.phase() == Some("installed"),
            (PanelKind::Voice, "test") => {
                self.enabled
                    && self.snapshot.is_some()
                    && !self.speaking()
                    && self.pending.is_empty()
            }
            (PanelKind::Voice, "stop") => true,
            _ => false,
        }
    }
}
