use crate::state::DesktopState;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::NSApplicationDidChangeScreenParametersNotification;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};
use std::ptr::NonNull;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::{mpsc, oneshot};

struct ScreenParametersObserver {
    center: Retained<NSNotificationCenter>,
    token: usize,
}

impl ScreenParametersObserver {
    fn new(center: Retained<NSNotificationCenter>, sender: mpsc::Sender<()>) -> Self {
        let block = block2::RcBlock::new(move |_notification: NonNull<NSNotification>| {
            let _ = sender.try_send(());
        });
        // SAFETY: 通知名と block のシグネチャは NSNotificationCenter の API に一致する。
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSApplicationDidChangeScreenParametersNotification),
                None,
                None,
                &block,
            )
        };
        Self {
            center,
            token: Retained::into_raw(token) as usize,
        }
    }
}

impl Drop for ScreenParametersObserver {
    fn drop(&mut self) {
        let pointer = self.token as *mut ProtocolObject<dyn NSObjectProtocol>;
        // SAFETY: 登録時に保持した token 一個分を、解除と解放の順で一度だけ処理する。
        unsafe {
            self.center.removeObserver(&*pointer.cast::<AnyObject>());
            drop(Retained::from_raw(pointer));
        }
    }
}

pub(crate) fn start(state: Arc<DesktopState>) {
    let (sender, mut receiver) = mpsc::channel(1);
    let observer = ScreenParametersObserver::new(NSNotificationCenter::defaultCenter(), sender);
    tauri::async_runtime::spawn(async move {
        let _observer = observer;
        refresh(&state).await;
        loop {
            tokio::select! {
                () = state.cancellation.cancelled() => break,
                change = receiver.recv() => {
                    if change.is_none() { break; }
                    refresh(&state).await;
                }
            }
        }
    });
}

pub(crate) async fn refresh(state: &Arc<DesktopState>) {
    let result = read_displays(state.app.clone()).await;
    state
        .publish_event(crate::snapshot_presenter::SnapshotEvent::DisplaysLoaded(
            result,
        ))
        .await;
}

async fn read_displays(
    app: AppHandle,
) -> Result<Vec<coosenpai_core::ports::BubbleDisplay>, String> {
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let result = crate::platform::bubble_display()
            .displays()
            .map_err(|error| error.to_string());
        let _ = sender.send(result);
    })
    .map_err(|error| error.to_string())?;
    receiver
        .await
        .map_err(|_| "ディスプレイ一覧を取得できません".to_owned())?
}
