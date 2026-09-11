use super::*;
use serde_json::{json, Map, Value};
use std::path::Path;

#[derive(Default)]
pub(super) struct DataFlow {
    previous: Option<Value>,
    history: Option<IoResult>,
    history_loaded: bool,
    events: Vec<Value>,
    error: Option<String>,
}

impl DataFlow {
    pub(super) fn history(&mut self, result: IoResult) -> Result<(), String> {
        result.validate()?;
        if !self.history_loaded {
            self.history = Some(result);
            self.seed_history()?;
        }
        Ok(())
    }

    pub(super) fn snapshot(&mut self, snapshot: Value) -> Result<(), String> {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        if let Some(previous) = &self.previous {
            self.merge(diff(previous, &snapshot, &now)?);
        }
        self.previous = Some(snapshot);
        self.seed_history()
    }

    fn seed_history(&mut self) -> Result<(), String> {
        let (Some(snapshot), Some(history)) = (&self.previous, &self.history) else {
            return Ok(());
        };
        if self.history_loaded {
            return Ok(());
        }
        self.history_loaded = true;
        if !history.ok {
            self.error = Some(history.message().into());
            return Ok(());
        }
        let observations = history.value["observations"]
            .as_array()
            .ok_or("履歴の observations がありません")?;
        let transcripts = history.value["transcripts"]
            .as_array()
            .ok_or("履歴の transcripts がありません")?;
        let mut events = Vec::new();
        for observation in observations {
            events.extend(observation_events(observation, transcripts)?);
        }
        for entry in snapshot["conversation"]
            .as_array()
            .ok_or("会話一覧がありません")?
        {
            if entry["role"] == "companion" {
                events.push(record(
                    "coo",
                    "speech",
                    "remark",
                    format!("coo-speech:{}", string(entry, "id")?),
                    string(entry, "createdAt")?,
                    entry.clone(),
                    conversation_references(entry, snapshot),
                ));
            }
        }
        let mut baseline = snapshot.clone();
        baseline["observer"]["lastObservation"] = Value::Null;
        baseline["audio"]["recentEvents"] = json!([]);
        baseline["latestCompanionDecision"] = Value::Null;
        baseline["latestUserInterruption"] = Value::Null;
        events.extend(diff(
            &baseline,
            snapshot,
            &chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        )?);
        self.merge(events);
        self.history = None;
        Ok(())
    }

    fn merge(&mut self, incoming: Vec<Value>) {
        let mut ids: std::collections::HashSet<_> = self
            .events
            .iter()
            .map(|event| event["id"].as_str().expect("event id").to_owned())
            .collect();
        self.events.extend(
            incoming
                .into_iter()
                .filter(|event| ids.insert(event["id"].as_str().expect("event id").to_owned())),
        );
        self.events
            .sort_by(|left, right| left["at"].as_str().cmp(&right["at"].as_str()));
        if self.events.len() > 200 {
            self.events.drain(..self.events.len() - 200);
        }
    }

    pub(super) fn view(&self) -> Value {
        let mut events = self.events.clone();
        for event in &mut events {
            for reference in event["references"].as_array_mut().expect("references") {
                if let Some(path) = reference["path"].as_str() {
                    reference["available"] = json!(Path::new(path).is_file());
                }
                if reference["type"] == "observation" {
                    if let Some(observation) = self.events.iter().find_map(|event| {
                        let record = &event["content"]["data"]["record"];
                        (record["id"] == reference["id"]).then_some(record)
                    }) {
                        reference["observationKind"] = json!(observation_marker(observation));
                    }
                }
                if reference["type"] == "transcript" && !reference["text"].is_string() {
                    if let Some(transcript) = self.events.iter().find_map(|event| {
                        let data = &event["content"]["data"];
                        if data["id"] == reference["id"] && data["text"].is_string() {
                            Some(data)
                        } else if data["transcript"]["observationId"] == reference["id"] {
                            Some(&data["transcript"])
                        } else {
                            None
                        }
                    }) {
                        reference["text"] = transcript["text"].clone();
                    }
                }
            }
        }
        json!({"events":events,"loadError":self.error})
    }
}

fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field]
        .as_str()
        .ok_or_else(|| format!("データフローの {field} がありません"))
}

fn record(
    subject: &str,
    marker: &str,
    content_type: &str,
    id: String,
    at: &str,
    data: Value,
    references: Vec<Value>,
) -> Value {
    json!({
        "id": id,
        "subject": subject,
        "marker": marker,
        "at": at,
        "references": references,
        "content": {"type": content_type, "data": data}
    })
}

