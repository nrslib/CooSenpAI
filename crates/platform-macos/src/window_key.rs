use anyhow::{Context, Result};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::NSWindowDidResignKeyNotification;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};
use std::ffi::c_void;
use std::ptr::NonNull;

/// 特定パネルの key 喪失だけを観測し、破棄時に購読を解除する。
pub struct WindowKeyMonitor {
    center: Retained<NSNotificationCenter>,
    token: usize,
}

impl WindowKeyMonitor {
    /// native_window は生存中の NSWindow とし、メインスレッドから登録する。
    pub fn new(
        native_window: *mut c_void,
        lost: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self> {
        let window =
            NonNull::new(native_window).context("対象ウィンドウのNSWindowを取得できません")?;
        // SAFETY: 呼出元の with_webview が NSWindow を保持している。
        Ok(Self::observe(
            unsafe { window.cast::<AnyObject>().as_ref() },
            lost,
        ))
    }

    fn observe(object: &AnyObject, lost: impl Fn() + Send + Sync + 'static) -> Self {
        let center = NSNotificationCenter::defaultCenter();
        let callback = block2::RcBlock::new(move |_notification: NonNull<NSNotification>| lost());
        // SAFETY: 通知名と block のシグネチャが一致し、object で対象を限定する。
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWindowDidResignKeyNotification),
                Some(object),
                None,
                &callback,
            )
        };
        Self {
            center,
            token: Retained::into_raw(token) as usize,
        }
    }
}

impl Drop for WindowKeyMonitor {
    fn drop(&mut self) {
        let pointer = self.token as *mut ProtocolObject<dyn NSObjectProtocol>;
        // SAFETY: 登録で保持した token を一度だけ解除・解放する。
        unsafe {
            self.center.removeObserver(&*pointer.cast::<AnyObject>());
            drop(Retained::from_raw(pointer));
        }
    }
}

