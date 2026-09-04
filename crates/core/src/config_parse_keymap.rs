use super::super::KeymapConfig;
use super::helpers::{enum_string, unknown_keys};
use super::{issue, ConfigValidationIssue};
use serde_json::{Map, Value};

pub(super) fn parse_keymap(
    root: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> KeymapConfig {
    let Some(value) = root.get("keymap") else {
        return KeymapConfig::default();
    };
    let Some(object) = value.as_object() else {
        issues.push(issue("keymap", "オブジェクトで指定してください。"));
        return KeymapConfig::default();
    };
    issues.extend(unknown_keys(
        object,
        &[
            "captureRegion",
            "microphone",
            "togglePanel",
            "toggleWatch",
            "sendText",
            "copyLastReply",
            "sendKey",
        ],
        "keymap",
    ));
    KeymapConfig {
        capture_region: optional_shortcut(
            object,
            "captureRegion",
            "Alt+Shift+4",
            "keymap.captureRegion",
            issues,
        ),
        microphone: optional_shortcut(
            object,
            "microphone",
            "Alt+Space",
            "keymap.microphone",
            issues,
        ),
        toggle_panel: optional_shortcut(
            object,
            "togglePanel",
            "Alt+Shift+V",
            "keymap.togglePanel",
            issues,
        ),
        toggle_watch: optional_shortcut(
            object,
            "toggleWatch",
            "Alt+Shift+W",
            "keymap.toggleWatch",
            issues,
        ),
        send_text: optional_shortcut(object, "sendText", "Alt+Shift+C", "keymap.sendText", issues),
        copy_last_reply: optional_shortcut(
            object,
            "copyLastReply",
            "Alt+Shift+Y",
            "keymap.copyLastReply",
            issues,
        ),
        send_key: enum_string(
            object,
            "sendKey",
            "enter",
            &["enter", "cmdEnter"],
            "keymap.sendKey",
            issues,
        ),
    }
}

fn optional_shortcut(
    object: &Map<String, Value>,
    key: &str,
    default: &str,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> Option<String> {
    match object.get(key) {
        None => Some(default.to_owned()),
        Some(Value::Null) => None,
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        Some(_) => {
            issues.push(issue(
                path,
                "ショートカット文字列または null で指定してください。",
            ));
            Some(default.to_owned())
        }
    }
}
