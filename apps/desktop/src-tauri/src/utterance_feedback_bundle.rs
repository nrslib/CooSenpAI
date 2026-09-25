use coosenpai_core::config::ConfigPaths;
use coosenpai_core::persistence::JsonlStore;
use coosenpai_core::utterance_feedback::{
    is_allowed_feedback_archive_path, sanitize_feedback_export, sanitize_feedback_export_text,
    UtteranceFeedbackRecord, UTTERANCE_FEEDBACK_ARCHIVE_DIRECTORY,
    UTTERANCE_FEEDBACK_SCHEMA_VERSION,
};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use tar::Builder;
use uuid::Uuid;

const BUNDLE_SCHEMA_VERSION: u8 = 1;
const BUNDLE_DIRECTORY: &str = "utterance-feedback-bundles";
const SOURCE_ID_FILE_NAME: &str = "utterance-feedback-source-id";

struct BundleFile {
    path: PathBuf,
    bytes: Vec<u8>,
}

pub(crate) fn export_feedback_bundle(paths: &ConfigPaths) -> Result<String, String> {
    let records = JsonlStore::new(paths.utterance_feedback.clone())
        .read::<UtteranceFeedbackRecord>()
        .map_err(|error| format!("評価ファイルを読み込めません: {error}"))?;
    let mut issues = Vec::new();
    let mut files = Vec::new();
    let mut archive_paths = Vec::new();
    let mut archived_record_ids = HashSet::new();
    let mut archive_directories = HashMap::new();
    for record in &records {
        if archived_record_ids.insert(record.record_id.clone()) {
            let archive_directory = archive_directory_for_record(record, &mut issues);
            archive_directories.insert(record.record_id.clone(), archive_directory.clone());
            collect_archive_files(
                paths,
                record,
                &archive_directory,
                &mut archive_paths,
                &mut issues,
            )?;
        }
    }

    let mut jsonl = Vec::new();
    let mut summaries = Vec::new();
    for record in &records {
        let value = serde_json::to_value(record)
            .map_err(|error| format!("評価 JSON の生成に失敗しました: {error}"))?;
        let archive_directory = archive_directories
            .get(&record.record_id)
            .cloned()
            .unwrap_or_else(|| archive_directory_for_record(record, &mut issues));
        let value = normalize_record_archive_directory(value, &archive_directory);
        let value = sanitize_feedback_export(&value);
        serde_json::to_writer(&mut jsonl, &value)
            .map_err(|error| format!("評価 JSONL の生成に失敗しました: {error}"))?;
        jsonl.push(b'\n');
        summaries.push(record_summary(record, &archive_directory));
    }
    files.push(BundleFile {
        path: PathBuf::from("utterance-feedback.jsonl"),
        bytes: jsonl,
    });
    files.extend(archive_paths);

    let bundle_id = Uuid::new_v4().to_string();
    let collection_id = format!("feedback-bundle:{bundle_id}");
    let source_id = persistent_source_id(paths)?;
    let mut manifest = json!({
        "bundleSchemaVersion": BUNDLE_SCHEMA_VERSION,
        "feedbackSchemaVersion": UTTERANCE_FEEDBACK_SCHEMA_VERSION,
        "bundleType": "coosenpai-utterance-feedback",
        "collectionId": collection_id,
        "generatedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "source": {
            "kind": "desktop-state",
            "id": source_id,
        },
        "recordCount": records.len(),
        "records": summaries,
        "files": Vec::<String>::new(),
        "issues": issues,
        "privacy": {
            "rawAudio": "omitted",
            "relativePathsOnly": true,
            "scrubbed": true,
        },
    });

    let mut manifest_files = vec!["manifest.json".to_owned()];
    manifest_files.extend(files.iter().map(|file| path_string(&file.path)));
    manifest["files"] = serde_json::to_value(manifest_files)
        .map_err(|error| format!("bundle manifest の生成に失敗しました: {error}"))?;
    let manifest_bytes = serde_json::to_vec_pretty(&sanitize_feedback_export(&manifest))
        .map_err(|error| format!("bundle manifest のシリアライズに失敗しました: {error}"))?;

    let bundle_directory = paths.state.join(BUNDLE_DIRECTORY);
    fs::create_dir_all(&bundle_directory)
        .map_err(|error| format!("bundle 保存先を作成できません: {error}"))?;
    let bundle_path = bundle_directory.join(format!("feedback-bundle-{bundle_id}.tar.gz"));
    let output =
        File::create(&bundle_path).map_err(|error| format!("bundle を作成できません: {error}"))?;
    let encoder = GzEncoder::new(output, Compression::default());
    let mut archive = Builder::new(encoder);
    append_bytes(&mut archive, Path::new("manifest.json"), &manifest_bytes)?;
    for file in &files {
        append_bytes(&mut archive, &file.path, &file.bytes)?;
    }
    let encoder = archive
        .into_inner()
        .map_err(|error| format!("bundle archive を閉じられません: {error}"))?;
    encoder
        .finish()
        .map_err(|error| format!("bundle compression を完了できません: {error}"))?;

    let relative = bundle_path
        .strip_prefix(&paths.root)
        .map(path_string)
        .unwrap_or_else(|_| format!("state/{BUNDLE_DIRECTORY}/feedback-bundle-{bundle_id}.tar.gz"));
    Ok(relative)
}

