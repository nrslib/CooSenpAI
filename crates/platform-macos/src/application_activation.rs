use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{
    NSApplicationDidResignActiveNotification, NSRunningApplication, NSWorkspace,
    NSWorkspaceApplicationKey, NSWorkspaceDidActivateApplicationNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol};
use std::ptr::NonNull;

/// 自アプリの非アクティブ化と他アプリのアクティブ化を観測する。
pub struct ApplicationActivationMonitor {
    observers: Vec<(Retained<NSNotificationCenter>, usize)>,
}

impl ApplicationActivationMonitor {
    pub fn new(
        deactivated: impl Fn() + Send + Sync + 'static,
        other_activated: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let center = NSNotificationCenter::defaultCenter();
        let resigned = block2::RcBlock::new(move |_notification: NonNull<NSNotification>| {
            deactivated();
        });
        // SAFETY: 通知名と block のシグネチャは AppKit の API に一致する。
        let deactivation = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSApplicationDidResignActiveNotification),
                None,
                None,
                &resigned,
            )
        };
        let workspace_center = NSWorkspace::sharedWorkspace().notificationCenter();
        let activated = block2::RcBlock::new(move |notification: NonNull<NSNotification>| {
            // SAFETY: NSNotificationCenter は block の実行中、通知を保持する。
            let notification = unsafe { notification.as_ref() };
            let Some(info) = notification.userInfo() else {
                return;
            };
            let Some(object) = info.objectForKey(unsafe { NSWorkspaceApplicationKey }) else {
                return;
            };
            let Some(application) = object.downcast_ref::<NSRunningApplication>() else {
                return;
            };
            if is_other_application(application.processIdentifier(), std::process::id() as i32) {
                other_activated();
            }
        });
        // SAFETY: workspace の通知名と block のシグネチャが API に一致する。
        let activation = unsafe {
            workspace_center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidActivateApplicationNotification),
                None,
                None,
                &activated,
            )
        };
        Self {
            observers: vec![
                (center, Retained::into_raw(deactivation) as usize),
                (workspace_center, Retained::into_raw(activation) as usize),
            ],
        }
    }
}

impl Drop for ApplicationActivationMonitor {
    fn drop(&mut self) {
        for (center, address) in &self.observers {
            let pointer = *address as *mut ProtocolObject<dyn NSObjectProtocol>;
            // SAFETY: 登録で保持した各 token を一度だけ解除・解放する。
            unsafe {
                center.removeObserver(&*pointer.cast::<AnyObject>());
                drop(Retained::from_raw(pointer));
            }
        }
    }
}

fn is_other_application(process_id: i32, self_pid: i32) -> bool {
    process_id > 0 && process_id != self_pid
}

