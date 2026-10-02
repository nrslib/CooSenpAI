use super::CompanionStorage;
use crate::persistence::{JsonlStore, PersistenceError};
use crate::speaker_id::PromptSpeakerIdResolver;
use crate::speaker_names::{
    load_speaker_name_index, speaker_name_record_for_prompt_id, SpeakerNamePromptContext,
    SpeakerNamePromptEntry, SpeakerNamePromptPreviousEntry,
};
use crate::state::SpeakerIdentificationStatus;
use chrono::{DateTime, FixedOffset, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const INDEXED_TRANSCRIPT_DAYS: usize = 2;
const SPEAKER_NAME_SCOPE: &str = "observation-sequence-and-indexed-transcripts";

pub(crate) struct CompanionAudioLogContext {
    pub(crate) index: Value,
    pub(crate) speaker_names: SpeakerNamePromptContext,
    pub(crate) speaker_id_resolver: PromptSpeakerIdResolver,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptIndexRecord {
    #[serde(default)]
    observation_id: String,
    time: DateTime<FixedOffset>,
    source: String,
    #[serde(default)]
    audio_start_ms: Option<u64>,
    #[serde(default)]
    audio_end_ms: Option<u64>,
    #[serde(default)]
    speaker_tag: Option<String>,
    #[serde(default)]
    speaker_registry_id: Option<String>,
    #[serde(default)]
    speaker_status: Option<SpeakerIdentificationStatus>,
}

#[derive(Default)]
struct SpeakerTranscriptRange {
    record_count: usize,
    first_time: Option<DateTime<FixedOffset>>,
    last_time: Option<DateTime<FixedOffset>>,
    references: Vec<TranscriptReference>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TranscriptReference {
    observation_id: String,
    time: DateTime<FixedOffset>,
    source: String,
    audio_start_ms: Option<u64>,
    audio_end_ms: Option<u64>,
    transcript_path: String,
}

impl SpeakerTranscriptRange {
    fn include(&mut self, time: DateTime<FixedOffset>) {
        self.record_count += 1;
        if self.first_time.is_none_or(|current| time < current) {
            self.first_time = Some(time);
        }
        if self.last_time.is_none_or(|current| time > current) {
            self.last_time = Some(time);
        }
    }
}

impl CompanionStorage {
    pub(crate) fn prompt_speaker_id_resolver(
        &self,
    ) -> Result<PromptSpeakerIdResolver, PersistenceError> {
        let paths = self.config_paths()?;
        let aliases = crate::observer::load_speaker_aliases(&paths)
            .map_err(|error| PersistenceError::Invalid(error.to_string()))?;
        Ok(PromptSpeakerIdResolver::new(
            aliases.registry_id,
            aliases.aliases,
        ))
    }

    pub(crate) fn audio_log_context(
        &self,
        observations: &[Value],
    ) -> Result<CompanionAudioLogContext, PersistenceError> {
        let directory = &self.transcript_directory;
        if !directory.is_absolute() {
            return Err(PersistenceError::Invalid(
                "音声ログのディレクトリは絶対パスでなければなりません".to_owned(),
            ));
        }
        let paths = self.config_paths()?;
        let aliases = crate::observer::load_speaker_aliases(&paths)
            .map_err(|error| PersistenceError::Invalid(error.to_string()))?;
        let resolver = PromptSpeakerIdResolver::new(aliases.registry_id, aliases.aliases);
        let names = load_speaker_name_index(&paths)?;
        let mut references = BTreeSet::new();
        let mut prompt_id_owners = BTreeMap::new();
        let mut files = Vec::new();

        for path in recent_transcript_files(directory)? {
            let records = JsonlStore::new(path.clone()).read::<TranscriptIndexRecord>()?;
            let transcript_path = path_text(&path)?.to_owned();
            let mut speakers = BTreeMap::<String, SpeakerTranscriptRange>::new();
            for record in &records {
                if record.speaker_status != Some(SpeakerIdentificationStatus::Identified) {
                    continue;
                }
                let (Some(registry_id), Some(speaker_id)) = (
                    record.speaker_registry_id.as_deref(),
                    record.speaker_tag.as_deref(),
                ) else {
                    continue;
                };
                let Some((canonical_id, prompt_id)) =
                    resolve_speaker_reference(registry_id, speaker_id, &resolver)
                else {
                    continue;
                };
                register_prompt_id_owner(
                    &mut prompt_id_owners,
                    &prompt_id,
                    registry_id,
                    &canonical_id,
                )?;
                references.insert((registry_id.to_owned(), canonical_id));
                let range = speakers.entry(prompt_id).or_default();
                range.include(record.time);
                if !record.observation_id.is_empty() {
                    range.references.push(TranscriptReference {
                        observation_id: record.observation_id.clone(),
                        time: record.time,
                        source: record.source.clone(),
                        audio_start_ms: record.audio_start_ms,
                        audio_end_ms: record.audio_end_ms,
                        transcript_path: transcript_path.clone(),
                    });
                }
            }
            for range in speakers.values_mut() {
                range.references.sort_by(|left, right| {
                    left.time
                        .cmp(&right.time)
                        .then_with(|| left.observation_id.cmp(&right.observation_id))
                        .then_with(|| left.audio_start_ms.cmp(&right.audio_start_ms))
                        .then_with(|| left.audio_end_ms.cmp(&right.audio_end_ms))
                });
            }
            files.push(json!({
                "transcriptPath": transcript_path,
                "recordCount": records.len(),
                "firstTime": records.iter().map(|record| record.time).min(),
                "lastTime": records.iter().map(|record| record.time).max(),
                "speakers": speakers.into_iter().map(|(speaker_id, range)| json!({
                    "speakerId": speaker_id,
                    "recordCount": range.record_count,
                    "firstTime": range.first_time,
                    "lastTime": range.last_time,
                    "references": range.references,
                })).collect::<Vec<_>>(),
            }));
        }

        for observation in observations {
            collect_observation_speakers(
                observation,
                &resolver,
                &mut references,
                &mut prompt_id_owners,
            )?;
        }

        let mut entries = Vec::new();
        let mut previous_names = Vec::new();
        for (registry_id, canonical_id) in references {
            let Some(resolved) = resolver.resolve(&registry_id, &canonical_id) else {
                continue;
            };
            let Some(record) = speaker_name_record_for_prompt_id(
                &names,
                Some(&registry_id),
                &canonical_id,
                &resolver,
            ) else {
                continue;
            };
            if let Some(name) = &record.current_name {
                entries.push(SpeakerNamePromptEntry {
                    speaker_id: resolved.prompt_id,
                    display_name: name.clone(),
                    source: "user-registration".to_owned(),
                });
            }
            if let Some(previous) = &record.previous_name {
                previous_names.push(SpeakerNamePromptPreviousEntry {
                    previous_name: previous.display_name.clone(),
                    current_name: record.current_name.clone(),
                    changed_at: previous.changed_at.clone(),
                });
            }
        }
        entries.sort_by(|left, right| left.speaker_id.cmp(&right.speaker_id));
        previous_names.sort_by(|left, right| {
            left.previous_name
                .cmp(&right.previous_name)
                .then_with(|| left.current_name.cmp(&right.current_name))
                .then_with(|| left.changed_at.cmp(&right.changed_at))
        });

        Ok(CompanionAudioLogContext {
            index: json!({
                "directory": path_text(directory)?,
                "indexedFileLimit": INDEXED_TRANSCRIPT_DAYS,
                "files": files,
            }),
            speaker_names: SpeakerNamePromptContext {
                scope: SPEAKER_NAME_SCOPE.to_owned(),
                retrieval_status: "available".to_owned(),
                entries,
                previous_names,
            },
            speaker_id_resolver: resolver,
        })
    }
}

fn collect_observation_speakers(
    observation: &Value,
    resolver: &PromptSpeakerIdResolver,
    references: &mut BTreeSet<(String, String)>,
    prompt_id_owners: &mut BTreeMap<String, (String, String)>,
) -> Result<(), PersistenceError> {
    if let Some(segments) = observation.get("audioSegments").and_then(Value::as_array) {
        for segment in segments {
            if segment.get("speakerStatus").and_then(Value::as_str) != Some("identified") {
                continue;
            }
            let (Some(registry_id), Some(speaker_id)) = (
                segment.get("speakerRegistryId").and_then(Value::as_str),
                segment
                    .get("speakerTag")
                    .or_else(|| segment.get("speakerId"))
                    .and_then(Value::as_str),
            ) else {
                continue;
            };
            add_speaker_reference(
                registry_id,
                speaker_id,
                resolver,
                references,
                prompt_id_owners,
            )?;
        }
    }

    if observation.get("kind").and_then(Value::as_str) == Some("audio")
        && observation.get("speakerStatus").and_then(Value::as_str) == Some("identified")
    {
        if let (Some(registry_id), Some(speaker_id)) = (
            observation.get("speakerRegistryId").and_then(Value::as_str),
            observation.get("speakerId").and_then(Value::as_str),
        ) {
            add_speaker_reference(
                registry_id,
                speaker_id,
                resolver,
                references,
                prompt_id_owners,
            )?;
        }
    }

    if let Some(segments) = observation.get("speakerSegments").and_then(Value::as_array) {
        for segment in segments {
            if segment.get("status").and_then(Value::as_str) != Some("identified") {
                continue;
            }
            let (Some(registry_id), Some(speaker_id)) = (
                segment.get("speakerRegistryId").and_then(Value::as_str),
                segment.get("speakerId").and_then(Value::as_str),
            ) else {
                continue;
            };
            add_speaker_reference(
                registry_id,
                speaker_id,
                resolver,
                references,
                prompt_id_owners,
            )?;
        }
    }
    Ok(())
}

fn add_speaker_reference(
    registry_id: &str,
    speaker_id: &str,
    resolver: &PromptSpeakerIdResolver,
    references: &mut BTreeSet<(String, String)>,
    prompt_id_owners: &mut BTreeMap<String, (String, String)>,
) -> Result<(), PersistenceError> {
    let Some((canonical_id, prompt_id)) =
        resolve_speaker_reference(registry_id, speaker_id, resolver)
    else {
        return Ok(());
    };
    register_prompt_id_owner(prompt_id_owners, &prompt_id, registry_id, &canonical_id)?;
    references.insert((registry_id.to_owned(), canonical_id));
    Ok(())
}

fn resolve_speaker_reference(
    registry_id: &str,
    speaker_id: &str,
    resolver: &PromptSpeakerIdResolver,
) -> Option<(String, String)> {
    let resolved = resolver.resolve(registry_id, speaker_id)?;
    Some((resolved.canonical_id, resolved.prompt_id))
}

fn register_prompt_id_owner(
    owners: &mut BTreeMap<String, (String, String)>,
    prompt_id: &str,
    registry_id: &str,
    canonical_id: &str,
) -> Result<(), PersistenceError> {
    let owner = (registry_id.to_owned(), canonical_id.to_owned());
    if owners
        .get(prompt_id)
        .is_some_and(|existing| existing != &owner)
    {
        return Err(PersistenceError::Invalid(
            "話者 ID の prompt namespace が衝突しています".to_owned(),
        ));
    }
    owners.insert(prompt_id.to_owned(), owner);
    Ok(())
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
