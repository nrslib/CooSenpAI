use anyhow::Result;
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::geometry::CGRect;
use core_graphics::window::{
    create_description_from_array, create_window_list, kCGNullWindowID,
    kCGWindowBounds as core_graphics_window_bounds, kCGWindowIsOnscreen,
    kCGWindowLayer as core_graphics_window_layer, kCGWindowListOptionAll,
    kCGWindowNumber as core_graphics_window_number, kCGWindowOwnerPID,
};
use objc2_app_kit::NSRunningApplication;
use objc2_foundation::NSString;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ApplicationWindow {
    pub id: u32,
    pub bounds: CGRect,
}

pub(crate) fn application_window(bundle_id: &str) -> Result<Option<ApplicationWindow>> {
    let windows = application_windows(bundle_id)?;
    Ok(largest_window(&windows))
}

pub(crate) fn frontmost_application_window(bundle_id: &str) -> Result<Option<ApplicationWindow>> {
    let windows = application_windows(bundle_id)?;
    Ok(frontmost_window(&windows))
}

fn application_windows(bundle_id: &str) -> Result<Vec<ApplicationWindow>> {
    let applications = NSRunningApplication::runningApplicationsWithBundleIdentifier(
        &NSString::from_str(bundle_id),
    );
    let pids = applications
        .iter()
        .map(|application| i64::from(application.processIdentifier()))
        .collect::<std::collections::HashSet<_>>();
    if pids.is_empty() {
        return Ok(Vec::new());
    }
    let Some(ids) = create_window_list(kCGWindowListOptionAll, kCGNullWindowID) else {
        anyhow::bail!("ウィンドウ一覧を取得できません")
    };
    let Some(descriptions) = create_description_from_array(ids) else {
        anyhow::bail!("ウィンドウ情報を取得できません")
    };
    // SAFETY: CoreGraphics の kCGWindow* 定数はプロセス存続中有効な borrowed
    // CFString であり、wrap_under_get_rule は retain 所有権を奪わない。
    let (owner_pid, window_number, window_layer, on_screen, bounds) = unsafe {
        (
            CFString::wrap_under_get_rule(kCGWindowOwnerPID),
            CFString::wrap_under_get_rule(core_graphics_window_number),
            CFString::wrap_under_get_rule(core_graphics_window_layer),
            CFString::wrap_under_get_rule(kCGWindowIsOnscreen),
            CFString::wrap_under_get_rule(core_graphics_window_bounds),
        )
    };
    let mut candidates = Vec::new();
    // CGWindow の front-to-back 順を保つ。撮影用の面積順ソートは呼び出し側で行う。
    for description in descriptions.iter() {
        let Some(pid) = dictionary_number(&description, &owner_pid) else {
            continue;
        };
        let Some(layer) = dictionary_number(&description, &window_layer) else {
            continue;
        };
        let Some(id) = dictionary_number(&description, &window_number) else {
            continue;
        };
        let Some(visible) = dictionary_boolean(&description, &on_screen) else {
            continue;
        };
        if !pids.contains(&pid) || layer != 0 || !visible || id <= 0 || id > i64::from(u32::MAX) {
            continue;
        }
        let Some(rect) = description
            .find(&bounds)
            .and_then(|value| value.downcast::<CFDictionary>())
            .and_then(|value| CGRect::from_dict_representation(&value))
        else {
            continue;
        };
        if window_area(rect) > 0.0 {
            candidates.push(ApplicationWindow {
                id: id as u32,
                bounds: rect,
            });
        }
    }
    Ok(candidates)
}

fn largest_window(candidates: &[ApplicationWindow]) -> Option<ApplicationWindow> {
    candidates
        .iter()
        .copied()
        .max_by(|left, right| window_area(left.bounds).total_cmp(&window_area(right.bounds)))
}

fn frontmost_window(candidates: &[ApplicationWindow]) -> Option<ApplicationWindow> {
    candidates.first().copied()
}

fn window_area(bounds: CGRect) -> f64 {
    bounds.size.width * bounds.size.height
}

fn dictionary_number(dictionary: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i64> {
    dictionary.find(key)?.downcast::<CFNumber>()?.to_i64()
}

fn dictionary_boolean(dictionary: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<bool> {
    dictionary
        .find(key)?
        .downcast::<CFBoolean>()
        .map(bool::from)
}

