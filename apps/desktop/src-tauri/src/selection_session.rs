use crate::capture::CaptureKind;
use crate::platform::region_screenshot::{ClipboardContents, ScreenshotShortcut};
use coosenpai_core::attachments::BoundedTextAttachment;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(50);
const TRANSITION_TIMEOUT: Duration = Duration::from_secs(1);
const DRAINING_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionState {
    Closed,
    Opening,
    AwaitingOpenToClose,
    Open,
    Closing,
    Draining,
}

#[derive(Debug)]
enum SessionInput {
    Open,
    Close,
    Shutdown,
    ShortcutPosted,
    EscapePosted,
    Windows(bool),
    ClipboardChanged(i64),
    ImageRead(bool),
    Timeout,
    Failed(String),
    ObservationFailed(String),
}

#[derive(Debug, PartialEq, Eq)]
enum SessionAction {
    Start,
    Escape,
    Observe,
    ReadImage,
    TrackClipboard,
    UpdateClipboardBaseline,
    Opened,
    Captured,
    Cancelled,
    Drained,
    Closed,
    CloseAccepted,
    Failed(String),
    RejectOpen,
    Ignore,
}

/// OSの選択窓の状態と期限は、この部品だけが所有する。
struct SelectionSession {
    state: SessionState,
    timeouts_enabled: bool,
    deadline: Option<Instant>,
    next_poll: Option<Instant>,
    baseline_windows: Vec<u32>,
    tracked_window: Option<u32>,
    clipboard_baseline: i64,
    clipboard_changed: i64,
    image_captured: bool,
}

impl Default for SelectionSession {
    fn default() -> Self {
        Self {
            state: SessionState::Closed,
            timeouts_enabled: true,
            deadline: None,
            next_poll: None,
            baseline_windows: Vec::new(),
            tracked_window: None,
            clipboard_baseline: 0,
            clipboard_changed: 0,
            image_captured: false,
        }
    }
}

