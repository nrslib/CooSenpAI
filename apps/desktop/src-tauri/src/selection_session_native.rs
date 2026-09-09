use coosenpai_core::ports::{ClipboardReader, RuntimeLogger, SelectedTextCopyPort};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub(super) struct NativeSelectionPort {
    pub(super) logger: Arc<dyn RuntimeLogger>,
    pub(super) copier: Arc<dyn SelectedTextCopyPort>,
    pub(super) reader: Arc<dyn ClipboardReader>,
}

#[async_trait::async_trait]
impl crate::selection_session::SelectionSessionPort for NativeSelectionPort {
    async fn prepare_image(&self) -> Result<crate::selection_session::ImageSetup, String> {
        use crate::platform::region_screenshot::ScreenshotSystem;
        let system = crate::platform::region_screenshot::MacScreenshotSystem {
            logger: self.logger.as_ref(),
        };
        let shortcut = system.shortcut()?;
        if !system.access_allowed() {
            system.request_access();
            return Err(crate::platform::SCREENSHOT_ACCESS_REQUIRED.into());
        }
        let count = system.change_count();
        let contents = system.snapshot();
        if count != system.change_count() {
            return Err("キー送出前にクリップボードが変更されました".into());
        }
        Ok(crate::selection_session::ImageSetup {
            shortcut,
            count,
            contents,
            windows: crate::platform::screenshot_selection_windows()
                .map_err(|error| error.to_string())?
                .onscreen_window_ids,
        })
    }
    async fn post_image(
        &self,
        shortcut: crate::platform::region_screenshot::ScreenshotShortcut,
    ) -> Result<(), String> {
        use crate::platform::region_screenshot::ScreenshotSystem;
        crate::platform::region_screenshot::MacScreenshotSystem {
            logger: self.logger.as_ref(),
        }
        .post(&shortcut)
    }
    async fn escape(&self) -> Result<(), String> {
        use crate::platform::region_screenshot::ScreenshotSystem;
        crate::platform::region_screenshot::MacScreenshotSystem {
            logger: self.logger.as_ref(),
        }
        .escape(0)
    }
    async fn observe(&self) -> Result<crate::selection_session::SessionObservation, String> {
        use crate::platform::region_screenshot::ScreenshotSystem;
        let windows =
            crate::platform::screenshot_selection_windows().map_err(|error| error.to_string())?;
        let change_count = crate::platform::region_screenshot::MacScreenshotSystem {
            logger: self.logger.as_ref(),
        }
        .change_count();
        Ok(crate::selection_session::SessionObservation {
            onscreen_window_ids: windows.onscreen_window_ids,
            windows: windows.windows,
            candidates: windows.candidates,
            change_count,
        })
    }
    async fn image(&self) -> Result<Option<Vec<u8>>, String> {
        use crate::platform::region_screenshot::ScreenshotSystem;
        crate::platform::region_screenshot::MacScreenshotSystem {
            logger: self.logger.as_ref(),
        }
        .image()
    }
    async fn restore(
        &self,
        contents: crate::platform::region_screenshot::ClipboardContents,
        expected_count: i64,
    ) -> Result<(), String> {
        use crate::platform::region_screenshot::ScreenshotSystem;
        crate::platform::region_screenshot::MacScreenshotSystem {
            logger: self.logger.as_ref(),
        }
        .restore(contents, expected_count, &CancellationToken::new())
        .map(|_| ())
    }
    async fn selected_text(
        &self,
        cancellation: CancellationToken,
    ) -> Result<Option<crate::selection_session::SelectionData>, String> {
        super::text::select(
            self.copier.as_ref(),
            self.reader.as_ref(),
            self.logger.as_ref(),
            &cancellation,
        )
        .await
    }
    fn log(&self, message: &str) {
        let _ = self.logger.write("INFO", message);
    }
    fn log_error(&self, message: &str) {
        let _ = self.logger.write("ERROR", message);
    }
}
