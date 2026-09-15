use coosenpai_core::ports::ScreenPoint;
use std::time::{Duration, Instant};

pub(crate) const EDGE_RECALL_HOLD: Duration = Duration::from_millis(500);
pub(crate) const EDGE_RECALL_DEPTH: f64 = 24.0;
pub(crate) const EDGE_POLL_UNKNOWN: u8 = 0;
pub(crate) const EDGE_POLL_AWAY: u8 = 1;
pub(crate) const EDGE_POLL_AT_EDGE: u8 = 2;

pub(crate) fn is_suppressed(
    enabled: bool,
    main_focused: bool,
    onboarding: bool,
    bubble_available: bool,
) -> bool {
    !enabled || main_focused || onboarding || !bubble_available
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EdgeRecallDebounce {
    entered_at: Option<Instant>,
    triggered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BubbleDisplayArea {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

impl BubbleDisplayArea {
    fn contains(self, point: ScreenPoint) -> bool {
        point.x >= self.x
            && point.x < self.x + self.width
            && point.y >= self.y
            && point.y < self.y + self.height
    }
}

pub(crate) fn is_at_bubble_edge(
    point: ScreenPoint,
    area: BubbleDisplayArea,
    position: &str,
) -> bool {
    if !area.contains(point) {
        return false;
    }
    let Some((vertical, horizontal)) = position.split_once('-') else {
        return false;
    };
    let (horizontal_edge, vertical_edge) = match (vertical, horizontal) {
        ("top", "left") | ("bottom", "left") => (
            point.x <= area.x + EDGE_RECALL_DEPTH,
            if vertical == "top" {
                point.y <= area.y + EDGE_RECALL_DEPTH
            } else {
                point.y >= area.y + area.height - EDGE_RECALL_DEPTH
            },
        ),
        ("top", "right") | ("bottom", "right") => (
            point.x >= area.x + area.width - EDGE_RECALL_DEPTH,
            if vertical == "top" {
                point.y <= area.y + EDGE_RECALL_DEPTH
            } else {
                point.y >= area.y + area.height - EDGE_RECALL_DEPTH
            },
        ),
        _ => return false,
    };
    horizontal_edge && vertical_edge
}

pub(crate) fn observe_edge(
    previous: EdgeRecallDebounce,
    at_edge: bool,
    now: Instant,
) -> (EdgeRecallDebounce, bool) {
    if !at_edge {
        return (EdgeRecallDebounce::default(), false);
    }
    let entered_at = previous.entered_at.unwrap_or(now);
    let triggered =
        previous.triggered || now.saturating_duration_since(entered_at) >= EDGE_RECALL_HOLD;
    (
        EdgeRecallDebounce {
            entered_at: Some(entered_at),
            triggered,
        },
        triggered && !previous.triggered,
    )
}

pub(crate) fn suspend_edge_recall(previous: EdgeRecallDebounce) -> EdgeRecallDebounce {
    EdgeRecallDebounce {
        entered_at: None,
        triggered: previous.triggered,
    }
}

pub(crate) fn should_publish_edge_poll(previous_at_edge: Option<bool>, at_edge: bool) -> bool {
    at_edge || previous_at_edge != Some(false)
}