impl SelectionSession {
    fn transition(&mut self, input: SessionInput, now: Instant) -> SessionAction {
        use SessionAction as A;
        use SessionInput as I;
        use SessionState as S;
        if matches!(input, I::Windows(_)) && self.deadline.is_some_and(|deadline| now >= deadline) {
            return self.transition(I::Timeout, now);
        }
        let (state, action) = match (self.state, input) {
            (S::Closed | S::Draining, I::ClipboardChanged(_)) => return A::Ignore,
            (_, I::ClipboardChanged(count))
                if count == self.clipboard_baseline || self.image_captured =>
            {
                return A::Ignore;
            }
            (S::Opening | S::AwaitingOpenToClose | S::Closing, I::ClipboardChanged(count)) => {
                self.clipboard_changed = count;
                return A::TrackClipboard;
            }
            (S::Open, I::ClipboardChanged(count)) => {
                self.clipboard_changed = count;
                return A::ReadImage;
            }
            (S::Open, I::ImageRead(true)) => {
                self.image_captured = true;
                (S::Draining, A::Captured)
            }
            (S::Open, I::ImageRead(false)) => {
                self.clipboard_baseline = self.clipboard_changed;
                return A::UpdateClipboardBaseline;
            }
            (
                S::Closed | S::Opening | S::AwaitingOpenToClose | S::Closing | S::Draining,
                I::ImageRead(_),
            ) => return A::Ignore,
            (S::Closed | S::Draining, I::Open) => (S::Closed, A::Start),
            (_, I::Open) => return A::RejectOpen,
            (S::Closed, I::Close | I::Shutdown) => (S::Closed, A::Closed),
            (S::Opening | S::AwaitingOpenToClose | S::Open, I::Shutdown) => (S::Closing, A::Escape),
            (S::Closing | S::Draining, I::Close | I::Shutdown) => (self.state, A::CloseAccepted),
            (S::Closing, I::EscapePosted) => (S::Draining, A::Cancelled),
            (S::Opening, I::Close) => (S::AwaitingOpenToClose, A::Observe),
            (S::Open, I::Close) => (S::Closing, A::Escape),
            (S::AwaitingOpenToClose, I::Close) => return A::Ignore,
            (S::Closed, I::ShortcutPosted) => (S::Opening, A::Observe),
            (S::Opening, I::Windows(true)) => (S::Open, A::Opened),
            (S::AwaitingOpenToClose, I::Windows(true)) => (S::Closing, A::Escape),
            (S::Open | S::Closing, I::Windows(false)) => (S::Closed, A::Closed),
            (S::Draining, I::Windows(false)) => (S::Closed, A::Drained),
            (
                S::Opening | S::AwaitingOpenToClose | S::Open | S::Closing | S::Draining,
                I::Windows(_),
            ) => (self.state, A::Observe),
            (S::Opening | S::AwaitingOpenToClose, I::Timeout) => (
                S::Closed,
                A::Failed("選択窓の表示を1秒以内に確認できませんでした".into()),
            ),
            (S::Closing, I::Timeout) => (
                S::Closed,
                A::Failed("選択窓への Esc 送出が完了しませんでした".into()),
            ),
            (S::Draining, I::Timeout) => (
                S::Closed,
                A::Failed("選択窓の後始末を6秒以内に確認できませんでした".into()),
            ),
            (
                S::Opening | S::AwaitingOpenToClose | S::Open | S::Closing | S::Draining,
                I::ObservationFailed(error),
            ) => (S::Closed, A::Failed(error)),
            (S::Closed, I::ObservationFailed(_)) => return A::Ignore,
            (_, I::Failed(error)) => (S::Closed, A::Failed(error)),
            (
                S::Opening | S::AwaitingOpenToClose | S::Open | S::Closing | S::Draining,
                I::ShortcutPosted,
            )
            | (
                S::Closed | S::Opening | S::AwaitingOpenToClose | S::Open | S::Draining,
                I::EscapePosted,
            )
            | (S::Closed | S::Open, I::Timeout)
            | (S::Closed, I::Windows(_)) => return A::Ignore,
        };
        if state != self.state && state != S::AwaitingOpenToClose {
            self.deadline = match state {
                _ if !self.timeouts_enabled => None,
                S::Opening => Some(now + TRANSITION_TIMEOUT),
                S::Closing => Some(now + TRANSITION_TIMEOUT),
                S::Draining => Some(now + DRAINING_TIMEOUT),
                _ => None,
            };
        }
        self.state = state;
        self.next_poll = (state != S::Closed).then_some(now + POLL_INTERVAL);
        action
    }

    fn windows_present(
        &mut self,
        windows: &[crate::platform::SelectionWindow],
        onscreen_window_ids: &[u32],
    ) -> bool {
        if self.tracked_window.is_none()
            && matches!(
                self.state,
                SessionState::Opening | SessionState::AwaitingOpenToClose | SessionState::Closing
            )
        {
            self.tracked_window = windows
                .iter()
                .map(|window| window.id)
                .filter(|id| !self.baseline_windows.contains(id))
                .max();
        }
        // 選択窓は表示中に bounds が変わるため、Opened 後は同じ id の存続だけを見る。
        let present = self
            .tracked_window
            .is_some_and(|id| onscreen_window_ids.contains(&id));
        if !present {
            self.tracked_window = None;
        }
        present
    }

    fn wake_at(&self) -> Option<Instant> {
        self.next_poll
            .map(|poll| self.deadline.map_or(poll, |deadline| poll.min(deadline)))
    }

    fn restoration_count(&self) -> Option<i64> {
        (self.clipboard_changed != self.clipboard_baseline).then_some(self.clipboard_changed)
    }
}

struct ImageSetup {
    pub shortcut: ScreenshotShortcut,
    pub count: i64,
    pub contents: Option<ClipboardContents>,
    pub windows: Vec<u32>,
}