fn observation_events(value: &Value, transcripts: &[Value]) -> Result<Vec<Value>, String> {
    let kind = string(value, "kind")?;
    if kind == "audio" {
        let transcript = transcripts
            .iter()
            .rev()
            .find(|transcript| transcript["observationId"] == value["id"]);
        let references = transcript
            .into_iter()
            .map(transcript_reference)
            .collect::<Vec<_>>();
        return Ok(vec![record(
            "hearing",
            "transcript",
            "observation",
            format!("hearing-transcript:{}", string(value, "id")?),
            string(value, "createdAt")?,
            json!({"record":value,"transcript":transcript}),
            references,
        )]);
    }

    let source_frame_ids = value["sourceFrameIds"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let has_audio = value["audioSegments"]
        .as_array()
        .is_some_and(|segments| !segments.is_empty());
    let hearing = has_audio && source_frame_ids.is_empty();
    let subject = if hearing { "hearing" } else { "vision" };
    let marker = if hearing {
        "audio-observation"
    } else {
        "screen-observation"
    };
    let mut events = vec![record(
        subject,
        marker,
        "observation",
        format!("{subject}-observation:{}", string(value, "id")?),
        string(value, "createdAt")?,
        json!({"record":value,"transcripts":matching_transcripts(value, transcripts)}),
        observation_references(value, subject, transcripts),
    )];
    if subject == "vision" {
        events.extend(ocr_events(value)?);
    }
    Ok(events)
}

fn matching_transcripts<'a>(value: &Value, transcripts: &'a [Value]) -> Vec<&'a Value> {
    let ids = value["audioSegments"]
        .as_array()
        .into_iter()
        .flat_map(|segments| segments.iter())
        .filter_map(|segment| segment["id"].as_str())
        .collect::<std::collections::HashSet<_>>();
    transcripts
        .iter()
        .filter(|transcript| {
            transcript["observationId"]
                .as_str()
                .is_some_and(|id| ids.contains(id))
        })
        .collect()
}

fn observation_marker(value: &Value) -> &'static str {
    if value["kind"] == "audio" {
        "transcript"
    } else if value["audioSegments"]
        .as_array()
        .is_some_and(|segments| !segments.is_empty())
    {
        "audio-observation"
    } else {
        "screen-observation"
    }
}

fn observation_references(value: &Value, subject: &str, transcripts: &[Value]) -> Vec<Value> {
    if subject == "hearing" {
        return value["audioSegments"]
            .as_array()
            .into_iter()
            .flat_map(|segments| segments.iter())
            .map(|segment| {
                let mut reference = audio_segment_reference(segment);
                if let Some(transcript) = transcripts
                    .iter()
                    .find(|transcript| transcript["observationId"] == segment["id"])
                {
                    reference["text"] = transcript["text"].clone();
                }
                reference
            })
            .collect();
    }
    frame_references(value)
}

fn frame_references(value: &Value) -> Vec<Value> {
    let paths = value["sourceFramePaths"].as_object();
    value["sourceFrameIds"]
        .as_array()
        .into_iter()
        .flat_map(|ids| ids.iter())
        .filter_map(Value::as_str)
        .map(|id| {
            let path = paths
                .and_then(|paths| paths.get(id))
                .and_then(Value::as_str);
            reference(
                "frame",
                id,
                path,
                path.is_some_and(|value| Path::new(value).is_file()),
            )
        })
        .collect()
}

