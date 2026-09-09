use super::*;
use serde_json::json;

#[derive(Default)]
pub(super) struct VrmPresenter {
    phase: Option<&'static str>,
    current: Option<u64>,
    operation: Option<&'static str>,
    model_name: Option<String>,
    candidate_name: Option<String>,
    quality: Option<String>,
    error: Option<String>,
    cancelled: bool,
}

impl VrmPresenter {
    pub(super) fn busy(&self) -> bool {
        self.phase.is_some()
    }

    pub(super) fn handle(&mut self, event: PanelEvent, io: &mut PanelIo) -> Result<Value, String> {
        match event {
            PanelEvent::Mount { .. } => {
                self.quality = Some("medium".into());
                self.start(io, "load", (), "loading", "load");
            }
            PanelEvent::Action { name, value } => match name.as_str() {
                "cancel" if self.phase == Some("loading") => {
                    self.cancelled = true;
                    io.command("abort", ());
                }
                "cancel" => {}
                "select" if self.phase.is_none() => {
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase")]
                    struct File {
                        file_id: String,
                        name: String,
                    }
                    let file: File = decode(value)?;
                    self.candidate_name = Some(file.name);
                    self.start(
                        io,
                        "verify",
                        json!({"fileId":file.file_id,"quality":self.quality}),
                        "loading",
                        "select",
                    );
                }
                "remove" if self.phase.is_none() => {
                    self.start(io, "remove", (), "saving", "remove")
                }
                "quality" if self.phase.is_none() => {
                    let quality: String = decode(value)?;
                    if !matches!(quality.as_str(), "low" | "medium" | "high") {
                        return Err("VRM の画質が不正です".into());
                    }
                    self.start(io, "saveQuality", quality, "saving", "quality");
                }
                "toggle" if self.phase.is_none() => {
                    self.start(io, "toggle", (), "saving", "toggle")
                }
                "select" | "remove" | "quality" | "toggle" => {}
                _ => return Err(action_error(&name)),
            },
            PanelEvent::Completed { id, result } => {
                if let Some(command) = io.completed(id) {
                    if self.current == Some(id) {
                        if self.cancelled {
                            self.finish(io);
                        } else if !result.ok {
                            self.error = Some(result.message().into());
                            self.finish(io);
                        } else {
                            self.advance(&command, result.value, io)?;
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(
            json!({"phase":self.phase,"busy":self.phase.is_some(),"modelName":self.model_name,
            "imageQuality":self.quality,"error":self.error,"aborted":self.cancelled && self.phase.is_none()}),
        )
    }
    fn start(
        &mut self,
        io: &mut PanelIo,
        command: &str,
        payload: impl Serialize,
        phase: &'static str,
        operation: &'static str,
    ) {
        self.error = None;
        self.cancelled = false;
        self.phase = Some(phase);
        self.operation = Some(operation);
        self.current = Some(io.command(command, payload));
    }
    fn finish(&mut self, io: &mut PanelIo) {
        if self.operation == Some("select") {
            io.command("release", ());
        }
        self.phase = None;
        self.operation = None;
        self.current = None;
    }
    fn advance(
        &mut self,
        command: &PanelCommand,
        value: Value,
        io: &mut PanelIo,
    ) -> Result<(), String> {
        match command.kind.as_str() {
            "load" => {
                self.model_name = decode(value["name"].clone())?;
                self.quality = Some(decode(value["quality"].clone())?);
                self.finish(io);
            }
            "verify" => {
                self.phase = Some("saving");
                self.current = Some(io.command("save", ()));
            }
            "save" => {
                self.model_name = self.candidate_name.take();
                self.current = Some(io.command("notify", "select"));
            }
            "remove" => {
                self.model_name = None;
                self.current = Some(io.command("notify", "remove"));
            }
            "saveQuality" => {
                self.quality = Some(decode(command.payload.clone())?);
                self.current = Some(io.command("notify", "quality"));
            }
            "notify" => match self.operation {
                Some("select") => self.current = Some(io.command("show", ())),
                Some("remove") => self.current = Some(io.command("hide", ())),
                _ => self.finish(io),
            },
            "show" | "hide" | "toggle" => self.finish(io),
            _ => return Err(action_error(&command.kind)),
        }
        Ok(())
    }
}
