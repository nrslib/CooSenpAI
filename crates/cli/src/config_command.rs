use anyhow::{Context, Result};
use coosenpai_core::config::{parse_config, patch_config, ConfigPaths, NUMERIC_CONFIG_PATHS};
use serde_json::Value;

pub(crate) fn run(paths: &ConfigPaths, key: &str, raw_value: &str) -> Result<()> {
    let value = parse_config_value(key, raw_value)?;
    patch_config(paths, None, move |config| {
        let mut document = serde_json::to_value(config)?;
        set_known_value(&mut document, key, value).map_err(|error| {
            coosenpai_core::config::ConfigError::Validation(vec![
                coosenpai_core::config::ConfigValidationIssue {
                    path: key.to_owned(),
                    message: error.to_string(),
                },
            ])
        })?;
        parse_config(document)
    })?;
    println!("設定を更新しました: {key}");
    Ok(())
}

fn parse_config_value(key: &str, raw: &str) -> Result<Value> {
    if matches!(
        key,
        "popup.quickActions.text" | "popup.quickActions.image" | "companion.reminders"
    ) {
        let value: Value = serde_json::from_str(raw)
            .with_context(|| format!("{key} は JSON 配列で指定してください。"))?;
        if !value.is_array() {
            anyhow::bail!("{key} は JSON 配列で指定してください。");
        }
        return Ok(value);
    }
    let field = key.rsplit('.').next().map_or("", |value| value);
    if field == "provider" {
        if !matches!(raw, "codex" | "claude" | "opencode") {
            anyhow::bail!("プロバイダが不正です: {raw}");
        }
        return Ok(Value::String(raw.to_owned()));
    }
    if field == "level" {
        if !matches!(raw, "fast" | "accurate") {
            anyhow::bail!("{key} は fast または accurate で指定してください。");
        }
        return Ok(Value::String(raw.to_owned()));
    }
    if (field == "executable" || key == "audio.debugDumpDir") && raw == "null" {
        return Ok(Value::Null);
    }
    if key == "companion.dailyProactiveLimit" && raw == "null" {
        return Ok(Value::Null);
    }
    if matches!(
        field,
        "showPriority"
            | "enabled"
            | "appSwitch"
            | "providerConsent"
            | "fullscreen"
            | "mic"
            | "speaker"
            | "confirmBeforeSend"
            | "thoughtBubble"
            | "alwaysShow"
            | "keepLatest"
            | "launchAtLogin"
            | "checkForUpdates"
    ) {
        return match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => anyhow::bail!("{key} は true または false で指定してください。"),
        };
    }
    if NUMERIC_CONFIG_PATHS.contains(&key) {
        let value: Value = serde_json::from_str(raw)
            .with_context(|| format!("{key} は数値で指定してください。"))?;
        if !value.is_number() || value.as_f64().is_none_or(|number| !number.is_finite()) {
            anyhow::bail!("{key} は数値で指定してください。");
        }
        return Ok(value);
    }
    if raw.trim().is_empty() && key != "companion.reviewTime" {
        anyhow::bail!("{key} は空にできません");
    }
    Ok(Value::String(raw.to_owned()))
}

fn set_known_value(document: &mut Value, key: &str, value: Value) -> Result<()> {
    let parts = key
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        anyhow::bail!("設定キーが空です")
    }
    let mut current = document;
    for part in &parts[..parts.len() - 1] {
        current = current
            .get_mut(*part)
            .with_context(|| format!("未対応の設定キーです: {key}"))?;
    }
    let object = current
        .as_object_mut()
        .context("設定キーの親が object ではありません")?;
    let Some(last) = parts.last() else {
        anyhow::bail!("設定キーが空です")
    };
    if !object.contains_key(*last) {
        anyhow::bail!("未対応の設定キーです: {key}")
    }
    object.insert((*last).to_owned(), value);
    Ok(())
}

