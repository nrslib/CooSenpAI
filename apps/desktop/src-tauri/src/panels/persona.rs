use super::*;
use serde_json::json;

#[derive(Default)]
pub(super) struct PersonaPresenter {
    builtin: bool,
    original_id: String,
    draft: Draft,
    busy: bool,
    delete_confirm: bool,
    error: Option<String>,
    closed: bool,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Draft {
    id: String,
    display_name: String,
    body: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Initial {
    builtin: bool,
    original_id: String,
    draft: Draft,
}

impl PersonaPresenter {
    pub(super) fn busy(&self) -> bool {
        self.busy
    }

    pub(super) fn handle(&mut self, event: PanelEvent, io: &mut PanelIo) -> Result<Value, String> {
        match event {
            PanelEvent::Mount { value } => {
                let input: Initial = decode(value)?;
                self.builtin = input.builtin;
                self.original_id = input.original_id;
                self.draft = input.draft;
            }
            PanelEvent::Change { value } => self.draft = decode(value)?,
            PanelEvent::Action { name, value } if !self.closed => {
                if name == "save" && !value.is_null() {
                    self.draft = decode(value.clone())?;
                }
                match name.as_str() {
                    "key" => {
                        #[derive(Deserialize)]
                        #[serde(rename_all = "camelCase")]
                        struct Key {
                            key: String,
                            composing: bool,
                            key_code: u32,
                        }
                        let key: Key = decode(value)?;
                        if key.key == "Escape"
                            && !key.composing
                            && key.key_code != 229
                            && !self.busy
                        {
                            if self.delete_confirm {
                                self.delete_confirm = false;
                            } else {
                                self.closed = true;
                                io.command("close", ());
                            }
                        }
                    }
                    "close" if !self.busy => {
                        self.closed = true;
                        io.command("close", ());
                    }
                    "close" => {}
                    "cancelDelete" => self.delete_confirm = false,
                    "requestDelete" if !self.busy && !self.builtin => self.delete_confirm = true,
                    "save" if !self.busy && self.can_save() => {
                        self.busy = true;
                        io.command("save", json!({"id": self.draft.id, "displayName": self.draft.display_name, "body": self.draft.body}));
                    }
                    "delete" if !self.busy && self.delete_confirm && !self.builtin => {
                        self.delete_confirm = false;
                        self.busy = true;
                        io.command("delete", &self.original_id);
                    }
                    "restore" if !self.busy && value.as_str().is_some_and(|v| !v.is_empty()) => {
                        self.busy = true;
                        io.command("restore", json!({"id":self.original_id,"version":value}));
                    }
                    "requestDelete" | "save" | "delete" | "restore" => {}
                    _ => return Err(action_error(&name)),
                }
            }
            PanelEvent::Completed { id, result } => {
                if let Some(command) = io.completed(id) {
                    if matches!(command.kind.as_str(), "save" | "delete" | "restore")
                        && !self.closed
                    {
                        self.busy = false;
                        if result.ok {
                            self.busy = true;
                            io.command("refresh", ());
                        } else {
                            self.error = Some(result.message().into());
                        }
                    } else if command.kind == "refresh" && !self.closed {
                        self.busy = false;
                        if result.ok {
                            self.closed = true;
                            io.command("close", ());
                        } else {
                            self.error = Some(result.message().into());
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(
            json!({"busy":self.busy,"canSave":!self.busy && self.can_save(),"deleteConfirmOpen":self.delete_confirm,"error":self.error}),
        )
    }

    fn can_save(&self) -> bool {
        !self.draft.id.is_empty()
            && self.draft.id.len() <= 64
            && self
                .draft
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !self.draft.display_name.trim().is_empty()
            && !self.draft.body.trim().is_empty()
    }
}
