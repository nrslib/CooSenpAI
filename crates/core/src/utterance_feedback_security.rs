use crate::persistence::PersistenceError;
use crate::state::ObservationRecord;
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) fn sanitize_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter(|(key, _)| !sensitive_key(key) && !discarded_path_key(key))
                .map(|(key, value)| (key.clone(), sanitize_json(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(sanitize_json).collect()),
        Value::String(value) => Value::String(super::archive::sanitize_text(value)),
        value => value.clone(),
    }
}

pub(super) fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn discarded_path_key(key: &str) -> bool {
    matches!(
        normalize_key(key).as_str(),
        "path"
            | "filepath"
            | "file"
            | "filename"
            | "filenames"
            | "sourcepath"
            | "sourcefile"
            | "sourcefiles"
            | "directory"
            | "imagepath"
            | "imagefile"
            | "imagefiles"
            | "transcriptpath"
    )
}

pub(super) fn sensitive_key(key: &str) -> bool {
    let compact = normalize_key(key);
    [
        "token",
        "secret",
        "password",
        "credential",
        "apikey",
        "authorization",
        "cookie",
        "embedding",
        "vector",
        "privatekey",
        "accesskey",
        "refreshkey",
        "sessionkey",
        "updatetoken",
        "key",
    ]
    .iter()
    .any(|forbidden| {
        if *forbidden == "key" {
            compact == *forbidden || compact.ends_with(*forbidden)
        } else {
            compact.contains(*forbidden)
        }
    })
}

/// Keep observation JSON in the observation-v1 shape while replacing source paths with
/// archive-relative paths and dropping transcript paths that point outside the archive.
pub(super) fn project_observation(
    observation: &ObservationRecord,
    frame_archive_paths: &BTreeMap<String, String>,
) -> Value {
    let mut value = serde_json::to_value(observation).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        if let Some(paths) = object
            .get_mut("sourceFramePaths")
            .and_then(Value::as_object_mut)
        {
            paths.retain(|frame_id, path| {
                let Some(archive_path) = frame_archive_paths.get(frame_id) else {
                    return false;
                };
                *path = Value::String(archive_path.clone());
                true
            });
        }
    }
    remove_transcript_paths(&mut value);
    sanitize_json(&value)
}

