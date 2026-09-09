use crate::bubbles::BubbleAppearancePreview;
use crate::commands::IpcResult;
use crate::ui_events::{UiEffect, UiTask};
use std::collections::VecDeque;
use tokio::sync::oneshot;

type Reply = oneshot::Sender<IpcResult<()>>;

#[derive(Default)]
pub(crate) struct SettingsPreviewPresenter {
    active: Option<Reply>,
    waiting: VecDeque<(Option<BubbleAppearancePreview>, Reply)>,
}
impl SettingsPreviewPresenter {
    pub(crate) fn request(
        &mut self,
        preview: Option<BubbleAppearancePreview>,
        reply: Reply,
    ) -> Vec<UiEffect> {
        self.waiting.push_back((preview, reply));
        self.next()
    }
    pub(crate) fn completed(&mut self, result: IpcResult<()>) -> Vec<UiEffect> {
        if let Some(reply) = self.active.take() {
            let _ = reply.send(result);
        }
        self.next()
    }
    fn next(&mut self) -> Vec<UiEffect> {
        if self.active.is_some() {
            return vec![];
        }
        let Some((preview, reply)) = self.waiting.pop_front() else {
            return vec![];
        };
        self.active = Some(reply);
        vec![UiEffect::Spawn(UiTask::SettingsPreview(preview))]
    }
}

