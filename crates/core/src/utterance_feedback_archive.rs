use crate::persistence::{
    set_private_directory_mode, set_private_file_mode, JsonlStore, PersistenceError, SiblingLock,
};
use crate::state::ObservationRecord;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

pub(super) use super::security::{project_observation, sanitize_json, validate_materials};

pub(super) fn safe_file_under(root: &Path, path: &str) -> Option<PathBuf> {
    let lexical_root = if root.is_absolute() {
        root.to_owned()
    } else {
        std::env::current_dir().ok()?.join(root)
    };
    let canonical_root = fs::canonicalize(&lexical_root).ok()?;
    let raw_candidate = PathBuf::from(path);
    let candidate = if raw_candidate.is_absolute() {
        raw_candidate
    } else {
        lexical_root.join(raw_candidate)
    };
    let relative = candidate.strip_prefix(&lexical_root).ok()?;
    let mut cursor = lexical_root;
    for component in relative.components() {
        if matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return None;
        }
        cursor.push(component.as_os_str());
        if fs::symlink_metadata(&cursor).ok()?.file_type().is_symlink() {
            return None;
        }
    }
    let metadata = fs::symlink_metadata(&candidate).ok()?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let candidate = fs::canonicalize(candidate).ok()?;
    candidate.starts_with(&canonical_root).then_some(candidate)
}

pub(super) fn sanitize_text(value: &str) -> String {
    let mut value = value.to_owned();
    redact_json_fields(&mut value);
    redact_sensitive_assignments(&mut value);
    redact_bearer(&mut value);
    for prefix in ["sk-", "ghp_", "github_pat_", "xoxb-"] {
        redact_prefix(&mut value, prefix);
    }
    redact_path_like_values(&mut value);
    value
}

/// テキスト中の `key: value` / `key = value` 代入を走査する。キーは
/// ASCII 英数字・`_`・`-` の最大連続で取り、camelCase・snake_case・hyphen 区切りを
/// 同一視できる形で呼び出し側へ渡す。戻り値は (key_start, key_end, separator)。
pub(super) fn next_assignment(value: &str, mut offset: usize) -> Option<(usize, usize, usize)> {
    while offset < value.len() {
        let relative = value[offset..].find(|character: char| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
        })?;
        let key_start = offset + relative;
        let key_end = value[key_start..]
            .find(|character: char| {
                !(character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
            })
            .map_or(value.len(), |relative| key_start + relative);
        let mut separator = key_end;
        while value
            .as_bytes()
            .get(separator)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            separator += 1;
        }
        if matches!(value.as_bytes().get(separator), Some(b':' | b'=')) {
            return Some((key_start, key_end, separator));
        }
        offset = key_end;
    }
    None
}

/// 代入の値域を返す。先頭の空白と `Bearer ` 接頭辞を飛ばし、文字列・配列・オブジェクトの
/// ネストを考慮して値の終端を求める。
pub(super) fn assignment_value_range(value: &str, separator: usize) -> (usize, usize) {
    let mut secret_start = separator + 1;
    while value
        .as_bytes()
        .get(secret_start)
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        secret_start += 1;
    }
    if value[secret_start..]
        .to_ascii_lowercase()
        .starts_with("bearer ")
    {
        secret_start += "bearer ".len();
    }
    (secret_start, text_value_end(value, secret_start))
}

fn redact_sensitive_assignments(value: &mut String) {
    let mut offset = 0;
    while let Some((key_start, key_end, separator)) = next_assignment(value, offset) {
        if !super::security::sensitive_key(&value[key_start..key_end]) {
            offset = key_end;
            continue;
        }
        let (secret_start, end) = assignment_value_range(value, separator);
        let candidate = value[secret_start..end].trim();
        if candidate.is_empty()
            || candidate.eq_ignore_ascii_case("null")
            || candidate.eq_ignore_ascii_case("[REDACTED]")
        {
            offset = end.max(key_end.saturating_add(1));
            continue;
        }
        value.replace_range(secret_start..end, "[REDACTED]");
        offset = secret_start + "[REDACTED]".len();
    }
}

