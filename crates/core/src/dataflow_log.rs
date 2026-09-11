use crate::config::ConfigPaths;
use crate::persistence::{JsonlStore, PersistenceError};
use crate::state::{
    parse_observation, ObservationRecord, TranscriptRecord, DEFAULT_OBSERVATION_LIMITS,
};
use serde::Serialize;
use std::path::{Path, PathBuf};

// details のデータフロータブは起動時に最新2日分だけを読み、あとは snapshot push で追従する。
const DATAFLOW_LOG_DAYS: usize = 2;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DataFlowLog {
    pub observations: Vec<ObservationRecord>,
    pub transcripts: Vec<TranscriptRecord>,
}

pub fn read_dataflow_log(
    paths: &ConfigPaths,
    limit: usize,
) -> Result<DataFlowLog, PersistenceError> {
    Ok(DataFlowLog {
        observations: read_observation_tail(&paths.observations, limit)?,
        transcripts: read_transcript_tail(&paths.transcripts, limit)?,
    })
}

fn recent_daily_files(directory: &Path) -> Result<Vec<PathBuf>, PersistenceError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let is_daily_jsonl = path
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|stem| chrono::NaiveDate::parse_from_str(stem, "%Y-%m-%d").is_ok())
            && path.extension().and_then(|value| value.to_str()) == Some("jsonl");
        if is_daily_jsonl {
            files.push(path);
        }
    }
    files.sort();
    Ok(files
        .into_iter()
        .rev()
        .take(DATAFLOW_LOG_DAYS)
        .rev()
        .collect())
}

fn read_observation_tail(
    directory: &Path,
    limit: usize,
) -> Result<Vec<ObservationRecord>, PersistenceError> {
    let mut observations = Vec::new();
    for path in recent_daily_files(directory)? {
        for value in JsonlStore::new(path).read::<serde_json::Value>()? {
            if let Ok(record) = parse_observation(value, DEFAULT_OBSERVATION_LIMITS) {
                observations.push(record);
            }
        }
    }
    if observations.len() > limit {
        observations.drain(..observations.len() - limit);
    }
    Ok(observations)
}

fn read_transcript_tail(
    directory: &Path,
    limit: usize,
) -> Result<Vec<TranscriptRecord>, PersistenceError> {
    let mut transcripts = Vec::new();
    for path in recent_daily_files(directory)? {
        let path_string = path.to_string_lossy().into_owned();
        transcripts.extend(
            JsonlStore::new(path)
                .read::<TranscriptRecord>()?
                .into_iter()
                .map(|mut record| {
                    record.transcript_path = Some(path_string.clone());
                    record
                }),
        );
    }
    if transcripts.len() > limit {
        transcripts.drain(..transcripts.len() - limit);
    }
    Ok(transcripts)
}

