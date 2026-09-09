use super::*;
use serde_json::json;

#[derive(Default)]
pub(super) struct AttachmentPresenter {
    path: String,
    load: Option<u64>,
    source: Option<String>,
    expired: bool,
    expanded: bool,
}

impl AttachmentPresenter {
    pub(super) fn handle(&mut self, event: PanelEvent, io: &mut PanelIo) -> Result<Value, String> {
        match event {
            PanelEvent::Mount { value } | PanelEvent::Change { value } => {
                let path: String = decode(value)?;
                if self.load.is_none() && self.source.is_none() && !self.expired
                    || path != self.path
                {
                    self.path = path;
                    self.source = None;
                    self.expired = false;
                    self.expanded = false;
                    self.load = Some(io.command("read", &self.path));
                }
            }
            PanelEvent::Action { name, .. } => match name.as_str() {
                "expand" if self.source.is_some() => self.expanded = true,
                "expand" => {}
                "close" => self.expanded = false,
                _ => return Err(action_error(&name)),
            },
            PanelEvent::Completed { id, result }
                if io.completed(id).is_some() && self.load == Some(id) =>
            {
                self.load = None;
                self.expired = !result.ok;
                if result.ok {
                    self.source = Some(decode(result.value)?);
                }
            }
            _ => {}
        }
        Ok(json!({"source":self.source,"expired":self.expired,"expanded":self.expanded}))
    }
}

#[derive(Default)]
pub(super) struct HistoryPresenter {
    items: Vec<Value>,
    filter: Option<String>,
    expanded_text: Option<String>,
}
impl HistoryPresenter {
    pub(super) fn handle(&mut self, event: PanelEvent, _io: &mut PanelIo) -> Result<Value, String> {
        match event {
            PanelEvent::Mount { value } | PanelEvent::Change { value } => {
                self.items = decode(value)?
            }
            PanelEvent::Action { name, value } => match name.as_str() {
                "filter" => {
                    let filter: String = decode(value)?;
                    if !matches!(filter.as_str(), "all" | "image" | "text") {
                        return Err(action_error(&filter));
                    }
                    self.filter = Some(filter);
                }
                "expand" => self.expanded_text = Some(decode(value)?),
                "close" => self.expanded_text = None,
                _ => return Err(action_error(&name)),
            },
            _ => {}
        }
        let filter = self.filter.as_deref().unwrap_or("all");
        let visible: Vec<_> = self
            .items
            .iter()
            .filter(|item| filter == "all" || item["kind"] == filter)
            .collect();
        Ok(
            json!({"filter":filter,"expandedText":self.expanded_text,"visible":visible,"empty":self.items.is_empty()}),
        )
    }
}