fn redact_json_fields(value: &mut String) {
    let mut offset = 0;
    while offset < value.len() {
        let Some(relative) = value[offset..].find('"') else {
            return;
        };
        let key_start = offset + relative;
        let Some(key_end) = quoted_string_end(value, key_start) else {
            return;
        };
        let key = serde_json::from_str::<String>(&value[key_start..=key_end]).ok();
        let mut value_start = key_end + 1;
        while value
            .as_bytes()
            .get(value_start)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            value_start += 1;
        }
        if key
            .as_deref()
            .is_none_or(|key| !super::security::sensitive_key(key))
            || value.as_bytes().get(value_start) != Some(&b':')
        {
            offset = key_end + 1;
            continue;
        }
        value_start += 1;
        while value
            .as_bytes()
            .get(value_start)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            value_start += 1;
        }
        let Some(value_end) = json_value_end(value, value_start) else {
            return;
        };
        value.replace_range(value_start..value_end, "null");
        offset = value_start + 4;
    }
}

fn quoted_string_end(value: &str, start: usize) -> Option<usize> {
    let bytes = value.as_bytes();
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(start + 1) {
        match (*byte, escaped) {
            (b'"', false) => return Some(index),
            (b'\\', false) => escaped = true,
            (_, _) => escaped = false,
        }
    }
    None
}

fn json_value_end(value: &str, start: usize) -> Option<usize> {
    let bytes = value.as_bytes();
    let first = *bytes.get(start)?;
    if first == b'"' {
        return quoted_string_end(value, start).map(|index| index + 1);
    }
    if matches!(first, b'[' | b'{') {
        let mut stack = Vec::new();
        let mut escaped = false;
        let mut in_string = false;
        for (index, byte) in bytes.iter().enumerate().skip(start) {
            let byte = *byte;
            if in_string {
                match (byte, escaped) {
                    (b'"', false) => in_string = false,
                    (b'\\', false) => escaped = true,
                    (_, _) => escaped = false,
                }
                continue;
            }
            match byte {
                b'"' => in_string = true,
                b'[' | b'{' => stack.push(byte),
                b']' if stack.last() == Some(&b'[') => {
                    stack.pop();
                }
                b'}' if stack.last() == Some(&b'{') => {
                    stack.pop();
                }
                _ => {}
            }
            if stack.is_empty() {
                return Some(index + 1);
            }
        }
        return None;
    }
    let end = bytes[start..]
        .iter()
        .position(|byte| matches!(byte, b',' | b'}' | b']' | b'\n' | b'\r'))
        .map_or(bytes.len(), |offset| start + offset);
    Some(end)
}

fn redact_bearer(value: &mut String) {
    redact_assignment_like_prefix(value, "bearer ");
}

fn redact_assignment_like_prefix(value: &mut String, prefix: &str) {
    let prefix_lower = prefix.to_ascii_lowercase();
    let mut offset = 0;
    while offset < value.len() {
        let Some(relative) = value[offset..].to_ascii_lowercase().find(&prefix_lower) else {
            break;
        };
        let start = offset + relative;
        let boundary_before = start == 0
            || value[..start]
                .chars()
                .next_back()
                .is_some_and(|character| !character.is_ascii_alphanumeric() && character != '_');
        if !boundary_before {
            offset = start + prefix.len();
            continue;
        }
        let secret_start = start + prefix.len();
        let end = text_value_end(value, secret_start);
        if secret_start < end {
            value.replace_range(secret_start..end, "[REDACTED]");
            offset = secret_start + "[REDACTED]".len();
        } else {
            offset = secret_start;
        }
    }
}

fn redact_path_like_values(value: &mut String) {
    let mut offset = 0;
    while offset < value.len() {
        let Some((start, length)) =
            value[offset..]
                .char_indices()
                .find_map(|(relative, character)| {
                    let start = offset + relative;
                    let boundary = start == 0
                        || value[..start].chars().next_back().is_some_and(|previous| {
                            previous.is_ascii_whitespace()
                                || matches!(previous, '=' | ':' | '"' | '\'' | '(' | '[' | '{')
                        });
                    let rest = &value[start..];
                    let traversal = (rest.starts_with("../") || rest.starts_with("./")) && boundary;
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
                    (traversal || absolute || windows).then_some((relative, rest.len()))
                })
        else {
            break;
        };
        let start = offset + start;
        let end = value[start..]
            .char_indices()
            .find(|(_, character)| {
                character.is_ascii_whitespace() || ",;)]}\"'".contains(*character)
            })
            .map_or(value.len(), |(relative, _)| start + relative);
        if start < end {
            value.replace_range(start..end, "[PATH_REDACTED]");
            offset = start + "[PATH_REDACTED]".len();
        } else {
            offset = start.saturating_add(length.max(1));
        }
    }
}