fn remove_transcript_paths(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("transcriptPath");
            for value in object.values_mut() {
                remove_transcript_paths(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_transcript_paths(value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

pub(super) fn validate_materials(
    materials: &BTreeMap<String, Vec<u8>>,
) -> Result<(), PersistenceError> {
    for (path, bytes) in materials {
        if !allowed_material_path(path) {
            return Err(PersistenceError::Invalid(format!(
                "退避 material の allowlist 外パスです: {path}"
            )));
        }
        if path_has_sensitive_name(path) {
            return Err(PersistenceError::Invalid(
                "退避 material のファイル名に秘密語があります".to_owned(),
            ));
        }
        if path.ends_with(".json") {
            let value = serde_json::from_slice::<Value>(bytes).map_err(|error| {
                PersistenceError::Invalid(format!("退避 JSON が不正です: {path}: {error}"))
            })?;
            if contains_forbidden_json(&value) {
                return Err(PersistenceError::Invalid(format!(
                    "退避 JSON に禁止フィールドまたはパスがあります: {path}"
                )));
            }
        } else if path.ends_with(".txt") {
            let text = String::from_utf8(bytes.clone()).map_err(|error| {
                PersistenceError::Invalid(format!(
                    "退避テキストが UTF-8 ではありません: {path}: {error}"
                ))
            })?;
            if contains_forbidden_text(&text) {
                return Err(PersistenceError::Invalid(format!(
                    "退避テキストに秘密または外部パスがあります: {path}"
                )));
            }
        }
    }
    Ok(())
}

fn allowed_material_path(path: &str) -> bool {
    if matches!(path, "manifest.json" | "utterance.json") {
        return true;
    }
    let Some((directory, name)) = path.split_once('/') else {
        return false;
    };
    if name.is_empty()
        || name.contains('/')
        || name == "."
        || name == ".."
        || directory.contains('/')
        || directory.is_empty()
    {
        return false;
    }
    match directory {
        "observations" => name.ends_with(".json") && !name.is_empty(),
        "audio-segments" => name.ends_with(".json") && !name.is_empty(),
        "transcripts" => name.ends_with(".txt") && !name.is_empty(),
        "frames" => name.ends_with(".png") && !name.is_empty(),
        "prompts" => name.ends_with(".txt") && !name.is_empty(),
        "responses" => name.ends_with(".json") && !name.is_empty(),
        "judge" => {
            name == "request.json"
                || name == "trace.json"
                || (name
                    .strip_prefix("module-")
                    .and_then(|value| value.strip_suffix(".json"))
                    .is_some_and(|value| {
                        !value.is_empty() && value.chars().all(|c| c.is_ascii_digit())
                    }))
        }
        _ => false,
    }
}

fn path_has_sensitive_name(path: &str) -> bool {
    path.split('/').any(|segment| {
        let stem = segment.rsplit_once('.').map_or(segment, |(stem, _)| stem);
        !stem.is_empty() && sensitive_key(stem)
    })
}

/// 公開前の最終検証。退避先の全ファイル（本文・ファイル名・manifest）と JSONL レコードに
/// 秘密語・秘密値・絶対パス・親ディレクトリ参照が残っていないことを決定的に確認する。
/// 失敗カテゴリだけを返し、秘密値を含む生の内容はエラーに載せない。
pub(super) fn validate_publication(
    record: &super::UtteranceFeedbackRecord,
    materials: &BTreeMap<String, Vec<u8>>,
) -> Result<(), &'static str> {
    validate_materials(materials).map_err(|_| "materials")?;
    let value = serde_json::to_value(record).map_err(|_| "record-serialize")?;
    if json_contains_forbidden_value(&value) {
        return Err("record");
    }
    Ok(())
}

pub(super) fn record_is_forbidden(record: &super::UtteranceFeedbackRecord) -> bool {
    serde_json::to_value(record)
        .map(|value| json_contains_forbidden_value(&value))
        .unwrap_or(true)
}

/// JSONL レコードの検証用。レコードのスキーマキーは固定であり、archive.directory や
/// transcriptPath のような退避相対パスを保持するフィールドが正当に存在するため、
/// キーではなく文字列値だけを検査する。
fn json_contains_forbidden_value(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.values().any(json_contains_forbidden_value),
        Value::Array(values) => values.iter().any(json_contains_forbidden_value),
        Value::String(value) => contains_forbidden_text(value),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn contains_forbidden_json(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            sensitive_key(key) || discarded_path_key(key) || contains_forbidden_json(value)
        }),
        Value::Array(values) => values.iter().any(contains_forbidden_json),
        Value::String(value) => contains_forbidden_text(value),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn contains_forbidden_text(value: &str) -> bool {
    contains_sensitive_assignment(value)
        || contains_unredacted_bearer(value)
        || ["sk-", "ghp_", "github_pat_", "xoxb-"]
            .iter()
            .any(|prefix| contains_unredacted_prefix(value, prefix))
        || contains_external_path(value)
}

/// camelCase・snake_case・hyphen 区切りを同一視するため、テキスト中の代入トークンを
/// 走査して正規化したキーで判定する。未知の合成表現（accessToken、embedding-vector 等）も
/// 安全側で拒否する。
fn contains_sensitive_assignment(value: &str) -> bool {
    let mut offset = 0;
    while let Some((key_start, key_end, separator)) = super::archive::next_assignment(value, offset)
    {
        if !sensitive_key(&value[key_start..key_end]) {
            offset = key_end;
            continue;
        }
        let (secret_start, end) = super::archive::assignment_value_range(value, separator);
        let candidate = value[secret_start..end].trim();
        if !candidate.is_empty()
            && !candidate.eq_ignore_ascii_case("null")
            && !candidate.eq_ignore_ascii_case("[REDACTED]")
        {
            return true;
        }
        offset = end.max(key_end.saturating_add(1));
    }
    false
}

fn contains_unredacted_bearer(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let mut offset = 0;
    while let Some(relative) = lower[offset..].find("bearer ") {
        let start = offset + relative;
        let boundary_before = start == 0
            || value[..start]
                .chars()
                .next_back()
                .is_some_and(|character| !character.is_ascii_alphanumeric() && character != '_');
        if boundary_before {
            let secret_start = start + "bearer ".len();
            let end = super::archive::text_value_end(value, secret_start);
            let candidate = value[secret_start..end].trim();
            if !candidate.is_empty() && !candidate.eq_ignore_ascii_case("[REDACTED]") {
                return true;
            }
        }
        offset = start + "bearer ".len();
    }
    false
}

fn contains_unredacted_prefix(value: &str, prefix: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let prefix = prefix.to_ascii_lowercase();
    let mut offset = 0;
    while let Some(relative) = lower[offset..].find(&prefix) {
        let start = offset + relative;
        let boundary_before = start == 0
            || value[..start]
                .chars()
                .next_back()
                .is_some_and(|character| !character.is_ascii_alphanumeric());
        if boundary_before && !value[start..].starts_with("[REDACTED]") {
            return true;
        }
        offset = start + prefix.len();
    }
    false
}

fn contains_external_path(value: &str) -> bool {
    value.char_indices().any(|(start, character)| {
        let boundary = start == 0
            || value[..start].chars().next_back().is_some_and(|previous| {
                previous.is_ascii_whitespace()
                    || matches!(previous, '=' | ':' | '"' | '\'' | '(' | '[' | '{')
            });
        let rest = &value[start..];
        let traversal = boundary && (rest.starts_with("../") || rest.starts_with("./"));
        let absolute = character == '/'
            && boundary
            && !rest.starts_with("//")
            && !value[..start].ends_with("http:")
            && !value[..start].ends_with("https:");
        let windows = character.is_ascii_alphabetic()
            && rest.as_bytes().get(1) == Some(&b':')
            && rest
                .as_bytes()
                .get(2)
                .is_some_and(|byte| *byte == b'\\' || *byte == b'/');
        traversal || absolute || windows
    })
}
