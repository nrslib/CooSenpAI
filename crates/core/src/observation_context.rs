use serde::{Deserialize, Serialize};

use super::ActivityTriggerKind;
use crate::ports::FocusElement;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingFrameContext {
    pub id: String,
    pub captured_at: String,
    pub trigger: ActivityTriggerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front_app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<FocusElement>,
}

impl PendingFrameContext {
    pub fn bounded(
        id: String,
        captured_at: String,
        trigger: ActivityTriggerKind,
        front_app: Option<String>,
        app: Option<String>,
        target: String,
        ocr_text: Option<String>,
    ) -> Self {
        Self::bounded_with_focus(
            id,
            captured_at,
            trigger,
            front_app,
            app,
            target,
            ocr_text,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn bounded_with_focus(
        id: String,
        captured_at: String,
        trigger: ActivityTriggerKind,
        front_app: Option<String>,
        app: Option<String>,
        target: String,
        ocr_text: Option<String>,
        focus: Option<FocusElement>,
    ) -> Self {
        Self {
            id,
            captured_at,
            trigger,
            front_app: front_app.map(|value| super::truncate(&value, 300)),
            app: app.map(|value| super::truncate(&value, 300)),
            target: super::truncate(&target, 500),
            ocr_text: ocr_text.map(|value| super::truncate(&value, 2_000)),
            focus: focus.map(FocusElement::bounded),
        }
    }
}
