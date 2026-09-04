use crate::config::ConfigPaths;
use crate::persistence::{atomic_write_bytes, set_private_directory_mode};
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

const RETENTION_DAYS: i64 = 3;
const MAX_TOTAL_BYTES: u64 = 200 * 1_024 * 1_024;
const OCR_PREVIEW_CHARS: usize = 500;

#[derive(Debug, Error)]
pub enum DebugError {
    #[error("debug 記録の I/O に失敗しました: {0}")]
    Io(#[from] io::Error),
    #[error("debug 記録を JSON 化できません: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct DebugStore {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DebugGateRecord {
    pub id: String,
    pub created_at: String,
    pub trigger: String,
    pub sent: bool,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DebugDetail {
    pub source_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observer_response: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub companion_context: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DebugCatalog {
    pub details: Vec<DebugDetail>,
    pub latest_gate: Option<DebugGateRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ObserverCallRecord {
    kind: String,
    observation_id: String,
    frame_ids: Vec<String>,
    image_files: Vec<String>,
    ocr_preview: Option<String>,
    prompt: String,
    response: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompanionCallRecord {
    kind: String,
    call_id: String,
    source_ids: Vec<String>,
    prompt: String,
    response: Value,
}

pub struct ObserverDebugCall<'a> {
    pub call_id: &'a str,
    pub observation_id: &'a str,
    pub frame_ids: Vec<String>,
    pub image_files: Vec<String>,
    pub ocr_preview: Option<String>,
    pub prompt: &'a str,
    pub response: &'a Value,
    pub created_at: DateTime<Utc>,
}

impl DebugStore {
    pub fn from_paths(paths: &ConfigPaths) -> Self {
        Self {
            root: paths.debug.clone(),
        }
    }

    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }

    pub fn record_frame(
        &self,
        id: &str,
        created_at: DateTime<Utc>,
        provider_png: &[u8],
        ocr_text: Option<&str>,
    ) -> Result<(), DebugError> {
        let directory = self.day_directory(created_at)?;
        atomic_write_bytes(&directory.join(format!("frame-{id}.png")), provider_png)?;
        atomic_write_bytes(
            &directory.join(format!("ocr-{id}.txt")),
            ocr_text.unwrap_or_default().as_bytes(),
        )?;
        self.prune(created_at)
    }

    pub fn record_gate(&self, record: &DebugGateRecord) -> Result<(), DebugError> {
        let created_at = parse_timestamp(&record.created_at)?;
        let directory = self.day_directory(created_at)?;
        write_json(&directory.join(format!("gate-{}.json", record.id)), record)?;
        self.prune(created_at)
    }

    pub fn record_observer_call(&self, call: ObserverDebugCall<'_>) -> Result<(), DebugError> {
        let directory = self.day_directory(call.created_at)?;
        let record = ObserverCallRecord {
            kind: "observer".to_owned(),
            observation_id: call.observation_id.to_owned(),
            frame_ids: call.frame_ids,
            image_files: call.image_files,
            ocr_preview: call.ocr_preview,
            prompt: call.prompt.to_owned(),
            response: call.response.clone(),
        };
        atomic_write_bytes(
            &directory.join(format!("observer-prompt-{}.txt", call.call_id)),
            call.prompt.as_bytes(),
        )?;
        write_json(
            &directory.join(format!("observer-response-{}.json", call.call_id)),
            call.response,
        )?;
        write_json(
            &directory.join(format!("observer-{}.json", call.observation_id)),
            &record,
        )?;
        self.prune(call.created_at)
    }

    pub fn record_companion_call(
        &self,
        call_id: &str,
        source_ids: Vec<String>,
        prompt: &str,
        response: &Value,
        created_at: DateTime<Utc>,
    ) -> Result<(), DebugError> {
        let directory = self.day_directory(created_at)?;
        let record = CompanionCallRecord {
            kind: "companion".to_owned(),
            call_id: call_id.to_owned(),
            source_ids,
            prompt: prompt.to_owned(),
            response: response.clone(),
        };
        atomic_write_bytes(
            &directory.join(format!("companion-prompt-{call_id}.txt")),
            prompt.as_bytes(),
        )?;
        write_json(
            &directory.join(format!("companion-response-{call_id}.json")),
            response,
        )?;
        write_json(
            &directory.join(format!("companion-{call_id}.json")),
            &record,
        )?;
        self.prune(created_at)
    }

    pub fn record_prompt(
        &self,
        agent: &str,
        call_id: &str,
        system_prompt: &str,
        prompt: &str,
        created_at: DateTime<Utc>,
    ) -> Result<(), DebugError> {
        let directory = self.day_directory(created_at)?;
        let complete_prompt = format!("[system]\n{system_prompt}\n\n[user]\n{prompt}");
        atomic_write_bytes(
            &directory.join(format!("{agent}-prompt-{call_id}.txt")),
            complete_prompt.as_bytes(),
        )?;
        self.prune(created_at)
    }

    pub fn record_response(
        &self,
        agent: &str,
        call_id: &str,
        response: &Value,
        created_at: DateTime<Utc>,
    ) -> Result<(), DebugError> {
        let directory = self.day_directory(created_at)?;
        write_json(
            &directory.join(format!("{agent}-response-{call_id}.json")),
            response,
        )?;
        self.prune(created_at)
    }

    pub fn record_provider_error(
        &self,
        agent: &str,
        call_id: &str,
        error: &str,
        created_at: DateTime<Utc>,
    ) -> Result<(), DebugError> {
        let directory = self.day_directory(created_at)?;
        atomic_write_bytes(
            &directory.join(format!("{agent}-error-{call_id}.txt")),
            error.as_bytes(),
        )?;
        self.prune(created_at)
    }

    pub fn load_catalog(&self) -> Result<DebugCatalog, DebugError> {
        if !self.root.is_dir() {
            return Ok(DebugCatalog::default());
        }
        let mut observers = Vec::new();
        let mut companions = Vec::new();
        let mut gates = Vec::new();
        for path in json_files(&self.root)? {
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let bytes = fs::read(&path)?;
            if name.starts_with("observer-") && !name.starts_with("observer-response-") {
                if let Ok(record) = serde_json::from_slice::<ObserverCallRecord>(&bytes) {
                    observers.push(record);
                }
            } else if name.starts_with("companion-") && !name.starts_with("companion-response-") {
                if let Ok(record) = serde_json::from_slice::<CompanionCallRecord>(&bytes) {
                    companions.push(record);
                }
            } else if name.starts_with("gate-") {
                if let Ok(record) = serde_json::from_slice::<DebugGateRecord>(&bytes) {
                    gates.push(record);
                }
            }
        }
        let mut details = observers
            .into_iter()
            .map(|record| DebugDetail {
                source_ids: vec![record.observation_id],
                image_files: record.image_files,
                ocr_preview: record.ocr_preview,
                observer_response: Some(record.response),
                companion_context: None,
            })
            .collect::<Vec<_>>();
        for call in companions {
            for detail in &mut details {
                if detail
                    .source_ids
                    .iter()
                    .any(|id| call.source_ids.contains(id))
                {
                    detail.companion_context = Some(call.prompt.clone());
                }
            }
            details.push(DebugDetail {
                source_ids: call.source_ids,
                image_files: Vec::new(),
                ocr_preview: None,
                observer_response: None,
                companion_context: Some(call.prompt),
            });
        }
        gates.sort_by(|left, right| left.created_at.cmp(&right.created_at));
        Ok(DebugCatalog {
            details,
            latest_gate: gates.pop(),
        })
    }

    fn day_directory(&self, created_at: DateTime<Utc>) -> Result<PathBuf, DebugError> {
        let day = created_at.with_timezone(&Local).format("%Y-%m-%d");
        let directory = self.root.join(day.to_string());
        fs::create_dir_all(&directory)?;
        set_private_directory_mode(&self.root)?;
        set_private_directory_mode(&directory)?;
        Ok(directory)
    }

    fn prune(&self, now: DateTime<Utc>) -> Result<(), DebugError> {
        let cutoff = now.with_timezone(&Local).date_naive() - Duration::days(RETENTION_DAYS - 1);
        if self.root.is_dir() {
            for entry in fs::read_dir(&self.root)? {
                let entry = entry?;
                let path = entry.path();
                let date = entry
                    .file_name()
                    .to_str()
                    .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok());
                if entry.file_type()?.is_dir() && date.is_some_and(|value| value < cutoff) {
                    fs::remove_dir_all(path)?;
                }
            }
        }
        prune_to_size(&self.root, MAX_TOTAL_BYTES)?;
        Ok(())
    }
}

fn parse_timestamp(value: &str) -> Result<DateTime<Utc>, DebugError> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| DebugError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), DebugError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    atomic_write_bytes(path, &bytes)?;
    Ok(())
}

pub fn ocr_preview(value: Option<&str>) -> Option<String> {
    value.map(|text| text.chars().take(OCR_PREVIEW_CHARS).collect())
}

fn json_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for directory in fs::read_dir(root)? {
        let directory = directory?;
        if !directory.file_type()?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(directory.path())? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.path().extension().and_then(|value| value.to_str()) == Some("json")
            {
                files.push(entry.path());
            }
        }
    }
    Ok(files)
}

fn prune_to_size(root: &Path, maximum: u64) -> io::Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut files = Vec::new();
    for directory in fs::read_dir(root)? {
        let directory = directory?;
        if !directory.file_type()?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(directory.path())? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let metadata = entry.metadata()?;
                files.push((metadata.modified()?, metadata.len(), entry.path()));
            }
        }
    }
    files.sort_by_key(|(modified, _, path)| (*modified, path.clone()));
    let mut total = files.iter().map(|(_, bytes, _)| *bytes).sum::<u64>();
    for (_, bytes, path) in files {
        if total <= maximum {
            break;
        }
        fs::remove_file(path)?;
        total = total.saturating_sub(bytes);
    }
    Ok(())
}