pub(super) fn text_value_end(value: &str, start: usize) -> usize {
    match value.as_bytes().get(start) {
        Some(b'"') => {
            quoted_string_end(value, start).map_or(value.len(), |index| index.saturating_add(1))
        }
        Some(b'[' | b'{') => json_value_end(value, start).unwrap_or(value.len()),
        Some(_) => value[start..]
            .char_indices()
            .find(|(_, character)| character.is_whitespace() || ",;)]}".contains(*character))
            .map_or(value.len(), |(index, _)| start + index),
        None => value.len(),
    }
}

fn redact_prefix(value: &mut String, prefix: &str) {
    let prefix = prefix.to_ascii_lowercase();
    let mut offset = 0;
    while offset < value.len() {
        let lower = value.to_ascii_lowercase();
        let Some(relative) = lower[offset..].find(&prefix) else {
            break;
        };
        let start = offset + relative;
        let is_boundary = start == 0
            || value[..start]
                .chars()
                .next_back()
                .is_some_and(|character| !character.is_ascii_alphanumeric());
        if !is_boundary {
            offset = start + prefix.len();
            continue;
        }
        let end = value[start..]
            .char_indices()
            .find(|(_, character)| character.is_whitespace() || ",;)]}".contains(*character))
            .map_or(value.len(), |(index, _)| index + start);
        value.replace_range(start..end, "[REDACTED]");
        offset = start + "[REDACTED]".len();
    }
}

pub(super) fn load_observations(
    directory: &Path,
    requested: &HashSet<String>,
) -> (Vec<ObservationRecord>, Vec<String>) {
    if requested.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut records = HashMap::new();
    let mut issues = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return (Vec::new(), Vec::new()),
        Err(error) => return (Vec::new(), vec![format!("observation-read-failed:{error}")]),
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(path) = safe_file_under(directory, &path.to_string_lossy()) else {
            continue;
        };
        let values = match JsonlStore::new(path).read::<Value>() {
            Ok(values) => values,
            Err(error) => {
                issues.push(format!("observation-read-failed:{error}"));
                continue;
            }
        };
        for value in values {
            let Some(id) = value.get("id").and_then(Value::as_str) else {
                continue;
            };
            if !requested.contains(id) {
                continue;
            }
            let record_id = id.to_owned();
            match serde_json::from_value::<ObservationRecord>(value) {
                Ok(record) => {
                    records.insert(record_id, record);
                }
                Err(error) => issues.push(format!("observation-invalid:{record_id}:{error}")),
            }
        }
    }
    (records.into_values().collect(), issues)
}

pub(super) fn load_transcript(
    transcript_directory: &Path,
    transcript_path: Option<&str>,
    observation_id: &str,
    issues: &mut Vec<String>,
) -> Option<String> {
    let paths = match transcript_path {
        Some(path) => safe_file_under(transcript_directory, path)
            .into_iter()
            .collect::<Vec<_>>(),
        None => match fs::read_dir(transcript_directory) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    let path = entry.path();
                    safe_file_under(transcript_directory, &path.to_string_lossy())
                })
                .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jsonl"))
                .collect::<Vec<_>>(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                issues.push(format!("transcript-read-failed:{observation_id}:{error}"));
                return None;
            }
        },
    };
    for path in paths {
        let records = match JsonlStore::new(path).read::<Value>() {
            Ok(records) => records,
            Err(error) => {
                issues.push(format!("transcript-read-failed:{observation_id}:{error}"));
                continue;
            }
        };
        // 期間行を持つ観察は、期間行を audioStartMs 順に連結した全文を正本とする。
        let mut matches = records
            .into_iter()
            .filter_map(|record| {
                if record.get("observationId").and_then(Value::as_str) != Some(observation_id) {
                    return None;
                }
                let text = record.get("text").and_then(Value::as_str)?.to_owned();
                let start = record.get("audioStartMs").and_then(Value::as_u64);
                Some((start.is_none(), start, text))
            })
            .collect::<Vec<_>>();
        if matches.is_empty() {
            continue;
        }
        matches.sort_by_key(|(legacy, start, _)| (*legacy, *start));
        return Some(matches.into_iter().map(|(_, _, text)| text).collect());
    }
    if transcript_path.is_some() {
        issues.push(format!("transcript-not-found:{observation_id}"));
    } else {
        issues.push(format!("transcript-unavailable:{observation_id}"));
    }
    None
}

