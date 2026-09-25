use super::{archive, FeedbackAudioSegment};
use crate::frame_buffer::FrameBuffer;
use crate::judge::JudgeTrace;
use crate::state::ObservationRecord;
use chrono::Utc;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub(super) struct MaterialContext {
    pub(super) observations: Vec<ObservationRecord>,
    pub(super) audio_segments: Vec<FeedbackAudioSegment>,
    pub(super) observation_archive_paths: BTreeMap<String, String>,
    pub(super) frame_archive_paths: BTreeMap<String, String>,
    pub(super) materials: BTreeMap<String, Vec<u8>>,
    pub(super) issues: Vec<String>,
}

pub(super) struct MaterialCollectionInput<'a> {
    pub(super) observation_directory: &'a Path,
    pub(super) transcript_directory: &'a Path,
    pub(super) frame_directory: &'a Path,
    pub(super) debug_directory: &'a Path,
    pub(super) source_ids: &'a [String],
    pub(super) provided_observations: &'a [ObservationRecord],
    pub(super) judge_trace: Option<&'a JudgeTrace>,
    pub(super) judge_input_id: Option<&'a str>,
    pub(super) llm_call_id: Option<&'a str>,
}

impl MaterialContext {
    pub(super) fn collect(input: MaterialCollectionInput<'_>) -> Self {
        let MaterialCollectionInput {
            observation_directory,
            transcript_directory,
            frame_directory,
            debug_directory,
            source_ids,
            provided_observations,
            judge_trace,
            judge_input_id,
            llm_call_id,
        } = input;
        let mut context = Self::default();
        let requested = source_ids.iter().cloned().collect::<HashSet<_>>();
        let (loaded_observations, mut issues) =
            archive::load_observations(observation_directory, &requested);
        let mut observations = loaded_observations
            .into_iter()
            .filter(|observation| {
                !provided_observations
                    .iter()
                    .any(|provided| provided.id() == observation.id())
            })
            .collect::<Vec<_>>();
        for observation in provided_observations {
            if requested.contains(observation.id()) {
                observations.push(observation.clone());
            }
        }
        let segment_ids = observations
            .iter()
            .flat_map(|record| match record {
                ObservationRecord::Visual(value) => value
                    .audio_segments
                    .iter()
                    .map(|segment| segment.id.clone())
                    .collect::<Vec<_>>(),
                ObservationRecord::NoChange(_) | ObservationRecord::Audio(_) => Vec::new(),
            })
            .collect::<HashSet<_>>();
        let (audio_records, audio_issues) =
            archive::load_observations(observation_directory, &segment_ids);
        issues.extend(audio_issues);
        for record in audio_records {
            if !observations
                .iter()
                .any(|existing| existing.id() == record.id())
            {
                observations.push(record);
            }
        }
        observations.sort_by(|left, right| {
            left.created_at()
                .cmp(right.created_at())
                .then(left.id().cmp(right.id()))
        });
        for id in source_ids {
            if !observations.iter().any(|record| record.id() == id) {
                issues.push(format!("observation-not-found:{id}"));
            }
        }
        context.observations = observations;
        context.issues = issues;
        context.collect_observation_materials(transcript_directory, frame_directory);
        let observer_frame_files = context
            .observations
            .iter()
            .filter_map(|observation| match observation {
                ObservationRecord::Visual(value) => Some((
                    value.id.clone(),
                    value
                        .source_frame_ids
                        .iter()
                        .chain(value.source_frame_paths.keys())
                        .map(|id| format!("frame-{id}.png"))
                        .collect::<HashSet<_>>(),
                )),
                ObservationRecord::Audio(_) | ObservationRecord::NoChange(_) => None,
            })
            .collect::<HashMap<_, _>>();
        archive::collect_debug_materials(
            &mut context,
            debug_directory,
            &observer_frame_files,
            llm_call_id,
        );
        if let Some(trace) = judge_trace {
            context.add_json("judge/trace.json", trace);
            context.add_json("judge/request.json", &trace.request);
            for module in &trace.responses {
                context.add_json(
                    &format!("judge/module-{}.json", module.module_index),
                    module,
                );
            }
        } else if let Some(input_id) = judge_input_id {
            context
                .issues
                .push(format!("judge-trace-unavailable:{input_id}"));
        }
        context
    }

