use super::*;
use serde_json::json;

#[path = "conversation_log.rs"]
mod conversation_log;
#[path = "dataflow.rs"]
mod dataflow;

pub(super) fn snapshot_revision(snapshot: &Value) -> Result<u64, String> {
    snapshot["revision"]
        .as_u64()
        .ok_or("snapshot の revision がありません".into())
}

#[derive(Default)]
pub(super) struct DetailsPresenter {
    snapshot: Option<Value>,
    tab: Option<String>,
    debug: Option<Value>,
    switching: Option<u64>,
    resetting: Option<u64>,
    reset_error: Option<String>,
    error: Option<String>,
    flow: dataflow::DataFlow,
    log: conversation_log::ConversationLog,
}
impl DetailsPresenter {
    pub(super) fn handle(&mut self, event: PanelEvent, io: &mut PanelIo) -> Result<Value, String> {
        match event {
            PanelEvent::Mount { .. } => {
                io.command("ready", ());
            }
            PanelEvent::Action { name, value } => match name.as_str() {
                "history" => self.flow.history(decode(value)?)?,
                "tab" => {
                    let tab: String = decode(value)?;
                    if !["state", "emotions", "conversation", "dataflow", "log"]
                        .contains(&tab.as_str())
                    {
                        return Err(action_error(&tab));
                    }
                    if tab == "log" {
                        if self.tab.as_deref() != Some("log") {
                            self.log.activate(io);
                        }
                    } else {
                        self.log.hidden();
                    }
                    self.tab = Some(tab);
                }
                "logDate" => self.log.select_date(decode(value)?, io)?,
                "logDeleteRequest" => self.log.request_delete()?,
                "logDeleteCancel" => self.log.cancel_delete(),
                "logDeleteConfirm" => self.log.confirm_delete(io)?,
                "logFilter" => self.log.set_filter(decode(value)?)?,
                "logScroll" => self.log.set_scroll_position(decode(value)?),
                "logSpeakerSelect" => self.log.select_speaker(decode(value)?)?,
                "logEntrySelect" => self.log.select_entry(decode(value)?)?,
                "logSpeakerName" => self.log.set_speaker_name(decode(value)?),
                "logSpeakerSave" => self.log.save_speaker(io)?,
                "logSpeakerClear" => self.log.clear_speaker(io)?,
                "logSpeakerClose" => self.log.close_speaker()?,
                "debug" => self.debug = decode(value)?,
                "resetEmotions" => {
                    if self.resetting.is_none() && self.snapshot.is_some() {
                        self.reset_error = None;
                        self.resetting = Some(io.command("resetEmotions", ()));
                    }
                }
                "select" => {
                    let generation: u64 = decode(value)?;
                    if self.switching.is_none()
                        && self.snapshot.as_ref().is_some_and(|snapshot| {
                            snapshot["selectedConversationGeneration"] != generation
                                && snapshot["conversationGenerations"].as_array().is_some_and(
                                    |items| {
                                        items.iter().any(|item| item["generation"] == generation)
                                    },
                                )
                        })
                    {
                        self.error = None;
                        self.switching = Some(io.command("select", generation));
                    }
                }
                _ => return Err(action_error(&name)),
            },
            PanelEvent::Completed { id, result } => {
                if let Some(command) = io.completed(id) {
                    if command.kind == "loadLog" {
                        self.log.loaded(id, result, io)?;
                        return self.view();
                    }
                    if command.kind == "speakerRename" {
                        self.log.speaker_rename_completed(id, result, io);
                        return self.view();
                    }
                    if command.kind == "deleteLogDay" {
                        self.log.delete_completed(id, result, io);
                        return self.view();
                    }
                    if command.kind == "resetEmotions" {
                        if self.resetting != Some(id) {
                            return self.view();
                        }
                        self.resetting = None;
                        if result.ok {
                            self.observe_snapshot(result.value, io)?;
                        } else {
                            self.reset_error = Some(result.message().into());
                        }
                        return self.view();
                    }
                    if command.kind == "select" && self.switching != Some(id) {
                        return self.view();
                    }
                    if command.kind == "select" {
                        self.switching = None;
                    }
                    if result.ok && command.kind == "select" {
                        self.observe_snapshot(result.value, io)?;
                    } else if !result.ok {
                        self.error = Some(result.message().into());
                    }
                }
            }
            _ => {}
        }
        self.view()
    }
    pub(super) fn hidden(&mut self, io: &mut PanelIo) {
        self.log.hidden();
        if let Some(id) = self.resetting.take() {
            io.completed(id);
        }
    }
    pub(super) fn observe_snapshot(
        &mut self,
        snapshot: Value,
        io: &mut PanelIo,
    ) -> Result<bool, String> {
        let revision = snapshot_revision(&snapshot)?;
        if self.snapshot.as_ref().is_some_and(|previous| {
            snapshot_revision(previous).expect("validated revision") >= revision
        }) {
            return Ok(false);
        }
        self.flow.snapshot(snapshot.clone())?;
        if self.snapshot.as_ref().is_some_and(|previous| {
            previous["speakerDirectoryRevision"] != snapshot["speakerDirectoryRevision"]
        }) {
            self.log.invalidate();
        }
        self.snapshot = Some(snapshot);
        // 会話ログタブの表示中だけ、保存された新しい発話を読み直す
        if self.tab.as_deref() == Some("log") {
            self.log.refresh(io);
        }
        Ok(true)
    }
    pub(super) fn view(&self) -> Result<Value, String> {
        Ok(
            json!({"snapshot":self.snapshot,"activeTab":self.tab.as_deref().unwrap_or("state"),"debugDetail":self.debug,
            "resetting":self.resetting.is_some(),"resetError":self.reset_error,
            "switching":self.switching.is_some(),"error":self.error,"dataFlow":self.flow.view(),
            "conversationLog":self.log.view()}),
        )
    }
}

