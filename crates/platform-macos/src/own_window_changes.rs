use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{
    NSApplicationDidChangeScreenParametersNotification, NSApplicationWillHideNotification,
    NSApplicationWillUnhideNotification, NSWindowDidChangeOcclusionStateNotification,
    NSWindowDidChangeScreenNotification, NSWindowDidDeminiaturizeNotification,
    NSWindowDidMoveNotification, NSWindowDidResizeNotification, NSWindowWillCloseNotification,
    NSWindowWillEnterFullScreenNotification, NSWindowWillExitFullScreenNotification,
    NSWindowWillMiniaturizeNotification,
};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSObjectProtocol,
};

/// 自プロセスのウィンドウ変更を、元の位置へ戻った場合も含めて記録する。
pub struct OwnWindowChanges {
    revision: Arc<AtomicU64>,
    center: Retained<NSNotificationCenter>,
    tokens: Vec<usize>,
}

fn change_notifications() -> [&'static NSNotificationName; 12] {
    // SAFETY: AppKit の通知名はプロセスの寿命中有効な定数。
    unsafe {
        [
            NSWindowDidMoveNotification,
            NSWindowDidResizeNotification,
            NSWindowDidChangeOcclusionStateNotification,
            NSWindowWillCloseNotification,
            NSWindowWillMiniaturizeNotification,
            NSWindowDidDeminiaturizeNotification,
            NSWindowWillEnterFullScreenNotification,
            NSWindowWillExitFullScreenNotification,
            NSWindowDidChangeScreenNotification,
            NSApplicationWillHideNotification,
            NSApplicationWillUnhideNotification,
            NSApplicationDidChangeScreenParametersNotification,
        ]
    }
}

impl Default for OwnWindowChanges {
    fn default() -> Self {
        Self::with_center(NSNotificationCenter::defaultCenter())
    }
}

impl OwnWindowChanges {
    pub(crate) fn with_center(center: Retained<NSNotificationCenter>) -> Self {
        let revision = Arc::new(AtomicU64::new(0));
        let tokens = change_notifications()
            .into_iter()
            .map(|name| {
                let revision = revision.clone();
                let block = block2::RcBlock::new(move |_notification: NonNull<NSNotification>| {
                    revision.fetch_add(1, Ordering::AcqRel);
                });
                // queue=None で通知元のスレッド上で世代を更新し、メインキューへの遅延を作らない。
                // SAFETY: 通知名と block のシグネチャは NSNotificationCenter の API に一致する。
                let token = unsafe {
                    center.addObserverForName_object_queue_usingBlock(
                        Some(name),
                        None,
                        None,
                        &block,
                    )
                };
                Retained::into_raw(token) as usize
            })
            .collect();
        Self {
            revision,
            center,
            tokens,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
}

impl Drop for OwnWindowChanges {
    fn drop(&mut self) {
        for address in &self.tokens {
            let pointer = *address as *mut ProtocolObject<dyn NSObjectProtocol>;
            // SAFETY: 各アドレスは登録時に保持した token 一個分。解除と解放は一回だけ。
            unsafe {
                self.center.removeObserver(&*pointer.cast::<AnyObject>());
                drop(Retained::from_raw(pointer));
            }
        }
    }
}

