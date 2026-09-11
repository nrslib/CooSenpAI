use serde_json::{Map, Value};

pub(super) fn changed(current: &Value, candidate: &Value) -> Value {
    changed_value(current, candidate).unwrap_or_else(|| Value::Object(Map::new()))
}
fn changed_value(current: &Value, candidate: &Value) -> Option<Value> {
    if let Some(record) = candidate.as_object() {
        let fields: Map<_, _> = record
            .iter()
            .filter(|(key, _)| *key != "revision")
            .filter_map(|(key, value)| {
                changed_value(&current[key], value).map(|value| (key.clone(), value))
            })
            .collect();
        if fields.is_empty() {
            None
        } else {
            Some(Value::Object(fields))
        }
    } else if current == candidate {
        None
    } else {
        Some(candidate.clone())
    }
}
pub(super) fn merge(current: &Value, patch: &Value) -> Value {
    let Some(patch) = patch.as_object() else {
        return patch.clone();
    };
    let mut merged = current.as_object().cloned().unwrap_or_default();
    for (key, value) in patch {
        merged.insert(key.clone(), merge(&current[key], value));
    }
    Value::Object(merged)
}
pub(super) fn paths(patch: &Value) -> Vec<String> {
    fn visit(value: &Value, prefix: String, output: &mut Vec<String>) {
        match value.as_object() {
            Some(map) if !map.is_empty() => {
                for (key, value) in map {
                    visit(
                        value,
                        if prefix.is_empty() {
                            key.clone()
                        } else {
                            format!("{prefix}.{key}")
                        },
                        output,
                    );
                }
            }
            _ if !prefix.is_empty() => output.push(prefix),
            _ => {}
        }
    }
    let mut output = vec![];
    visit(patch, String::new(), &mut output);
    output
}

pub(super) fn category_for_issue(path: &str) -> &'static str {
    fn family(path: &str, prefix: &str) -> bool {
        path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|tail| tail.starts_with('.') || tail.starts_with('['))
    }
    let belongs = |prefix: &str| family(path, prefix);
    if belongs("work") {
        "work"
    } else if belongs("watch") || belongs("retention") {
        "vision"
    } else if belongs("audio") {
        "hearing"
    } else if belongs("voiceOutput") {
        "beta"
    } else if path == "speech.locale" {
        "general"
    } else if belongs("speech") {
        "speech"
    } else if belongs("notification")
        || belongs("bubble")
        || belongs("popup")
        || belongs("ui.thoughtBubble")
    {
        "notifications"
    } else if belongs("keymap") {
        "shortcuts"
    } else if belongs("observer.hearing") {
        "providers"
    } else if belongs("observer") {
        let provider_fields = [
            "provider",
            "model",
            "effort",
            "executable",
            "timeoutMs",
            "dailyCallLimit",
        ];
        if path == "observer"
            || provider_fields
                .iter()
                .any(|field| belongs(&format!("observer.{field}")))
            || ["vision", "hearing"].iter().any(|role| {
                provider_fields
                    .iter()
                    .any(|field| belongs(&format!("observer.{role}.{field}")))
            })
        {
            "providers"
        } else {
            "vision"
        }
    } else if belongs("companion") {
        if [
            "provider",
            "model",
            "effort",
            "executable",
            "timeoutMs",
            "dailyProactiveLimit",
        ]
        .iter()
        .any(|field| belongs(&format!("companion.{field}")))
        {
            "providers"
        } else if [
            "wakeCoalesceMax",
            "sessionMaxCalls",
            "stuckAfterMs",
            "pendingDeliveryLimit",
            "pendingDeliveryMaxBytes",
            "proactiveQuietMinutes",
        ]
        .iter()
        .any(|field| belongs(&format!("companion.{field}")))
        {
            "vision"
        } else {
            "general"
        }
    } else {
        "general"
    }
}
