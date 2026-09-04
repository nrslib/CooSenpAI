use coosenpai_core::ports::{BubbleDisplayPort, PortError, ScreenPoint};
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use objc2_app_kit::NSWorkspace;

pub struct MacBubbleDisplay;

impl BubbleDisplayPort for MacBubbleDisplay {
    fn cursor_point(&self) -> Result<ScreenPoint, PortError> {
        cursor_point()
    }

    fn frontmost_window_point(&self) -> Result<Option<ScreenPoint>, PortError> {
        front_window_point()
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
