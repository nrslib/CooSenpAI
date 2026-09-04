use super::super::{
    is_valid_avatar_path, issue, unknown_issue, AudioConfig, ChatConfig, ConfigValidationIssue,
    DebugConfig, SpeechConfig, UiConfig,
};
use serde_json::{Map, Value};
use std::convert::TryFrom;
use std::path::Path;

pub(super) fn positive_u64(
    object: &Map<String, Value>,
    key: &str,
    default: u64,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> u64 {
    let Some(value) = object.get(key) else {
        return default;
    };
    match value.as_u64().filter(|value| *value > 0) {
        Some(value) => value,
        None => {
            issues.push(issue(path, "正の整数で指定してください。"));
            default
        }
    }
}

pub(super) fn unknown_keys(
    object: &Map<String, Value>,
    allowed: &[&str],
    path: &str,
) -> Vec<ConfigValidationIssue> {
    object
        .keys()
        .filter(|key| !allowed.contains(&key.as_str()))
        .map(|key| unknown_issue(path, key, "未知のキーです。"))
        .collect()
}

pub(super) fn positive_u32(
    object: &Map<String, Value>,
    key: &str,
    default: u32,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> u32 {
    match u32::try_from(positive_u64(object, key, u64::from(default), path, issues)) {
        Ok(value) => value,
        Err(_) => {
            issues.push(issue(path, "正の整数で指定してください。"));
            default
        }
    }
}

pub(super) fn positive_usize(
    object: &Map<String, Value>,
    key: &str,
    default: usize,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> usize {
    match usize::try_from(positive_u64(object, key, default as u64, path, issues)) {
        Ok(value) => value,
        Err(_) => {
            issues.push(issue(path, "正の整数で指定してください。"));
            default
        }
    }
}

pub(super) fn nonnegative_u32(
    object: &Map<String, Value>,
    key: &str,
    default: u32,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> u32 {
    let Some(value) = object.get(key) else {
        return default;
    };
    match value.as_u64().and_then(|value| u32::try_from(value).ok()) {
        Some(value) => value,
        None => {
            issues.push(issue(path, "0以上の整数で指定してください。"));
            default
        }
    }
}

pub(super) fn optional_nonnegative_u32(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> Option<u32> {
    match object.get(key) {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_u64().and_then(|value| u32::try_from(value).ok()) {
            Some(value) => Some(value),
            None => {
                issues.push(issue(path, "0以上の整数または null で指定してください。"));
                None
            }
        },
    }
}

pub(super) fn frames_per_send(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> usize {
    let Some(value) = object.get("framesPerSend") else {
        return 4;
    };
    match value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| (1..=12).contains(value))
    {
        Some(value) => value,
        None => {
            issues.push(issue(
                "watch.framesPerSend",
                "1以上12以下の整数で指定してください。",
            ));
            4
        }
    }
}

pub(super) fn positive_number(
    object: &Map<String, Value>,
    key: &str,
    default: f64,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> f64 {
    let Some(value) = object.get(key) else {
        return default;
    };
    match value
        .as_f64()
        .filter(|value| value.is_finite() && *value > 0.0)
    {
        Some(value) => value,
        None => {
            issues.push(issue(path, "正の数で指定してください。"));
            default
        }
    }
}

pub(super) fn boolean(
    object: &Map<String, Value>,
    key: &str,
    default: bool,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> bool {
    let Some(value) = object.get(key) else {
        return default;
    };
    match value.as_bool() {
        Some(value) => value,
        None => {
            issues.push(issue(path, "true または false で指定してください。"));
            default
        }
    }
}

pub(super) fn string(
    object: &Map<String, Value>,
    key: &str,
    default: &str,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> String {
    let Some(value) = object.get(key) else {
        return default.to_owned();
    };
    match value.as_str().filter(|value| !value.is_empty()) {
        Some(value) => value.to_owned(),
        None => {
            issues.push(issue(path, "空でない文字列で指定してください。"));
            default.to_owned()
        }
    }
}

pub(super) fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> Option<String> {
    match object.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.clone()),
        Some(_) => {
            issues.push(issue(
                path,
                "空でない文字列または null で指定してください。",
            ));
            None
        }
    }
}

pub(super) fn parse_audio(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> AudioConfig {
    issues.extend(unknown_keys(
        object,
        &["enabled", "mic", "speaker", "debugDumpDir"],
        "audio",
    ));
    AudioConfig {
        enabled: boolean(object, "enabled", false, "audio.enabled", issues),
        mic: boolean(object, "mic", false, "audio.mic", issues),
        speaker: boolean(object, "speaker", false, "audio.speaker", issues),
        debug_dump_dir: optional_string(object, "debugDumpDir", "audio.debugDumpDir", issues),
    }
}

pub(super) fn parse_speech(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> SpeechConfig {
    issues.extend(unknown_keys(
        object,
        &["locale", "mode", "confirmBeforeSend", "inputDevice"],
        "speech",
    ));
    SpeechConfig {
        locale: string(object, "locale", "system", "speech.locale", issues),
        mode: enum_string(
            object,
            "mode",
            "toggle",
            &["pushToTalk", "toggle"],
            "speech.mode",
            issues,
        ),
        confirm_before_send: boolean(
            object,
            "confirmBeforeSend",
            true,
            "speech.confirmBeforeSend",
            issues,
        ),
        input_device: string(
            object,
            "inputDevice",
            "default",
            "speech.inputDevice",
            issues,
        ),
    }
}

pub(super) fn parse_chat(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> ChatConfig {
    issues.extend(unknown_keys(object, &["whileThinking"], "chat"));
    ChatConfig {
        while_thinking: enum_string(
            object,
            "whileThinking",
            "queue",
            &["queue", "append"],
            "chat.whileThinking",
            issues,
        ),
    }
}

pub(super) fn parse_ui(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> UiConfig {
    issues.extend(unknown_keys(
        object,
        &[
            "avatarColor",
            "avatarPath",
            "theme",
            "font",
            "thoughtBubble",
        ],
        "ui",
    ));
    let avatar_color = optional_string(object, "avatarColor", "ui.avatarColor", issues);
    if avatar_color.as_ref().is_some_and(|value| {
        value.len() != 7
            || !value.starts_with('#')
            || !value[1..]
                .chars()
                .all(|character| character.is_ascii_hexdigit())
    }) {
        issues.push(issue("ui.avatarColor", "#RRGGBB 形式で指定してください。"));
    }
    let avatar_path = optional_string(object, "avatarPath", "ui.avatarPath", issues);
    if avatar_path
        .as_ref()
        .is_some_and(|value| !is_valid_avatar_path(value))
    {
        issues.push(issue(
            "ui.avatarPath",
            "state 配下の png / jpg / jpeg ファイルパスで指定してください。",
        ));
    }
    UiConfig {
        avatar_color,
        avatar_path,
        theme: enum_string(
            object,
            "theme",
            "system",
            &["system", "light", "dark"],
            "ui.theme",
            issues,
        ),
        font: string(object, "font", "system", "ui.font", issues),
        thought_bubble: boolean(object, "thoughtBubble", true, "ui.thoughtBubble", issues),
    }
}

pub(super) fn parse_debug(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> DebugConfig {
    issues.extend(unknown_keys(object, &["enabled"], "debug"));
    DebugConfig {
        enabled: boolean(object, "enabled", false, "debug.enabled", issues),
    }
}

pub(super) fn effort(
    object: &Map<String, Value>,
    key: &str,
    default: &str,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> String {
    let Some(value) = object.get(key) else {
        return default.to_owned();
    };
    match value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => value.to_owned(),
        None => {
            issues.push(issue(path, "空白以外の文字列で指定してください。"));
            default.to_owned()
        }
    }
}

pub(super) fn provider(
    object: &Map<String, Value>,
    key: &str,
    default: &str,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> String {
    let Some(value) = object.get(key) else {
        return default.to_owned();
    };
    match value
        .as_str()
        .filter(|value| matches!(*value, "codex" | "claude" | "opencode"))
    {
        Some(value) => value.to_owned(),
        None => {
            issues.push(issue(
                path,
                "codex、claude、opencode のいずれかで指定してください。",
            ));
            default.to_owned()
        }
    }
}

pub(super) fn enum_string(
    object: &Map<String, Value>,
    key: &str,
    default: &str,
    allowed: &[&str],
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> String {
    let Some(value) = object.get(key) else {
        return default.to_owned();
    };
    match value.as_str().filter(|value| allowed.contains(value)) {
        Some(value) => value.to_owned(),
        None => {
            issues.push(issue(
                path,
                format!("{} のいずれかで指定してください。", allowed.join("、")),
            ));
            default.to_owned()
        }
    }
}

pub(super) fn persona(
    object: &Map<String, Value>,
    key: &str,
    default: &str,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> String {
    let value = string(object, key, default, path, issues);
    if !valid_name(&value) {
        issues.push(issue(path, "性格のIDが不正です。"));
        default.to_owned()
    } else {
        value
    }
}

pub(super) fn executable(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> Option<String> {
    let value = object.get(key)?;
    match value {
        Value::Null => None,
        Value::String(value) if Path::new(value).is_absolute() => Some(value.clone()),
        _ => {
            issues.push(issue(
                path,
                "実行ファイルは絶対パスまたは null で指定してください。",
            ));
            None
        }
    }
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric() || index > 0 && matches!(byte, b'_' | b'-')
        })
}
