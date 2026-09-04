use super::super::{issue, ConfigValidationIssue, WatchAppConfig};
use super::helpers::{boolean, string, unknown_keys};
use serde_json::Value;

pub(super) fn parse_watch_apps(
    value: Option<&Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> Vec<WatchAppConfig> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(values) = value.as_array() else {
        issues.push(issue("watch.apps", "配列で指定してください。"));
        return Vec::new();
    };
    values
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            let path = format!("watch.apps[{index}]");
            let Some(object) = value.as_object() else {
                issues.push(issue(path, "オブジェクトで指定してください。"));
                return None;
            };
            issues.extend(unknown_keys(
                object,
                &["bundleId", "name", "enabled"],
                &path,
            ));
            Some(WatchAppConfig {
                bundle_id: string(object, "bundleId", "", &format!("{path}.bundleId"), issues),
                name: string(object, "name", "", &format!("{path}.name"), issues),
                enabled: boolean(object, "enabled", true, &format!("{path}.enabled"), issues),
            })
        })
        .collect()
}
