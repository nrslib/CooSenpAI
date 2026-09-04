use super::helpers::{boolean, string, unknown_keys};
use super::{AppConfig, ConfigValidationIssue};
use crate::config::CompanionReminder;
use serde_json::{Map, Value};

pub(super) fn parse_app(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> AppConfig {
    issues.extend(unknown_keys(
        object,
        &["launchAtLogin", "checkForUpdates"],
        "app",
    ));
    AppConfig {
        launch_at_login: boolean(object, "launchAtLogin", false, "app.launchAtLogin", issues),
        check_for_updates: boolean(
            object,
            "checkForUpdates",
            true,
            "app.checkForUpdates",
            issues,
        ),
    }
}

pub(super) fn parse_reminders(
    value: Option<&Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> Vec<CompanionReminder> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(items) = value.as_array() else {
        issues.push(super::issue(
            "companion.reminders",
            "配列で指定してください。",
        ));
        return Vec::new();
    };
    items
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let Some(object) = value.as_object() else {
                issues.push(super::issue(
                    format!("companion.reminders[{index}]"),
                    "オブジェクトで指定してください。",
                ));
                return None;
            };
            issues.extend(unknown_keys(
                object,
                &["id", "time", "theme"],
                &format!("companion.reminders[{index}]"),
            ));
            let time = string(
                object,
                "time",
                "",
                &format!("companion.reminders[{index}].time"),
                issues,
            );
            let theme = string(
                object,
                "theme",
                "",
                &format!("companion.reminders[{index}].theme"),
                issues,
            );
            let id_path = format!("companion.reminders[{index}].id");
            if !object.contains_key("id") {
                issues.push(super::issue(&id_path, "必須です。"));
            }
            let id = string(object, "id", "", &id_path, issues);
            Some(CompanionReminder { id, time, theme })
        })
        .collect()
}

pub(super) fn review_time(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> String {
    match object.get("reviewTime") {
        None => "18:00".to_owned(),
        Some(Value::String(value)) => value.clone(),
        Some(_) => {
            issues.push(super::issue(
                "companion.reviewTime",
                "文字列で指定してください。",
            ));
            "18:00".to_owned()
        }
    }
}