struct SessionObservation {
    pub onscreen_window_ids: Vec<u32>,
    pub windows: Vec<crate::platform::SelectionWindow>,
    pub candidates: Vec<crate::platform::SelectionWindowCandidate>,
    pub change_count: i64,
}

pub(crate) enum SelectionData {
    Image(Vec<u8>),
    Text {
        attachment: Option<BoundedTextAttachment>,
        permission_required: bool,
    },
}

#[async_trait::async_trait]
trait SelectionSessionPort: Send + Sync + 'static {
    async fn prepare_image(&self) -> Result<ImageSetup, String>;
    async fn post_image(&self, shortcut: ScreenshotShortcut) -> Result<(), String>;
    async fn escape(&self) -> Result<(), String>;
    async fn observe(&self) -> Result<SessionObservation, String>;
    async fn image(&self) -> Result<Option<Vec<u8>>, String>;
    async fn restore(&self, contents: ClipboardContents, expected_count: i64)
        -> Result<(), String>;
    async fn selected_text(
        &self,
        cancellation: CancellationToken,
    ) -> Result<Option<SelectionData>, String>;
    fn log(&self, message: &str);
    fn log_error(&self, message: &str);
}

pub(crate) enum SessionEvent {
    Opened,
    Completed(Result<Option<SelectionData>, String>),
}

struct SelectionReply(mpsc::UnboundedSender<SessionEvent>);

impl SelectionReply {
    fn opened(&self) {
        let _ = self.0.send(SessionEvent::Opened);
    }

    fn send(self, result: Result<Option<SelectionData>, String>) {
        let _ = self.0.send(SessionEvent::Completed(result));
    }
}

pub(crate) struct SelectionEvents(mpsc::UnboundedReceiver<SessionEvent>);

impl SelectionEvents {
    pub(crate) async fn next(&mut self) -> Result<SessionEvent, String> {
        self.0
            .recv()
            .await
            .ok_or_else(|| "選択セッションの応答がありません".to_owned())
    }
}

enum Request {
    Open {
        generation: u64,
        kind: CaptureKind,
        reply: SelectionReply,
    },
    Close {
        generation: u64,
        shutdown: bool,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

#[derive(Clone)]
pub(crate) struct SelectionSessionHandle {
    sender: mpsc::UnboundedSender<Request>,
}

impl SelectionSessionHandle {
    pub(crate) fn native(state: std::sync::Arc<crate::state::DesktopState>) -> Self {
        let (handle, run) = Self::channel(native::NativeSelectionPort {
            logger: state.logger.clone(),
            copier: state.selected_text_copier.clone(),
            reader: state.clipboard_reader.clone(),
        });
        tauri::async_runtime::spawn(run);
        handle
    }

    fn channel(
        port: impl SelectionSessionPort,
    ) -> (Self, impl std::future::Future<Output = ()> + Send) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            Self { sender },
            runtime::SessionRuntime::new(port, true).run(receiver),
        )
    }

    pub(crate) fn open(
        &self,
        generation: u64,
        kind: CaptureKind,
    ) -> Result<SelectionEvents, String> {
        let (reply, result) = mpsc::unbounded_channel();
        self.sender
            .send(Request::Open {
                generation,
                kind,
                reply: SelectionReply(reply),
            })
            .map_err(|_| "選択セッションが終了しました".to_owned())?;
        Ok(SelectionEvents(result))
    }

    pub(crate) async fn close(&self, generation: u64, shutdown: bool) -> Result<(), String> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(Request::Close {
                generation,
                shutdown,
                reply,
            })
            .map_err(|_| "選択セッションが終了しました".to_owned())?;
        result
            .await
            .map_err(|_| "選択セッションの終了応答がありません".to_owned())?
    }
}

#[cfg(test)]
#[path = "selection_session_tests.rs"]
pub(crate) mod tests;

#[path = "selection_session_native.rs"]
mod native;
#[path = "selection_session_runtime.rs"]
mod runtime;
#[path = "selection_session_text.rs"]
mod text;
