use anyhow::Result;
use coosenpai_core::ports::{ScreenDisplay, WindowBounds};
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
    kCGWindowNumber as core_graphics_window_number, kCGWindowOwnerName, kCGWindowOwnerPID,
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

/// 常駐プロセスではなく、表示中の OS 範囲選択窓を観測する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionWindow {
    pub id: u32,
    pub owner_pid: i32,
}

/// 判定で除外した小窓や offscreen の窓も、タイムアウト診断用に保持する。
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionWindowCandidate {
    pub id: Option<i64>,
    pub owner_pid: Option<i64>,
    pub owner: String,
    pub name: Option<String>,
    pub bounds: Option<WindowBounds>,
    pub onscreen: Option<bool>,
    pub layer: Option<i64>,
}

pub struct SelectionWindowObservation {
    pub onscreen_window_ids: Vec<u32>,
    pub windows: Vec<SelectionWindow>,
    pub candidates: Vec<SelectionWindowCandidate>,
}

pub fn screenshot_selection_windows() -> Result<SelectionWindowObservation> {
    let displays = crate::display_capture::active_screen_displays()?;
    let ids = create_window_list(kCGWindowListOptionAll, kCGNullWindowID)
        .ok_or_else(|| anyhow::anyhow!("ウィンドウ一覧を取得できません"))?;
    let descriptions = create_description_from_array(ids)
        .ok_or_else(|| anyhow::anyhow!("ウィンドウ情報を取得できません"))?;
    Ok(SelectionWindowObservation::from_candidates(
        descriptions
            .iter()
            .map(|description| selection_candidate(&description)),
        &displays,
    ))
}

impl SelectionWindowObservation {
    pub fn from_candidates(
        candidates: impl IntoIterator<Item = SelectionWindowCandidate>,
        displays: &[ScreenDisplay],
    ) -> Self {
        let mut candidates = candidates.into_iter().collect::<Vec<_>>();
        let onscreen_window_ids = candidates
            .iter()
            .filter(|candidate| candidate.onscreen == Some(true))
            .filter_map(|candidate| u32::try_from(candidate.id?).ok().filter(|id| *id > 0))
            .collect();
        candidates.retain(|candidate| {
            matches!(
                candidate.owner.as_str(),
                "スクリーンショット" | "Screenshot"
            )
        });
        let windows = candidates
            .iter()
            .filter_map(|candidate| selection_window(candidate, displays))
            .collect();
        Self {
            windows,
            candidates,
            onscreen_window_ids,
        }
    }
}

fn selection_candidate(description: &CFDictionary<CFString, CFType>) -> SelectionWindowCandidate {
    // SAFETY: CoreGraphics のプロセス存続中有効な定数を borrowed として retain する。
    let (owner_key, layer_key, visible_key, id_key, bounds_key) = unsafe {
        (
            CFString::wrap_under_get_rule(kCGWindowOwnerName),
            CFString::wrap_under_get_rule(core_graphics_window_layer),
            CFString::wrap_under_get_rule(kCGWindowIsOnscreen),
            CFString::wrap_under_get_rule(core_graphics_window_number),
            CFString::wrap_under_get_rule(core_graphics_window_bounds),
        )
    };
    let owner = description
        .find(&owner_key)
        .and_then(|value| value.downcast::<CFString>())
        .map(|value| value.to_string())
        .unwrap_or_default();
    let bounds = description
        .find(&bounds_key)
        .and_then(|value| value.downcast::<CFDictionary>())
        .and_then(|value| CGRect::from_dict_representation(&value))
        .map(|rect| WindowBounds {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        });
    SelectionWindowCandidate {
        id: dictionary_number(description, &id_key),
        owner_pid: dictionary_number(description, &CFString::new("kCGWindowOwnerPID")),
        owner,
        name: description
            .find(CFString::new("kCGWindowName"))
            .and_then(|value| value.downcast::<CFString>())
            .map(|name| name.to_string()),
        bounds,
        onscreen: dictionary_boolean(description, &visible_key),
        layer: dictionary_number(description, &layer_key),
    }
}

fn selection_window(
    candidate: &SelectionWindowCandidate,
    displays: &[ScreenDisplay],
) -> Option<SelectionWindow> {
    if candidate.layer != Some(24) || candidate.onscreen != Some(true) {
        return None;
    }
    let bounds = candidate.bounds?;
    if !displays
        .iter()
        .any(|display| covers_display(bounds, display.bounds))
    {
        return None;
    }
    let id = u32::try_from(candidate.id?).ok()?;
    let owner_pid = i32::try_from(candidate.owner_pid?).ok()?;
    (id > 0 && owner_pid > 0).then_some(SelectionWindow { id, owner_pid })
}

fn covers_display(window: WindowBounds, display: WindowBounds) -> bool {
    const TOLERANCE: f64 = 4.0;
    [window.x, window.y, window.width, window.height]
        .iter()
        .all(|value| value.is_finite())
        && window.width > 0.0
        && window.height > 0.0
        && window.x <= display.x + TOLERANCE
        && window.y <= display.y + TOLERANCE
        && window.x + window.width >= display.x + display.width - TOLERANCE
        && window.y + window.height >= display.y + display.height - TOLERANCE
}