pub(super) fn collect_debug_materials(
    context: &mut super::MaterialContext,
    debug_directory: &Path,
    observer_frame_files: &HashMap<String, HashSet<String>>,
    companion_call_id: Option<&str>,
) {
    if observer_frame_files.is_empty() && companion_call_id.is_none() {
        context
            .issues
            .push("llm-material-unavailable:call-unknown".to_owned());
        return;
    }
    let directories = match fs::read_dir(debug_directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            for observation_id in observer_frame_files.keys() {
                context.issues.push(format!(
                    "llm-material-unavailable:observer:{observation_id}:debug-store-disabled"
                ));
            }
            if companion_call_id.is_some() {
                context
                    .issues
                    .push("llm-material-unavailable:companion:debug-store-disabled".to_owned());
            } else {
                context
                    .issues
                    .push("llm-material-unavailable:companion:call-unknown".to_owned());
            }
            return;
        }
        Err(error) => {
            context.issues.push(format!("debug-read-failed:{error}"));
            return;
        }
    };
    let directories = directories
        .filter_map(Result::ok)
        .filter_map(|directory| {
            directory
                .file_type()
                .ok()
                .filter(|file_type| file_type.is_dir() && !file_type.is_symlink())
                .map(|_| directory.path())
        })
        .collect::<Vec<_>>();
    let mut observer_found = HashSet::new();
    let mut companion_found = false;
    for directory in &directories {
        let Ok(files) = fs::read_dir(directory) else {
            context
                .issues
                .push("debug-directory-read-failed".to_owned());
            continue;
        };
        for file in files.filter_map(Result::ok) {
            let path = file.path();
            let Ok(file_type) = file.file_type() else {
                continue;
            };
            if !file_type.is_file()
                || file_type.is_symlink()
                || path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let Some(path) = safe_file_under(directory, &path.to_string_lossy()) else {
                continue;
            };
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    context
                        .issues
                        .push(format!("debug-file-read-failed:{error}"));
                    continue;
                }
            };
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                context.issues.push(format!("debug-file-invalid:{name}"));
                continue;
            };
            if name.starts_with("observer-") && !name.starts_with("observer-response-") {
                let Some(observation_id) = value.get("observationId").and_then(Value::as_str)
                else {
                    continue;
                };
                let Some(expected_frame_files) = observer_frame_files.get(observation_id) else {
                    continue;
                };
                observer_found.insert(observation_id.to_owned());
                if let Some(prompt) = value
                    .get("prompt")
                    .and_then(Value::as_str)
                    .filter(|prompt| !prompt.trim().is_empty())
                {
                    context.add_bytes(
                        &format!("prompts/observer-{}.txt", super::digest(observation_id)),
                        sanitize_text(prompt).into_bytes(),
                    );
                } else {
                    context.issues.push(format!("debug-prompt-missing:{name}"));
                }
                if let Some(response) = value.get("response").filter(|response| !response.is_null())
                {
                    context.add_json(
                        &format!("responses/observer-{}.json", super::digest(observation_id)),
                        response,
                    );
                } else {
                    context
                        .issues
                        .push(format!("debug-response-missing:{name}"));
                }
                if let Some(image_files) = value.get("imageFiles").and_then(Value::as_array) {
                    for image_file in image_files.iter().filter_map(Value::as_str) {
                        let image_path = Path::new(image_file);
                        let valid_name = !image_path.is_absolute()
                            && image_path
                                .extension()
                                .and_then(|value| value.to_str())
                                .is_some_and(|extension| {
                                    matches!(extension, "png" | "jpg" | "jpeg" | "webp")
                                })
                            && image_path.components().all(|component| {
                                !matches!(component, std::path::Component::ParentDir)
                            })
                            && expected_frame_files.contains(image_file);
                        let Some(source) = valid_name
                            .then(|| safe_file_under(directory, image_file))
                            .flatten()
                        else {
                            context.issues.push(format!(
                                "debug-frame-path-rejected:{}",
                                super::digest(image_file)
                            ));
                            continue;
                        };
                        match fs::read(source) {
                            Ok(bytes) => context.add_bytes(
                                &format!("frames/debug-{}.png", super::digest(image_file)),
                                bytes,
                            ),
                            Err(error) => context.issues.push(format!(
                                "debug-frame-read-failed:{}:{error}",
                                super::digest(image_file)
                            )),
                        }
                    }
                }
            } else if name.starts_with("companion-") && !name.starts_with("companion-response-") {
                let Some(expected_call_id) = companion_call_id else {
                    continue;
                };
                let Some(call_id) = value.get("callId").and_then(Value::as_str) else {
                    continue;
                };
                if call_id != expected_call_id {
                    continue;
                }
                companion_found = true;
                let call_id = super::digest(call_id);
                if let Some(prompt) = value
                    .get("prompt")
                    .and_then(Value::as_str)
                    .filter(|prompt| !prompt.trim().is_empty())
                {
                    context.add_bytes(
                        &format!("prompts/companion-{call_id}.txt"),
                        sanitize_text(prompt).into_bytes(),
                    );
                } else {
                    context.issues.push(format!("debug-prompt-missing:{name}"));
                }
                if let Some(response) = value.get("response").filter(|response| !response.is_null())
                {
                    context.add_json(&format!("responses/companion-{call_id}.json"), response);
                } else {
                    context
                        .issues
                        .push(format!("debug-response-missing:{name}"));
                }
            }
        }
    }
    for observation_id in observer_frame_files.keys() {
        if !observer_found.contains(observation_id) {
            context.issues.push(format!(
                "llm-material-unavailable:observer:{observation_id}:retention-expired"
            ));
        }
    }
    if companion_call_id.is_some() {
        if !companion_found {
            context
                .issues
                .push("llm-material-unavailable:companion:retention-expired".to_owned());
        }
    } else {
        context
            .issues
            .push("llm-material-unavailable:companion:call-unknown".to_owned());
    }
}

