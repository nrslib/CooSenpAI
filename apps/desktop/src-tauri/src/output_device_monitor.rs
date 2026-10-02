use crate::state::DesktopState;
use std::ffi::c_void;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use tokio::sync::mpsc;

#[repr(C)]
#[derive(Clone, Copy)]
struct AudioObjectPropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

const DEVICES: u32 = u32::from_be_bytes(*b"dev#");
const ADDRESS: AudioObjectPropertyAddress = AudioObjectPropertyAddress {
    selector: DEVICES,
    scope: u32::from_be_bytes(*b"glob"),
    element: 0,
};
static CHANGES: OnceLock<mpsc::Sender<()>> = OnceLock::new();
static REGISTRATION_FAILED: AtomicBool = AtomicBool::new(false);

pub(crate) fn registration_failed() -> bool {
    REGISTRATION_FAILED.load(Ordering::Relaxed)
}

type PropertyListener =
    unsafe extern "C" fn(u32, u32, *const AudioObjectPropertyAddress, *mut c_void) -> i32;

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectAddPropertyListener(
        object: u32,
        address: *const AudioObjectPropertyAddress,
        listener: PropertyListener,
        client_data: *mut c_void,
    ) -> i32;
}

fn contains_device_list(addresses: &[AudioObjectPropertyAddress]) -> bool {
    addresses.iter().any(|address| address.selector == DEVICES)
}

fn registration_result(status: i32) -> Result<(), i32> {
    if status == 0 {
        Ok(())
    } else {
        Err(status)
    }
}

unsafe extern "C" fn device_list_changed(
    _: u32,
    count: u32,
    addresses: *const AudioObjectPropertyAddress,
    _: *mut c_void,
) -> i32 {
    if !addresses.is_null()
        && contains_device_list(unsafe { std::slice::from_raw_parts(addresses, count as usize) })
    {
        if let Some(sender) = CHANGES.get() {
            let _ = sender.try_send(());
        }
    }
    0
}

pub(crate) fn start(state: Arc<DesktopState>) -> Result<(), i32> {
    let (sender, mut receiver) = mpsc::channel(1);
    CHANGES.set(sender).map_err(|_| {
        REGISTRATION_FAILED.store(true, Ordering::Relaxed);
        -1
    })?;
    let status = unsafe {
        AudioObjectAddPropertyListener(1, &ADDRESS, device_list_changed, std::ptr::null_mut())
    };
    if let Err(status) = registration_result(status) {
        REGISTRATION_FAILED.store(true, Ordering::Relaxed);
        return Err(status);
    }
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                _ = state.cancellation.cancelled() => break,
                change = receiver.recv() => {
                    if change.is_none() { break; }
                    state.refresh_output_devices().await;
                }
            }
        }
    });
    Ok(())
}