fn collect_archive_files(
    paths: &ConfigPaths,
    record: &UtteranceFeedbackRecord,
    archive_directory: &Path,
    files: &mut Vec<BundleFile>,
    issues: &mut Vec<String>,
) -> Result<(), String> {
    if !safe_component(&record.record_id) {
        issues.push(format!("archive-invalid-record-id:{}", record.record_id));
        return Ok(());
    }
    let root = paths.utterance_feedback_archive.join(&record.record_id);
    if !root.exists() {
        issues.push(format!("archive-missing:{}", record.record_id));
        return Ok(());
    }
    if fs::symlink_metadata(&root)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(true)
    {
        issues.push(format!("archive-symlink-omitted:{}", record.record_id));
        return Ok(());
    }
    let mut pending = vec![root.clone()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                issues.push(format!("archive-read-failed:{}:{error}", record.record_id));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    issues.push(format!(
                        "archive-entry-read-failed:{}:{error}",
                        record.record_id
                    ));
                    continue;
                }
            };
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    issues.push(format!(
                        "archive-type-read-failed:{}:{error}",
                        record.record_id
                    ));
                    continue;
                }
            };
            let path = entry.path();
            if file_type.is_symlink() {
                issues.push(format!("archive-symlink-omitted:{}", record.record_id));
                continue;
            }
            if file_type.is_dir() {
                pending.push(path);
                continue;
            }
            if !file_type.is_file() {
                issues.push(format!("archive-special-file-omitted:{}", record.record_id));
                continue;
            }
            let relative = match path.strip_prefix(&root) {
                Ok(relative) => relative,
                Err(_) => {
                    issues.push(format!("archive-path-invalid:{}", record.record_id));
                    continue;
                }
            };
            if relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            }) {
                issues.push(format!("archive-path-invalid:{}", record.record_id));
                continue;
            }
            if is_raw_audio(relative) {
                issues.push(format!("raw-audio-omitted:{}", record.record_id));
                continue;
            }
            let relative_string = path_string(relative);
            if !is_allowed_feedback_archive_path(&relative_string) {
                issues.push(format!("archive-file-omitted:{}", record.record_id));
                continue;
            }
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    issues.push(format!(
                        "archive-material-read-failed:{}:{error}",
                        record.record_id
                    ));
                    continue;
                }
            };
            let bytes = if is_text_material(relative) {
                String::from_utf8(bytes)
                    .map(|value| sanitize_feedback_export_text(&value).into_bytes())
                    .unwrap_or_else(|error| error.into_bytes())
            } else {
                bytes
            };
            files.push(BundleFile {
                path: archive_directory.join(relative),
                bytes,
            });
        }
    }
    Ok(())
}

fn record_summary(record: &UtteranceFeedbackRecord, archive_directory: &Path) -> Value {
    let mut summary = json!({
        "recordId": record.record_id,
        "revision": record.revision,
        "cancelled": record.cancelled,
        "sign": record.sign,
        "reasonCode": record.reason_code,
        "trigger": record.trigger,
        "utterance": {
            "id": record.utterance.id,
            "createdAt": record.utterance.created_at,
            "message": record.utterance.message,
            "messageKind": record.utterance.message_kind,
        },
        "observationIds": record.observation_ids,
        "feed": record.feed,
        "archive": record.archive,
    });
    summary["archive"]["directory"] = json!(path_string(archive_directory));
    summary
}

fn append_bytes(
    archive: &mut Builder<GzEncoder<File>>,
    path: &Path,
    bytes: &[u8],
) -> Result<(), String> {
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("bundle 内の相対パスが不正です: {}", path.display()));
    }
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_mtime(0);
    header.set_cksum();
    archive
        .append_data(&mut header, path, bytes)
        .map_err(|error| format!("bundle entry を追加できません: {error}"))
}

fn safe_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn is_raw_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "wav" | "mp3" | "m4a" | "flac" | "aiff" | "caf" | "ogg" | "opus"
            )
        })
}

fn is_text_material(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "json" | "txt"))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn archive_directory_for_record(
    record: &UtteranceFeedbackRecord,
    issues: &mut Vec<String>,
) -> PathBuf {
    let existing = PathBuf::from(&record.archive.directory);
    if is_safe_relative_path(&existing) {
        return existing;
    }
    issues.push(format!("archive-directory-invalid:{}", record.record_id));
    if safe_component(&record.record_id) {
        PathBuf::from(UTTERANCE_FEEDBACK_ARCHIVE_DIRECTORY).join(&record.record_id)
    } else {
        PathBuf::from(UTTERANCE_FEEDBACK_ARCHIVE_DIRECTORY).join("invalid-record")
    }
}

fn normalize_record_archive_directory(mut value: Value, archive_directory: &Path) -> Value {
    if let Some(directory) = value.get_mut("archive").and_then(Value::as_object_mut) {
        directory.insert(
            "directory".to_owned(),
            Value::String(path_string(archive_directory)),
        );
    }
    value
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn persistent_source_id(paths: &ConfigPaths) -> Result<String, String> {
    fs::create_dir_all(&paths.state)
        .map_err(|error| format!("収集元IDの保存先を作成できません: {error}"))?;
    let path = paths.state.join(SOURCE_ID_FILE_NAME);
    match fs::read_to_string(&path) {
        Ok(value) => return parse_source_id(&value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("収集元IDを読み込めません: {error}")),
    }

    let generated = Uuid::new_v4().to_string();
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(generated.as_bytes())
                .and_then(|_| file.sync_data())
                .map_err(|error| format!("収集元IDを保存できません: {error}"))?;
            Ok(generated)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let value = fs::read_to_string(&path)
                .map_err(|read_error| format!("収集元IDを再読み込みできません: {read_error}"))?;
            parse_source_id(&value)
        }
        Err(error) => Err(format!("収集元IDを保存できません: {error}")),
    }
}

fn parse_source_id(value: &str) -> Result<String, String> {
    Uuid::parse_str(value.trim())
        .map(|id| id.to_string())
        .map_err(|_| "収集元IDの形式が不正です".to_owned())
}