pub(super) fn cleanup_stale_archive_temps(
    directory: &Path,
    feedback_path: &Path,
) -> io::Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    let _lock = SiblingLock::acquire(&directory.join(".utterance-feedback.archive.lock"))
        .map_err(|error| io::Error::other(error.to_string()))?;
    let referenced = JsonlStore::new(feedback_path.to_owned())
        .read::<super::UtteranceFeedbackRecord>()
        .map_err(|error| io::Error::other(error.to_string()))?
        .into_iter()
        .map(|record| record.record_id)
        .collect::<HashSet<_>>();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        if file_type.is_dir() {
            let is_temp = name
                .to_str()
                .is_some_and(|name| name.starts_with('.') && name.ends_with(".tmp"));
            let is_unreferenced_archive = name.to_str().is_some_and(|name| {
                Uuid::parse_str(name).is_ok()
                    && !referenced.contains(name)
                    && committed_manifest(&entry.path(), name)
            });
            if is_temp || is_unreferenced_archive {
                fs::remove_dir_all(entry.path())?;
            }
        }
    }
    Ok(())
}

fn committed_manifest(directory: &Path, record_id: &str) -> bool {
    let Ok(bytes) = fs::read(directory.join("manifest.json")) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    manifest.get("commitMarker").and_then(Value::as_str) == Some("complete")
        && manifest.get("recordId").and_then(Value::as_str) == Some(record_id)
}

pub(super) fn publish_archive(
    archive_directory: &Path,
    materials: BTreeMap<String, Vec<u8>>,
) -> Result<(), PersistenceError> {
    validate_materials(&materials)?;
    let parent = archive_directory
        .parent()
        .ok_or_else(|| PersistenceError::Invalid("退避ディレクトリの親がありません".to_owned()))?;
    fs::create_dir_all(parent)?;
    set_private_directory_mode(parent)?;
    let _lock = SiblingLock::acquire(&parent.join(".utterance-feedback.archive.lock"))?;
    if archive_directory.exists() {
        return Err(PersistenceError::Invalid(
            "同じ record_id の退避ディレクトリが既にあります".to_owned(),
        ));
    }
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        archive_directory
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("utterance-feedback"),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&temporary)?;
    set_private_directory_mode(&temporary)?;
    let result = (|| -> Result<(), PersistenceError> {
        for (relative, bytes) in materials
            .iter()
            .filter(|(path, _)| *path != "manifest.json")
        {
            write_staged_file(&temporary.join(relative), bytes)?;
        }
        if let Some((_, manifest)) = materials.iter().find(|(path, _)| *path == "manifest.json") {
            write_staged_file(&temporary.join("manifest.json"), manifest)?;
        } else {
            return Err(PersistenceError::Invalid(
                "退避 manifest がありません".to_owned(),
            ));
        }
        File::open(&temporary)?.sync_all()?;
        fs::rename(&temporary, archive_directory)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&temporary);
    }
    result
}

fn write_staged_file(path: &Path, bytes: &[u8]) -> Result<(), PersistenceError> {
    let parent = path
        .parent()
        .ok_or_else(|| PersistenceError::Invalid("退避先の親がありません".to_owned()))?;
    fs::create_dir_all(parent)?;
    set_private_directory_mode(parent)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    set_private_file_mode(&file)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