fn ocr_events(value: &Value) -> Result<Vec<Value>, String> {
    let ids = value["sourceFrameIds"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let references = frame_references(value);
    Ok(value["frames"]
        .as_array()
        .into_iter()
        .flat_map(|frames| frames.iter().enumerate())
        .filter_map(|(index, frame)| {
            let text = frame["ocrText"].as_str()?.trim();
            if text.is_empty() {
                return None;
            }
            let frame_id = ids.get(index)?.as_str()?;
            let frame_reference = references
                .iter()
                .find(|reference| reference["id"] == frame_id)
                .cloned()
                .into_iter()
                .collect();
            Some(record(
                "vision",
                "ocr",
                "ocr",
                format!("ocr:{}:{frame_id}", string(value, "id").ok()?),
                string(value, "createdAt").ok()?,
                json!({"observationId":value["id"],"frameId":frame_id,"text":text}),
                frame_reference,
            ))
        })
        .collect())
}

fn audio_segment_reference(segment: &Value) -> Value {
    let mut value = reference(
        "transcript",
        segment["id"].as_str().unwrap_or(""),
        segment["transcriptPath"].as_str(),
        segment["transcriptPath"]
            .as_str()
            .is_some_and(|path| Path::new(path).is_file()),
    );
    value["source"] = segment["source"].clone();
    value
}

fn transcript_reference(transcript: &Value) -> Value {
    let mut value = reference(
        "transcript",
        transcript["observationId"].as_str().unwrap_or(""),
        transcript["transcriptPath"].as_str(),
        transcript["transcriptPath"]
            .as_str()
            .is_some_and(|path| Path::new(path).is_file()),
    );
    value["source"] = transcript["source"].clone();
    value["text"] = transcript["text"].clone();
    value
}

fn reference(reference_type: &str, id: &str, path: Option<&str>, available: bool) -> Value {
    let mut value = Map::new();
    value.insert("type".into(), Value::String(reference_type.into()));
    value.insert("id".into(), Value::String(id.into()));
    value.insert("available".into(), Value::Bool(available));
    if let Some(path) = path {
        value.insert("path".into(), Value::String(path.into()));
    }
    Value::Object(value)
}

fn observation_references_for_ids(value: &Value) -> Vec<Value> {
    value["observationIds"]
        .as_array()
        .into_iter()
        .flat_map(|ids| ids.iter())
        .filter_map(Value::as_str)
        .map(|id| json!({"type":"observation","id":id}))
        .collect()
}

fn conversation_references(value: &Value, snapshot: &Value) -> Vec<Value> {
    let mut references = recent_conversation_references(
        snapshot,
        value["createdAt"].as_str().unwrap_or(""),
        value["id"].as_str(),
    );
    references.extend(
        value["causedByIds"]
            .as_array()
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(Value::as_str)
            .filter(|id| {
                !snapshot["conversation"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|entry| entry["id"] == *id)
            })
            .map(|id| json!({"type":"observation","id":id})),
    );
    references
}

fn recent_conversation_references(
    snapshot: &Value,
    at: &str,
    exclude_id: Option<&str>,
) -> Vec<Value> {
    snapshot["conversation"].as_array().into_iter().flatten()
        .filter(|entry| entry["id"].as_str() != exclude_id && entry["createdAt"].as_str().is_some_and(|time| time <= at))
        .rev().take(10).map(|entry| json!({"type":"conversation","id":entry["id"],"role":entry["role"],"text":entry["message"]})).collect()
}

fn diff(previous: &Value, next: &Value, now: &str) -> Result<Vec<Value>, String> {
    let mut events = Vec::new();
    let observation = &next["observer"]["lastObservation"];
    if !observation.is_null() && observation["id"] != previous["observer"]["lastObservation"]["id"]
    {
        events.extend(observation_events(observation, &[])?);
    }
    let previous_audio = previous["audio"]["recentEvents"]
        .as_array()
        .ok_or("聴覚のイベント一覧がありません")?;
    for event in next["audio"]["recentEvents"]
        .as_array()
        .ok_or("聴覚のイベント一覧がありません")?
    {
        if !previous_audio.iter().any(|old| old["id"] == event["id"]) {
            let marker = if event["stage"] == "confirmed" {
                "transcript"
            } else {
                "audio-status"
            };
            events.push(record(
                "hearing",
                marker,
                "hearing",
                format!("hearing:{}", string(event, "id")?),
                string(event, "createdAt")?,
                event.clone(),
                if marker == "transcript" {
                    let mut reference = transcript_reference(event);
                    reference["id"] = event["id"].clone();
                    vec![reference]
                } else {
                    Vec::new()
                },
            ));
        }
    }
    let audio = &next["audio"];
    if audio["phase"] != previous["audio"]["phase"]
        && ["listening", "off", "error"]
            .iter()
            .any(|phase| audio["phase"] == *phase)
    {
        events.push(record(
            "hearing",
            "session",
            "session",
            format!(
                "hearing-session:{}:{}",
                audio["generation"], next["revision"]
            ),
            now,
            audio.clone(),
            Vec::new(),
        ));
    }
    if !audio["warningKind"].is_null()
        && !audio["message"].is_null()
        && (audio["warningKind"] != previous["audio"]["warningKind"]
            || audio["message"] != previous["audio"]["message"])
    {
        events.push(record(
            "hearing",
            "diagnostic",
            "diagnostic",
            format!("hearing-diagnostic:{}", next["revision"]),
            now,
            json!({"kind":audio["warningKind"],"message":audio["message"]}),
            Vec::new(),
        ));
    }
    let interruption = &next["latestUserInterruption"];
    if !interruption.is_null()
        && interruption["sequence"] != previous["latestUserInterruption"]["sequence"]
    {
        for (field, subject) in [("observer", "vision"), ("proactive", "coo")] {
            if interruption[field] == true {
                events.push(record(
                    subject,
                    "interruption",
                    "interruption",
                    format!("user-interruption:{}:{subject}", interruption["sequence"]),
                    string(interruption, "occurredAt")?,
                    Value::Null,
                    Vec::new(),
                ));
            }
        }
    }
    let decision = &next["latestCompanionDecision"];
    if !decision.is_null()
        && decision["sequence"] != previous["latestCompanionDecision"]["sequence"]
    {
        let mut references = observation_references_for_ids(decision);
        references.extend(recent_conversation_references(
            next,
            string(decision, "occurredAt")?,
            None,
        ));
        events.push(record(
            "coo",
            "thought",
            "decision",
            format!("coo-decision:{}", decision["sequence"]),
            string(decision, "occurredAt")?,
            decision.clone(),
            references,
        ));
    }
    let previous_conversation = previous["conversation"]
        .as_array()
        .ok_or("会話一覧がありません")?;
    for entry in next["conversation"]
        .as_array()
        .ok_or("会話一覧がありません")?
    {
        if entry["role"] == "companion"
            && !previous_conversation
                .iter()
                .any(|old| old["id"] == entry["id"])
        {
            events.push(record(
                "coo",
                "speech",
                "remark",
                format!("coo-speech:{}", string(entry, "id")?),
                string(entry, "createdAt")?,
                entry.clone(),
                conversation_references(entry, next),
            ));
        }
    }
    Ok(events)
}
