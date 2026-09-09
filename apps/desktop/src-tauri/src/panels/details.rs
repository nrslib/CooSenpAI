use super::*;
use serde_json::json;

#[path = "dataflow.rs"]
mod dataflow;

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
}
impl DetailsPresenter {
    pub(super) fn handle(&mut self, event: PanelEvent, io: &mut PanelIo) -> Result<Value, String> {
        match event {
            PanelEvent::Mount { .. } => {
                io.command("ready", ());
            }
            PanelEvent::Action { name, value } => match name.as_str() {
                "snapshot" => self.observe(value)?,
                "history" => self.flow.history(decode(value)?)?,
                "tab" => {
                    let tab: String = decode(value)?;
                    if !["state", "emotions", "conversation", "dataflow"].contains(&tab.as_str()) {
                        return Err(action_error(&tab));
                    }
                    self.tab = Some(tab);
                }
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
                    if command.kind == "resetEmotions" {
                        if self.resetting != Some(id) {
                            return self.view();
                        }
                        self.resetting = None;
                        if result.ok {
                            self.observe(result.value)?;
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
                        self.observe(result.value)?;
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
        if let Some(id) = self.resetting.take() {
            io.completed(id);
        }
    }
    fn observe(&mut self, snapshot: Value) -> Result<(), String> {
        let revision = snapshot["revision"]
            .as_u64()
            .ok_or("snapshot の revision がありません")?;
        if self.snapshot.as_ref().is_some_and(|previous| {
            previous["revision"].as_u64().expect("validated revision") >= revision
        }) {
            return Ok(());
        }
        self.flow.snapshot(snapshot.clone())?;
        self.snapshot = Some(snapshot);
        Ok(())
    }
    fn view(&self) -> Result<Value, String> {
        Ok(
            json!({"snapshot":self.snapshot,"activeTab":self.tab.as_deref().unwrap_or("state"),"debugDetail":self.debug,
            "resetting":self.resetting.is_some(),"resetError":self.reset_error,
            "switching":self.switching.is_some(),"error":self.error,"dataFlow":self.flow.view()}),
        )
    }
}

#[derive(Default)]
pub(super) struct DataFlowPresenter {
    events: Vec<Value>,
    filter: Option<String>,
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
                    if !["all", "visual", "hearing", "companion"].contains(&filter.as_str()) {
                        return Err(action_error(&filter));
                    }
                    self.filter = Some(filter);
                }
                "query" => self.query = decode(value)?,
                "scroll" => self.scroll_distance = decode(value)?,
                "toggle" => {
                    let id: String = decode(value)?;
                    if !self.expanded.remove(&id) {
                        self.expanded.insert(id);
                    }
                }
                _ => return Err(action_error(&name)),
            },
            PanelEvent::Completed { id, .. } => {
                io.completed(id);
            }
            _ => {}
        }
        let filter = self.filter.as_deref().unwrap_or("all");
        let needle = self.query.trim().to_lowercase();
        let visible: Vec<_> = self
            .events
            .iter()
            .filter(|event| {
                (filter == "all" || event["kind"] == filter)
                    && (needle.is_empty()
                        || ["summary", "detail"].iter().any(|field| {
                            event[field]
                                .as_str()
                                .is_some_and(|text| text.to_lowercase().contains(&needle))
                        }))
            })
            .collect();
        Ok(json!({"filter":filter,"visible":visible,"expanded":self.expanded}))
    }
}
