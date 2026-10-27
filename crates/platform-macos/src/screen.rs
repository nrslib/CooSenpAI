use coosenpai_core::ports::{
    BubbleDisplay, BubbleDisplayPort, PortError, ScreenPoint, WindowBounds,
};
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSWorkspace};
use objc2_core_foundation::{CFRetained, CFUUID};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGError, CGGetActiveDisplayList, CGMainDisplayID,
};
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;

#[link(name = "ColorSync", kind = "framework")]
extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: CGDirectDisplayID) -> Option<NonNull<CFUUID>>;
}

pub struct MacBubbleDisplay;

impl BubbleDisplayPort for MacBubbleDisplay {
    fn cursor_point(&self) -> Result<ScreenPoint, PortError> {
        cursor_point()
    }

    fn frontmost_window_point(&self) -> Result<Option<ScreenPoint>, PortError> {
        front_window_point()
    }

    fn displays(&self) -> Result<Vec<BubbleDisplay>, PortError> {
        bubble_displays()
    }
}

fn cursor_point() -> Result<ScreenPoint, PortError> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| PortError::Unavailable("マウス位置を取得できません".to_owned()))?;
    let location = CGEvent::new(source)
        .map_err(|_| PortError::Unavailable("マウス位置を取得できません".to_owned()))?
        .location();
    Ok(ScreenPoint {
        x: location.x,
        y: location.y,
    })
}

fn front_window_point() -> Result<Option<ScreenPoint>, PortError> {
    let Some(application) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
        return Ok(None);
    };
    let Some(bundle_id) = application.bundleIdentifier() else {
        return Ok(None);
    };
    let window = crate::window_info::frontmost_application_window(&bundle_id.to_string())
        .map_err(|error| PortError::Unavailable(error.to_string()))?;
    Ok(window.map(|window| ScreenPoint {
        x: window.bounds.origin.x + window.bounds.size.width / 2.0,
        y: window.bounds.origin.y + window.bounds.size.height / 2.0,
    }))
}

fn bubble_displays() -> Result<Vec<BubbleDisplay>, PortError> {
    let main_thread = MainThreadMarker::new().ok_or_else(|| {
        PortError::Unavailable("ディスプレイ一覧の取得にはメインスレッドが必要です".to_owned())
    })?;
    let ids = active_display_ids()?;
    let names = NSScreen::screens(main_thread)
        .iter()
        .map(|screen| {
            (
                screen.CGDirectDisplayID(),
                screen.localizedName().to_string(),
            )
        })
        .collect::<HashMap<_, _>>();
    let main_id = CGMainDisplayID();
    let mut seen = HashSet::new();
    ids.into_iter()
        .map(|id| {
            let uuid = display_uuid(id)?;
            if !seen.insert(uuid.clone()) {
                return Err(PortError::Unavailable(
                    "ディスプレイ UUID が重複しています".to_owned(),
                ));
            }
            let bounds = CGDisplayBounds(id);
            if !bounds.origin.x.is_finite()
                || !bounds.origin.y.is_finite()
                || !bounds.size.width.is_finite()
                || !bounds.size.height.is_finite()
                || bounds.size.width <= 0.0
                || bounds.size.height <= 0.0
            {
                return Err(PortError::Unavailable(
                    "ディスプレイの寸法が不正です".to_owned(),
                ));
            }
            Ok(BubbleDisplay {
                uuid,
                name: names.get(&id).cloned().unwrap_or_default(),
                is_main: id == main_id,
                bounds: WindowBounds {
                    x: bounds.origin.x,
                    y: bounds.origin.y,
                    width: bounds.size.width,
                    height: bounds.size.height,
                },
            })
        })
        .collect()
}

fn active_display_ids() -> Result<Vec<CGDirectDisplayID>, PortError> {
    let mut count = 0;
    // SAFETY: 一覧を NULL、件数を有効な out-pointer として渡す。
    let error = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut count) };
    if error != CGError::Success || count == 0 {
        return Err(PortError::Unavailable(
            "ディスプレイ一覧を取得できません".to_owned(),
        ));
    }
    let mut ids = vec![0; count as usize];
    // SAFETY: ids の容量と件数の out-pointer を API へ渡す。
    let error = unsafe { CGGetActiveDisplayList(count, ids.as_mut_ptr(), &mut count) };
    if error != CGError::Success || count == 0 || count as usize > ids.len() {
        return Err(PortError::Unavailable(
            "ディスプレイ一覧を取得できません".to_owned(),
        ));
    }
    ids.truncate(count as usize);
    Ok(ids)
}

fn display_uuid(id: CGDirectDisplayID) -> Result<String, PortError> {
    // SAFETY: 戻り値は Create 規則の CFUUID で、成功時に一度だけ CFRetained へ移す。
    let uuid = unsafe {
        CGDisplayCreateUUIDFromDisplayID(id)
            .map(|pointer| CFRetained::from_raw(pointer))
            .ok_or_else(|| {
                PortError::Unavailable("ディスプレイ UUID を取得できません".to_owned())
            })?
    };
    let bytes = uuid.uuid_bytes();
    Ok(uuid::Uuid::from_bytes([
        bytes.byte0,
        bytes.byte1,
        bytes.byte2,
        bytes.byte3,
        bytes.byte4,
        bytes.byte5,
        bytes.byte6,
        bytes.byte7,
        bytes.byte8,
        bytes.byte9,
        bytes.byte10,
        bytes.byte11,
        bytes.byte12,
        bytes.byte13,
        bytes.byte14,
        bytes.byte15,
    ])
    .hyphenated()
    .to_string())
}
