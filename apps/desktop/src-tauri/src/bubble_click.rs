use async_trait::async_trait;
use coosenpai_core::locale::Locale;
use std::sync::Arc;

#[async_trait]
pub(crate) trait BubbleClickHost: Send + Sync {
    fn locale(&self) -> Locale;
    async fn input_view(
        &self,
        input: crate::bubble_controls_presenter::BubbleViewInput,
    ) -> Result<(), String>;
    async fn input_click(&self, id: String, body: bool) -> Result<(), String>;
}

pub(crate) struct BubbleClickState(pub(crate) Arc<dyn BubbleClickHost>);
