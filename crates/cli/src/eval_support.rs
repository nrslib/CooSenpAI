use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) fn find_images(directory: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let images = find_optional_images(directory)?;
    if images.is_empty() {
        anyhow::bail!("画像がありません: {}", directory.display())
    }
    Ok(images)
}

pub(super) fn find_optional_images(directory: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut images = std::fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("frame-") && name.ends_with(".png"))
        })
        .collect::<Vec<_>>();
    images.sort();
    if images.is_empty() {
        let screen = directory.join("screen.png");
        if screen.is_file() {
            images.push(screen);
        }
    }
    Ok(images)
}

pub(super) fn eval_case_selected(name: &str, requested: Option<&str>) -> bool {
    requested.map_or_else(|| !name.starts_with("exp-"), |requested| requested == name)
}

pub(super) fn contains_question(message: &str) -> bool {
    message.contains(['？', '?'])
        || [
            "ですか",
            "ますか",
            "ましたか",
            "でしょうか",
            "ましょうか",
            "何の",
            "どんな",
            "どう ",
            "どの",
            "いつ",
            "どこ",
            "誰",
            "なぜ",
        ]
        .iter()
        .any(|marker| message.contains(marker))
}

pub(super) fn mention_reasons(text: &str, expected: &Value) -> Vec<String> {
    let normalized = text.to_lowercase();
    let mut reasons = Vec::new();
    for word in expected
        .get("mustMention")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !normalized.contains(&word.to_lowercase()) {
            reasons.push(format!("「{word}」に触れていない"));
        }
    }
    for alternatives in expected
        .get("mustMentionAny")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
    {
        let words = alternatives
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        if !words
            .iter()
            .any(|word| normalized.contains(&word.to_lowercase()))
        {
            reasons.push(format!("{} のいずれにも触れていない", words.join(" / ")));
        }
    }
    for word in expected
        .get("mustNotMention")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if normalized.contains(&word.to_lowercase()) {
            reasons.push(format!("「{word}」に触れてはいけない"));
        }
    }
    reasons
}

pub(super) fn changes_mention_reasons(value: &Value, expected: &Value) -> Vec<String> {
    let changes = value
        .get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    expected
        .get("changesMustMention")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|word| !changes.contains(&word.to_lowercase()))
        .map(|word| format!("changes が「{word}」に触れていない"))
        .collect()
}

pub(super) fn observation_forbidden_reasons(value: &Value, expected: &Value) -> Vec<String> {
    let semantic = ["activity", "changes", "events"]
        .into_iter()
        .filter_map(|key| value.get(key))
        .filter_map(|field| serde_json::to_string(field).ok())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    expected
        .get("observationMustNotMention")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|word| semantic.contains(&word.to_lowercase()))
        .map(|word| format!("観察内容が「{word}」に触れてはいけない"))
        .collect()
}

