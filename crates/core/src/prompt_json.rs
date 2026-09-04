use serde_json::Value;

pub fn ordered_json_string(value: &Value) -> String {
    let mut result = String::new();
    write_ordered_json(value, &mut result);
    result
}

fn write_ordered_json(value: &Value, result: &mut String) {
    match value {
        Value::Null => result.push_str("null"),
        Value::Bool(value) => result.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => result.push_str(&value.to_string()),
        Value::String(value) => write_json_string(value, result),
        Value::Array(values) => {
            result.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    result.push(',');
                }
                write_ordered_json(value, result);
            }
            result.push(']');
        }
        Value::Object(values) => write_object(values, result),
    }
}

fn write_object(values: &serde_json::Map<String, Value>, result: &mut String) {
    result.push('{');
    let preferred = preferred_keys(values);
    let mut first = true;
    for key in preferred {
        if let Some(value) = values.get(*key) {
            write_object_entry(key, value, &mut first, result);
        }
    }
    let mut remaining = values
        .keys()
        .filter(|key| !preferred.contains(&key.as_str()))
        .collect::<Vec<_>>();
    remaining.sort();
    for key in remaining {
        if let Some(value) = values.get(key) {
            write_object_entry(key, value, &mut first, result);
        }
    }
    result.push('}');
}

fn write_object_entry(key: &str, value: &Value, first: &mut bool, result: &mut String) {
    if !*first {
        result.push(',');
    }
    *first = false;
    write_json_string(key, result);
    result.push(':');
    write_ordered_json(value, result);
}

fn preferred_keys(value: &serde_json::Map<String, Value>) -> &'static [&'static str] {
    if value.contains_key("kind") && value.contains_key("schemaVersion") {
        if value.get("kind").and_then(Value::as_str) == Some("no-change") {
            &[
                "kind",
                "schemaVersion",
                "id",
                "createdAt",
                "windowStart",
                "windowEnd",
            ]
        } else {
            &[
                "kind",
                "schemaVersion",
                "id",
                "createdAt",
                "windowStart",
                "windowEnd",
                "frameCount",
                "frames",
                "activity",
                "outline",
                "changes",
                "events",
                "guess",
                "confidence",
                "wakeCompanion",
            ]
        }
    } else if value.contains_key("activity") && value.contains_key("outline") {
        &[
            "activity",
            "outline",
            "changes",
            "events",
            "guess",
            "confidence",
            "wakeCompanion",
        ]
    } else if value.contains_key("trigger") && value.contains_key("frontApp") {
        &["trigger", "frontApp"]
    } else if value.contains_key("region") && value.contains_key("text") {
        &["region", "app", "text"]
    } else if value.contains_key("type") && value.contains_key("detail") {
        &["type", "detail"]
    } else if value.contains_key("role") && value.contains_key("message") {
        &["role", "message"]
    } else {
        &[]
    }
}

fn write_json_string(value: &str, result: &mut String) {
    result.push('"');
    for character in value.chars() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\u{08}' => result.push_str("\\b"),
            '\u{09}' => result.push_str("\\t"),
            '\u{0a}' => result.push_str("\\n"),
            '\u{0c}' => result.push_str("\\f"),
            '\u{0d}' => result.push_str("\\r"),
            character if character <= '\u{1f}' => {
                result.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => result.push(character),
        }
    }
    result.push('"');
}