#[derive(Default)]
pub(super) struct DataFlowPresenter {
    events: Vec<Value>,
    filters: std::collections::BTreeSet<String>,
    query: String,
    expanded: std::collections::BTreeSet<String>,
    scroll_distance: f64,
}
impl DataFlowPresenter {
    pub(super) fn handle(&mut self, event: PanelEvent, io: &mut PanelIo) -> Result<Value, String> {
        match event {
            PanelEvent::Mount { value } | PanelEvent::Change { value } => {
                self.events = decode(value)?;
                self.expanded
                    .retain(|id| self.events.iter().any(|event| event["id"] == *id));
                if self.scroll_distance < 24.0 {
                    io.command("scrollEnd", ());
                }
            }
            PanelEvent::Action { name, value } => match name.as_str() {
                "filter" => {
                    let filter: String = decode(value)?;
                    if !["all", "vision", "hearing", "coo"].contains(&filter.as_str()) {
                        return Err(action_error(&filter));
                    }
                    if filter == "all" {
                        self.filters.clear();
                    } else if !self.filters.insert(filter.clone()) {
                        self.filters.remove(&filter);
                    }
                }
                "query" => self.query = decode(value)?,
                "scroll" => self.scroll_distance = decode(value)?,
                "toggle" => {
                    let id: String = decode(value)?;
                    if !self.expanded.remove(&id) {
                        self.expanded.insert(id);
                    }
                }
                "open" => {
                    let path: String = decode(value)?;
                    if !self.events.iter().any(|event| {
                        event["references"].as_array().is_some_and(|references| {
                            references.iter().any(|reference| {
                                reference["path"] == path && reference["available"] == true
                            })
                        })
                    }) {
                        return Err("データフローの参照元を開けません".to_owned());
                    }
                    io.command("openPath", path);
                }
                _ => return Err(action_error(&name)),
            },
            PanelEvent::Completed { id, .. } => {
                io.completed(id);
            }
            _ => {}
        }
        let needle = self.query.trim().to_lowercase();
        let visible: Vec<_> = self
            .events
            .iter()
            .filter(|event| {
                (self.filters.is_empty()
                    || event["subject"]
                        .as_str()
                        .is_some_and(|subject| self.filters.contains(subject)))
                    && (needle.is_empty()
                        || ["summary", "detail"].iter().any(|field| {
                            event[field]
                                .as_str()
                                .is_some_and(|text| text.to_lowercase().contains(&needle))
                        }))
            })
            .collect();
        Ok(json!({"filters":self.filters,"visible":visible,"expanded":self.expanded}))
    }
}