    fn collect_observation_materials(
        &mut self,
        transcript_directory: &Path,
        frame_directory: &Path,
    ) {
        let frame_ids = self
            .observations
            .iter()
            .filter_map(|record| match record {
                ObservationRecord::Visual(value) => Some(
                    value
                        .source_frame_ids
                        .iter()
                        .cloned()
                        .chain(value.source_frame_paths.keys().cloned())
                        .collect::<Vec<_>>(),
                ),
                ObservationRecord::NoChange(_) | ObservationRecord::Audio(_) => None,
            })
            .flatten()
            .collect::<BTreeSet<_>>();
        let fallback_paths = match FrameBuffer::new(frame_directory.to_owned())
            .paths_for_ids(frame_ids.iter().map(String::as_str), Utc::now())
        {
            Ok(paths) => paths,
            Err(error) => {
                self.issues
                    .push(format!("frame-buffer-read-failed:{error}"));
                HashMap::new()
            }
        };
        let audio_by_id = self
            .observations
            .iter()
            .filter_map(|record| match record {
                ObservationRecord::Audio(value) => Some((value.id.clone(), value.clone())),
                ObservationRecord::Visual(_) | ObservationRecord::NoChange(_) => None,
            })
            .collect::<HashMap<_, _>>();
        let observations = self.observations.clone();
        for observation in observations {
            let id = observation.id().to_owned();
            let path = format!("observations/{}.json", digest(&id));
            match &observation {
                ObservationRecord::Visual(value) => {
                    self.collect_frames(value, frame_directory, &fallback_paths);
                    for segment in &value.audio_segments {
                        self.collect_audio_segment(
                            transcript_directory,
                            segment,
                            audio_by_id
                                .get(crate::state::audio_segment_observation_id(&segment.id)),
                        );
                    }
                }
                ObservationRecord::Audio(value) => {
                    self.add_json(&format!("audio-segments/{}.json", digest(&value.id)), value);
                    let transcript = sanitize_text(&value.text);
                    self.add_bytes(
                        &format!("transcripts/{}.txt", digest(&value.id)),
                        transcript.as_bytes().to_vec(),
                    );
                    self.audio_segments.push(FeedbackAudioSegment {
                        id: value.id.clone(),
                        time: value.created_at.clone(),
                        source: value.source,
                        transcript_path: Some(format!("transcripts/{}.txt", digest(&value.id))),
                        transcript: Some(transcript),
                    });
                }
                ObservationRecord::NoChange(_) => {}
            }
            let projected = archive::project_observation(&observation, &self.frame_archive_paths);
            self.add_json(&path, &projected);
            self.observation_archive_paths.insert(id, path);
        }
        self.audio_segments
            .sort_by(|left, right| left.id.cmp(&right.id));
        let mut unique_segments = Vec::with_capacity(self.audio_segments.len());
        for segment in self.audio_segments.drain(..) {
            if let Some(existing) = unique_segments
                .iter_mut()
                .find(|existing: &&mut FeedbackAudioSegment| existing.id == segment.id)
            {
                if existing.transcript.is_none() {
                    existing.transcript = segment.transcript.clone();
                }
                if existing.transcript_path.is_none() {
                    existing.transcript_path = segment.transcript_path.clone();
                }
            } else {
                unique_segments.push(segment);
            }
        }
        self.audio_segments = unique_segments;
    }

    fn collect_audio_segment(
        &mut self,
        transcript_directory: &Path,
        segment: &crate::state::AudioSegmentReference,
        audio: Option<&crate::state::AudioObservation>,
    ) {
        // 期間参照は観察 id に戻して扱う。判断役の材料の本文は観察単位の全文を正本とし、
        // 期間ごとの分割はこの経路では使わない。観察が欠けた場合だけ期間行の連結へ落ちる。
        let id = crate::state::audio_segment_observation_id(segment.id.as_str());
        let transcript = audio.map(|audio| audio.text.clone()).or_else(|| {
            archive::load_transcript(
                transcript_directory,
                segment.transcript_path.as_deref(),
                id,
                &mut self.issues,
            )
        });
        if let Some(audio) = audio {
            self.add_json(&format!("audio-segments/{}.json", digest(id)), audio);
        } else {
            self.issues
                .push(format!("audio-observation-not-found:{id}"));
        }
        let transcript = transcript.map(|value| sanitize_text(&value));
        if let Some(transcript) = transcript.as_deref() {
            self.add_bytes(
                &format!("transcripts/{}.txt", digest(id)),
                transcript.as_bytes().to_vec(),
            );
        }
        self.audio_segments.push(FeedbackAudioSegment {
            id: id.to_owned(),
            time: segment.time.clone(),
            source: segment.source,
            transcript_path: transcript
                .as_ref()
                .map(|_| format!("transcripts/{}.txt", digest(id))),
            transcript,
        });
    }

    fn collect_frames(
        &mut self,
        observation: &crate::state::VisualObservation,
        frame_directory: &Path,
        fallback_paths: &HashMap<String, PathBuf>,
    ) {
        let frame_ids = observation
            .source_frame_ids
            .iter()
            .cloned()
            .chain(observation.source_frame_paths.keys().cloned())
            .collect::<BTreeSet<_>>();
        for frame_id in frame_ids {
            let source = observation
                .source_frame_paths
                .get(&frame_id)
                .filter(|path| is_image_path(path))
                .and_then(|path| archive::safe_file_under(frame_directory, path))
                .or_else(|| {
                    fallback_paths
                        .get(&frame_id)
                        .filter(|path| is_image_path(&path.to_string_lossy()))
                        .and_then(|path| {
                            archive::safe_file_under(frame_directory, &path.to_string_lossy())
                        })
                });
            let Some(source) = source else {
                self.issues.push(format!("frame-expired:{frame_id}"));
                continue;
            };
            let archive_path = format!("frames/{}.png", digest(&frame_id));
            match fs::read(&source) {
                Ok(bytes) => {
                    self.add_bytes(&archive_path, bytes);
                    self.frame_archive_paths.insert(frame_id, archive_path);
                }
                Err(error) => self
                    .issues
                    .push(format!("frame-read-failed:{frame_id}:{error}")),
            }
        }
    }

    pub(crate) fn add_json<T: Serialize>(&mut self, path: &str, value: &T) {
        match serde_json::to_value(value)
            .map(|value| serde_json::to_vec(&archive::sanitize_json(&value)))
            .and_then(|result| result)
        {
            Ok(bytes) => self.add_bytes(path, bytes),
            Err(error) => self
                .issues
                .push(format!("archive-json-failed:{path}:{error}")),
        }
    }

    pub(crate) fn add_bytes(&mut self, path: &str, bytes: Vec<u8>) {
        match self.materials.get(path) {
            Some(previous) if previous != &bytes => {
                self.issues.push(format!("archive-path-collision:{path}"))
            }
            Some(_) => {}
            None => {
                self.materials.insert(path.to_owned(), bytes);
            }
        }
    }
}

fn is_image_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp"
            )
        })
}

fn digest(value: &str) -> String {
    super::digest(value)
}

fn sanitize_text(value: &str) -> String {
    archive::sanitize_text(value)
}
