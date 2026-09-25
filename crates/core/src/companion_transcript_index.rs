use super::CompanionStorage;
use crate::persistence::{JsonlStore, PersistenceError};
use chrono::{DateTime, FixedOffset, NaiveDate};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const INDEXED_TRANSCRIPT_DAYS: usize = 2;

#[derive(Deserialize)]
struct TranscriptTime {
    time: DateTime<FixedOffset>,
}

impl CompanionStorage {
    pub(crate) fn audio_log_index(&self) -> Result<Value, PersistenceError> {
        let directory = &self.transcript_directory;
        if !directory.is_absolute() {
            return Err(PersistenceError::Invalid(
                "音声ログのディレクトリは絶対パスでなければなりません".to_owned(),
            ));
        }
        let mut files = Vec::new();
        for path in recent_transcript_files(directory)? {
            // 本文はデシリアライズせず、参照範囲を選ぶための日時だけを集計する。
            let records = JsonlStore::new(path.clone()).read::<TranscriptTime>()?;
            files.push(json!({
                "transcriptPath": path_text(&path)?,
                "recordCount": records.len(),
                "firstTime": records.iter().map(|record| record.time).min(),
                "lastTime": records.iter().map(|record| record.time).max(),
            }));
        }
        Ok(json!({
            "directory": path_text(directory)?,
            "indexedFileLimit": INDEXED_TRANSCRIPT_DAYS,
            "files": files,
        }))
    }
}

fn path_text(path: &Path) -> Result<&str, PersistenceError> {
    path.to_str().ok_or_else(|| {
        PersistenceError::Invalid("音声ログの path が UTF-8 ではありません".to_owned())
    })
}

fn recent_transcript_files(directory: &Path) -> Result<Vec<PathBuf>, PersistenceError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("jsonl")
            && path
                .file_stem()
                .and_then(|value| value.to_str())
                .is_some_and(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok())
        {
            files.push(path);
        }
    }
    files.sort();
    Ok(files
        .into_iter()
        .rev()
        .take(INDEXED_TRANSCRIPT_DAYS)
        .collect())
}
