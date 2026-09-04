use super::helpers::{string, unknown_keys};
use super::{issue, parse_nested};
use crate::config::{
    ConfigValidationIssue, PopupConfig, PopupQuickAction, PopupQuickActionsConfig,
};
use serde_json::{Map, Value};

pub(super) fn parse_popup(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> PopupConfig {
    issues.extend(unknown_keys(object, &["quickActions"], "popup"));
    let quick_actions = parse_nested(
        object.get("quickActions"),
        PopupQuickActionsConfig::default(),
        "popup.quickActions",
        issues,
        parse_popup_quick_actions,
    );
    PopupConfig { quick_actions }
}

fn parse_popup_quick_actions(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> PopupQuickActionsConfig {
    issues.extend(unknown_keys(
        object,
        &["text", "image"],
        "popup.quickActions",
    ));
    let defaults = PopupQuickActionsConfig::default();
    PopupQuickActionsConfig {
        text: parse_quick_action_list(
            object.get("text"),
            defaults.text,
            "popup.quickActions.text",
            issues,
        ),
        image: parse_quick_action_list(
            object.get("image"),
            defaults.image,
            "popup.quickActions.image",
            issues,
        ),
    }
}

fn parse_quick_action_list(
    value: Option<&Value>,
    default: Vec<PopupQuickAction>,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> Vec<PopupQuickAction> {
    let Some(value) = value else { return default };
    let Some(items) = value.as_array() else {
        issues.push(issue(path, "配列で指定してください。"));
        return default;
    };
    items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let item_path = format!("{path}[{index}]");
            let Some(object) = item.as_object() else {
                issues.push(issue(&item_path, "オブジェクトで指定してください。"));
                return None;
            };
            issues.extend(unknown_keys(object, &["label", "message"], &item_path));
            Some(PopupQuickAction {
                label: string(object, "label", "", &format!("{item_path}.label"), issues),
                message: string(
                    object,
                    "message",
                    "",
                    &format!("{item_path}.message"),
                    issues,
                ),
            })
        })
        .collect()
}
