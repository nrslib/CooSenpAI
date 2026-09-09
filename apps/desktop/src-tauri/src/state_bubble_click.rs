use super::DesktopState;
use crate::bubble_click::BubbleClickHost;
use async_trait::async_trait;
use coosenpai_core::locale::Locale;

#[async_trait]
impl BubbleClickHost for DesktopState {
    fn locale(&self) -> Locale {
        Locale::from_config(&self.runtime_config().ui.language)
    }

    async fn input_view(
        &self,
        input: crate::bubble_controls_presenter::BubbleViewInput,
    ) -> Result<(), String> {
        let ui = self.ui.clone();
        ui.request(
            crate::ui_events::UiView::Bubble,
            crate::ui_events::UiEvent::BubbleView(
                crate::bubble_controls_presenter::BubbleViewEvent::Input(input),
            ),
        )
        .await
        .map(|_| ())
    }

    async fn input_click(&self, id: String, body: bool) -> Result<(), String> {
        self.ui
            .request(
                crate::ui_events::UiView::Bubble,
                crate::ui_events::UiEvent::BubbleClick { id, body },
            )
            .await
            .map(|_| ())
    }
}
