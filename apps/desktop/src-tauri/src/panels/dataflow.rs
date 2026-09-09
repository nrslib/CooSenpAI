use super::*;
use serde_json::json;

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
            let transcript = transcripts
                .iter()
                .rev()
                .find(|t| !t["observationId"].is_null() && t["observationId"] == observation["id"]);
            events.push(observation_event(
                observation,
                transcript.and_then(|t| t["text"].as_str()),
            )?);
        }
        for entry in snapshot["conversation"]
            .as_array()
            .ok_or("会話一覧がありません")?
        {
            if entry["role"] == "companion" {
                events.push(record(
                    "remark",
                    "companion",
                    format!("companion-remark:{}", string(entry, "id")?),
                    string(entry, "createdAt")?,
                    entry.clone(),
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
            .map(|e| e["id"].as_str().expect("event id").to_owned())
            .collect();
        self.events.extend(
            incoming
                .into_iter()
                .filter(|event| ids.insert(event["id"].as_str().expect("event id").to_owned())),
        );
        self.events
            .sort_by(|a, b| a["at"].as_str().cmp(&b["at"].as_str()));
        if self.events.len() > 200 {
            self.events.drain(..self.events.len() - 200);
        }
    }
    pub(super) fn view(&self) -> Value {
        json!({"events":self.events,"loadError":self.error})
    }
}
fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field]
        .as_str()
        .ok_or_else(|| format!("データフローの {field} がありません"))
}
fn record(kind: &str, category: &str, id: String, at: &str, data: Value) -> Value {
    json!({"id":id,"kind":category,"at":at,"content":{"type":kind,"data":data}})
}
fn observation_event(value: &Value, transcript: Option<&str>) -> Result<Value, String> {
    let kind = string(value, "kind")?;
    let category = if kind == "audio" { "hearing" } else { "visual" };
    Ok(record(
        "observation",
        category,
        format!("{category}:{}", string(value, "id")?),
        string(value, "createdAt")?,
        json!({"record":value,"transcript":transcript}),
    ))
}
fn diff(previous: &Value, next: &Value, now: &str) -> Result<Vec<Value>, String> {
    let mut events = Vec::new();
    let observation = &next["observer"]["lastObservation"];
    if !observation.is_null() && observation["id"] != previous["observer"]["lastObservation"]["id"]
    {
        events.push(observation_event(observation, None)?);
    }
    let previous_audio = previous["audio"]["recentEvents"]
        .as_array()
        .ok_or("聴覚のイベント一覧がありません")?;
    for event in next["audio"]["recentEvents"]
        .as_array()
        .ok_or("聴覚のイベント一覧がありません")?
    {
        if !previous_audio.iter().any(|old| old["id"] == event["id"]) {
            events.push(record(
                "hearing",
                "hearing",
                format!("hearing:{}", string(event, "id")?),
                string(event, "createdAt")?,
                event.clone(),
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
            "session",
            "hearing",
            format!(
                "hearing-session:{}:{}",
                audio["generation"], next["revision"]
            ),
            now,
            audio.clone(),
        ));
    }
    if !audio["warningKind"].is_null()
        && !audio["message"].is_null()
        && (audio["warningKind"] != previous["audio"]["warningKind"]
            || audio["message"] != previous["audio"]["message"])
    {
        events.push(record(
            "diagnostic",
            "hearing",
            format!("hearing-diagnostic:{}", next["revision"]),
            now,
            json!({"kind":audio["warningKind"],"message":audio["message"]}),
        ));
    }
    let interruption = &next["latestUserInterruption"];
    if !interruption.is_null()
        && interruption["sequence"] != previous["latestUserInterruption"]["sequence"]
    {
        for (field, kind) in [("observer", "visual"), ("proactive", "companion")] {
            if interruption[field] == true {
                events.push(record(
                    "interruption",
                    kind,
                    format!("user-interruption:{}:{kind}", interruption["sequence"]),
                    string(interruption, "occurredAt")?,
                    Value::Null,
                ));
            }
        }
    }
    let decision = &next["latestCompanionDecision"];
    if !decision.is_null()
        && decision["sequence"] != previous["latestCompanionDecision"]["sequence"]
    {
        events.push(record(
            "decision",
            "companion",
            format!("companion-decision:{}", decision["sequence"]),
            string(decision, "occurredAt")?,
            decision.clone(),
        ));
    }
    Ok(events)
}
