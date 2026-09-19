use super::defaults::default_observer_daily_limit;
use super::{
    issue, AgentConfig, AppConfig, AudioConfig, BatteryConfig, BubbleConfig, ChatConfig,
    CompanionConfig, Config, ConfigError, ConfigValidationIssue, DebugConfig, JudgeComposition,
    JudgeConfig, JudgeModuleConfig, MemoryConfig, NotificationConfig, ObserverConfig,
    ObserverProfile, OcrGateConfig, PopupConfig, RetentionConfig, SpeechConfig, TriggerConfig,
    UiConfig, VoiceOutputConfig, WatchConfig,
};
#[path = "config_parse_helpers.rs"]
mod helpers;
#[path = "config_parse_keymap.rs"]
mod keymap;
#[path = "config_parse_memory.rs"]
mod memory;
#[path = "config_parse_popup.rs"]
mod popup;
#[path = "config_parse_presence.rs"]
mod presence;
#[path = "config_parse_watch_apps.rs"]
mod watch_apps;
use self::helpers::{
    app_window_limit, boolean, effort, effort_or_empty, enum_string, executable, frames_per_send,
    nonnegative_u32, nonnegative_u64, optional_nonnegative_u32, parse_audio, parse_chat,
    parse_debug, parse_speech, parse_ui, parse_voice_output, persona, positive_number,
    positive_u32, positive_u64, positive_usize, provider, string, string_or_empty, unknown_keys,
};
use keymap::parse_keymap;
use memory::parse_memory;
use popup::parse_popup;
use presence::{parse_app, parse_reminders, review_time};
use serde_json::{json, Map, Value};
use watch_apps::parse_watch_apps;

/// 旧形式のファイルを、現行の正規形式へ変換する。
///
/// この関数はファイル読み込みからだけ呼び出す。CLI や IPC の入力は
/// `parse_config` を直接通るため、廃止・統合前のキーを受理しない。
pub(super) fn normalize_legacy_file(mut value: Value) -> (Value, Vec<ConfigValidationIssue>) {
    let mut issues = Vec::new();
    let Some(root) = value.as_object_mut() else {
        return (value, issues);
    };

    let send_interval = root
        .get_mut("watch")
        .and_then(Value::as_object_mut)
        .and_then(|watch| watch.remove("sendIntervalMs"))
        .map(|value| legacy_positive_u64(value, "watch.sendIntervalMs", &mut issues));

    if let Some(notification) = root.get_mut("notification").and_then(Value::as_object_mut) {
        notification.remove("showPriority");
    }
    if let Some(bubble) = root.get_mut("bubble").and_then(Value::as_object_mut) {
        bubble.remove("alwaysShow");
    }

    let Some(observer) = root.get_mut("observer") else {
        if let Some(interval) = send_interval {
            root.insert(
                "observer".to_owned(),
                json!({"vision": {"intervalMs": interval}}),
            );
        }
        return (value, issues);
    };
    let Some(observer) = observer.as_object_mut() else {
        return (value, issues);
    };

    if observer.contains_key("vision") || observer.contains_key("hearing") {
        if let Some(vision) = observer.get_mut("vision").and_then(Value::as_object_mut) {
            normalize_legacy_profile(vision, "observer.vision", &mut issues);
        }
        if let Some(hearing) = observer.get_mut("hearing").and_then(Value::as_object_mut) {
            normalize_legacy_profile(hearing, "observer.hearing", &mut issues);
        }
        if let Some(interval) = send_interval {
            if observer.get("vision").is_none() {
                observer.insert("vision".to_owned(), json!({"intervalMs": interval}));
            }
        }
    } else {
        normalize_legacy_profile(observer, "observer", &mut issues);
        if observer.get("intervalMs").is_none() {
            if let Some(interval) = send_interval {
                observer.insert("intervalMs".to_owned(), Value::from(interval));
            }
        }
    }

    (value, issues)
}

fn normalize_legacy_profile(
    profile: &mut Map<String, Value>,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) {
    let old_chars = profile.remove("textExcerptMaxChars");
    let old_count = profile.remove("textExcerptMaxCount");
    if old_chars.is_none() && old_count.is_none() {
        return;
    }

    let chars = old_chars
        .map(|value| legacy_positive_usize(value, &format!("{path}.textExcerptMaxChars"), issues))
        .unwrap_or(600);
    let count = old_count
        .map(|value| legacy_positive_usize(value, &format!("{path}.textExcerptMaxCount"), issues))
        .unwrap_or(6);
    let legacy_total = chars.saturating_mul(count);
    match profile.get_mut("textTotalMaxChars") {
        None => {
            profile.insert("textTotalMaxChars".to_owned(), Value::from(legacy_total));
        }
        Some(value) => {
            if let Some(total) = positive_usize_value(value) {
                *value = Value::from(total.min(legacy_total));
            }
        }
    }
}

fn legacy_positive_u64(value: Value, path: &str, issues: &mut Vec<ConfigValidationIssue>) -> u64 {
    match value.as_u64() {
        Some(value) if value > 0 => value,
        _ => {
            issues.push(issue(path, "正の整数で指定してください。"));
            60_000
        }
    }
}

fn legacy_positive_usize(
    value: Value,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> usize {
    match value.as_u64().and_then(|value| usize::try_from(value).ok()) {
        Some(value) if value > 0 => value,
        _ => {
            issues.push(issue(path, "正の整数で指定してください。"));
            if path.ends_with("textExcerptMaxCount") {
                6
            } else {
                600
            }
        }
    }
}

fn positive_usize_value(value: &Value) -> Option<usize> {
    value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
}

pub(super) fn parse_v3(value: Value) -> Result<Config, ConfigError> {
    let (config, issues) = parse_v3_with_issues(value);
    if issues.is_empty() {
        Ok(config)
    } else {
        Err(ConfigError::Validation(issues))
    }
}

pub(super) fn parse_v3_with_issues(value: Value) -> (Config, Vec<ConfigValidationIssue>) {
    let Some(object) = value.as_object() else {
        return (
            Config::default(),
            vec![issue("config", "設定はオブジェクトで指定してください。")],
        );
    };
    let mut issues = unknown_keys(
        object,
        &[
            "configVersion",
            "work",
            "revision",
            "watch",
            "observer",
            "companion",
            "chat",
            "notification",
            "bubble",
            "retention",
            "memory",
            "debug",
            "judge",
            "audio",
            "speech",
            "voiceOutput",
            "ui",
            "keymap",
            "popup",
            "app",
        ],
        "config",
    );
    let work = match object.get("work") {
        None => crate::work::WorkConfig::default(),
        Some(value) => match serde_json::from_value::<crate::work::WorkConfig>(value.clone()) {
            Ok(work) => work,
            Err(_) => {
                issues.push(issue(
                    "work",
                    "approvalMode は manual または auto、allowedRoots は {path, read, write} の配列で指定してください。",
                ));
                crate::work::WorkConfig::default()
            }
        },
    };
    let watch = parse_section(
        object.get("watch"),
        WatchConfig::default(),
        "watch",
        &mut issues,
        parse_watch,
    );
    let revision = nonnegative_u64(object, "revision", 0, "revision", &mut issues);
    let observer = parse_section(
        object.get("observer"),
        ObserverConfig::default(),
        "observer",
        &mut issues,
        parse_observer,
    );
    let companion = parse_section(
        object.get("companion"),
        CompanionConfig::default(),
        "companion",
        &mut issues,
        parse_companion,
    );
    let chat = parse_section(
        object.get("chat"),
        ChatConfig::default(),
        "chat",
        &mut issues,
        parse_chat,
    );
    let notification = parse_section(
        object.get("notification"),
        NotificationConfig::default(),
        "notification",
        &mut issues,
        parse_notification,
    );
    let bubble = parse_section(
        object.get("bubble"),
        BubbleConfig::default(),
        "bubble",
        &mut issues,
        parse_bubble,
    );
    let retention = parse_section(
        object.get("retention"),
        RetentionConfig::default(),
        "retention",
        &mut issues,
        parse_retention,
    );
    let memory = parse_section(
        object.get("memory"),
        MemoryConfig::default(),
        "memory",
        &mut issues,
        parse_memory,
    );
    let debug = parse_section(
        object.get("debug"),
        DebugConfig::default(),
        "debug",
        &mut issues,
        parse_debug,
    );
    let judge = parse_section(
        object.get("judge"),
        JudgeConfig::default(),
        "judge",
        &mut issues,
        parse_judge,
    );
    let audio = parse_section(
        object.get("audio"),
        AudioConfig::default(),
        "audio",
        &mut issues,
        parse_audio,
    );
    let speech = parse_section(
        object.get("speech"),
        SpeechConfig::default(),
        "speech",
        &mut issues,
        parse_speech,
    );
    let voice_output = parse_section(
        object.get("voiceOutput"),
        VoiceOutputConfig::default(),
        "voiceOutput",
        &mut issues,
        parse_voice_output,
    );
    let ui = parse_section(
        object.get("ui"),
        UiConfig::default(),
        "ui",
        &mut issues,
        parse_ui,
    );
    let keymap = parse_keymap(object, &mut issues);
    let popup = parse_section(
        object.get("popup"),
        PopupConfig::default(),
        "popup",
        &mut issues,
        parse_popup,
    );
    let app = parse_section(
        object.get("app"),
        AppConfig::default(),
        "app",
        &mut issues,
        parse_app,
    );
    (
        Config {
            work,
            config_version: 3,
            revision,
            watch,
            observer,
            companion,
            chat,
            notification,
            bubble,
            retention,
            memory,
            debug,
            judge,
            audio,
            speech,
            voice_output,
            ui,
            keymap,
            popup,
            app,
        },
        issues,
    )
}

fn parse_judge(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> JudgeConfig {
    issues.extend(unknown_keys(
        object,
        &["follow", "composition", "veto", "modules", "timeoutMs"],
        "judge",
    ));
    let composition = match enum_string(
        object,
        "composition",
        "single",
        &["single", "ensemble", "weighted"],
        "judge.composition",
        issues,
    )
    .as_str()
    {
        "ensemble" => JudgeComposition::Ensemble,
        "weighted" => JudgeComposition::Weighted,
        _ => JudgeComposition::Single,
    };
    JudgeConfig {
        follow: boolean(object, "follow", false, "judge.follow", issues),
        composition,
        veto: boolean(object, "veto", false, "judge.veto", issues),
        modules: parse_judge_modules(object.get("modules"), issues),
        timeout_ms: positive_u64(object, "timeoutMs", 3_000, "judge.timeoutMs", issues),
    }
}

fn parse_judge_modules(
    value: Option<&Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> Vec<JudgeModuleConfig> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Some(values) = value.as_array() else {
        issues.push(issue("judge.modules", "配列で指定してください。"));
        return Vec::new();
    };
    let mut modules = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let path = format!("judge.modules[{index}]");
        let Some(object) = value.as_object() else {
            issues.push(issue(&path, "オブジェクトで指定してください。"));
            continue;
        };
        issues.extend(unknown_keys(
            object,
            &["executable", "arguments", "environment", "weight"],
            &path,
        ));
        let executable = match object.get("executable") {
            Some(Value::String(value))
                if !value.is_empty() && std::path::Path::new(value).is_absolute() =>
            {
                Some(value.clone())
            }
            _ => {
                issues.push(issue(
                    format!("{path}.executable"),
                    "実行ファイルは空でない絶対パスで指定してください。",
                ));
                None
            }
        };
        let arguments = parse_judge_arguments(object.get("arguments"), &path, issues);
        let environment = parse_judge_environment(object.get("environment"), &path, issues);
        let weight = positive_number(object, "weight", 1.0, &format!("{path}.weight"), issues);
        if let Some(executable) = executable {
            modules.push(JudgeModuleConfig {
                executable,
                arguments,
                environment,
                weight,
            });
        }
    }
    modules
}

fn parse_judge_arguments(
    value: Option<&Value>,
    module_path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    let path = format!("{module_path}.arguments");
    let Some(values) = value.as_array() else {
        issues.push(issue(path, "文字列の配列で指定してください。"));
        return Vec::new();
    };
    values
        .iter()
        .enumerate()
        .filter_map(|(index, value)| match value {
            Value::String(value) => Some(value.clone()),
            _ => {
                issues.push(issue(
                    format!("{path}[{index}]"),
                    "文字列で指定してください。",
                ));
                None
            }
        })
        .collect()
}

fn parse_judge_environment(
    value: Option<&Value>,
    module_path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> std::collections::BTreeMap<String, String> {
    let Some(value) = value else {
        return std::collections::BTreeMap::new();
    };
    let path = format!("{module_path}.environment");
    let Some(values) = value.as_object() else {
        issues.push(issue(
            path,
            "文字列値を持つオブジェクトで指定してください。",
        ));
        return std::collections::BTreeMap::new();
    };
    let mut environment = std::collections::BTreeMap::new();
    for (key, value) in values {
        match value {
            Value::String(value) => {
                environment.insert(key.clone(), value.clone());
            }
            _ => issues.push(issue(&path, "環境変数の値は文字列で指定してください。")),
        }
    }
    environment
}

fn parse_bubble(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> BubbleConfig {
    issues.extend(unknown_keys(
        object,
        &[
            "keepLatest",
            "edgeRecall",
            "maxStack",
            "position",
            "display",
        ],
        "bubble",
    ));
    BubbleConfig {
        keep_latest: boolean(object, "keepLatest", false, "bubble.keepLatest", issues),
        edge_recall: boolean(object, "edgeRecall", true, "bubble.edgeRecall", issues),
        max_stack: positive_usize(object, "maxStack", 3, "bubble.maxStack", issues),
        position: enum_string(
            object,
            "position",
            "bottom-right",
            &["bottom-right", "top-right", "bottom-left", "top-left"],
            "bubble.position",
            issues,
        ),
        display: enum_string(
            object,
            "display",
            "main",
            &["main", "cursor", "front"],
            "bubble.display",
            issues,
        ),
    }
}

fn parse_section<T>(
    value: Option<&Value>,
    default: T,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
    parser: fn(&Map<String, Value>, &mut Vec<ConfigValidationIssue>) -> T,
) -> T {
    let Some(value) = value else { return default };
    let Some(object) = value.as_object() else {
        issues.push(issue(path, "設定はオブジェクトで指定してください。"));
        return default;
    };
    parser(object, issues)
}

fn parse_watch(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> WatchConfig {
    issues.extend(unknown_keys(
        object,
        &[
            "sendDebounceMs",
            "framesPerSend",
            "appWindowLimit",
            "changeThreshold",
            "changedPixelThreshold",
            "downscaleWidth",
            "triggers",
            "battery",
            "ocrGate",
            "enabled",
            "fullscreen",
            "focusElement",
            "apps",
        ],
        "watch",
    ));
    let triggers = parse_nested(
        object.get("triggers"),
        TriggerConfig::default(),
        "watch.triggers",
        issues,
        parse_triggers,
    );
    let battery = parse_nested(
        object.get("battery"),
        BatteryConfig::default(),
        "watch.battery",
        issues,
        parse_battery,
    );
    let ocr_gate = parse_nested(
        object.get("ocrGate"),
        OcrGateConfig::default(),
        "watch.ocrGate",
        issues,
        parse_ocr_gate,
    );
    let result = WatchConfig {
        enabled: boolean(object, "enabled", false, "watch.enabled", issues),
        fullscreen: boolean(object, "fullscreen", false, "watch.fullscreen", issues),
        focus_element: boolean(object, "focusElement", false, "watch.focusElement", issues),
        apps: parse_watch_apps(object.get("apps"), issues),
        send_debounce_ms: positive_u64(
            object,
            "sendDebounceMs",
            2_000,
            "watch.sendDebounceMs",
            issues,
        ),
        frames_per_send: frames_per_send(object, issues),
        app_window_limit: app_window_limit(object, issues),
        downscale_width: positive_u32(
            object,
            "downscaleWidth",
            1_280,
            "watch.downscaleWidth",
            issues,
        ),
        triggers,
        battery,
        ocr_gate,
    };
    if result.triggers.min_spacing_ms < result.triggers.poll_ms {
        issues.push(issue(
            "watch.triggers.minSpacingMs",
            "pollMs 以上で指定してください。",
        ));
    }
    if result.triggers.min_spacing_ms > result.triggers.max_interval_ms {
        issues.push(issue(
            "watch.triggers.minSpacingMs",
            "maxIntervalMs 以下で指定してください。",
        ));
    }
    result
}

fn parse_triggers(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> TriggerConfig {
    issues.extend(unknown_keys(
        object,
        &[
            "typingPauseMs",
            "activeThresholdMs",
            "appSwitch",
            "appSwitchSettleMs",
            "maxIntervalMs",
            "minSpacingMs",
            "pollMs",
        ],
        "watch.triggers",
    ));
    let result = TriggerConfig {
        typing_pause_ms: positive_u64(
            object,
            "typingPauseMs",
            2_000,
            "watch.triggers.typingPauseMs",
            issues,
        ),
        active_threshold_ms: positive_u64(
            object,
            "activeThresholdMs",
            1_000,
            "watch.triggers.activeThresholdMs",
            issues,
        ),
        app_switch: boolean(
            object,
            "appSwitch",
            true,
            "watch.triggers.appSwitch",
            issues,
        ),
        app_switch_settle_ms: positive_u64(
            object,
            "appSwitchSettleMs",
            1_500,
            "watch.triggers.appSwitchSettleMs",
            issues,
        ),
        max_interval_ms: positive_u64(
            object,
            "maxIntervalMs",
            60_000,
            "watch.triggers.maxIntervalMs",
            issues,
        ),
        min_spacing_ms: positive_u64(
            object,
            "minSpacingMs",
            5_000,
            "watch.triggers.minSpacingMs",
            issues,
        ),
        poll_ms: positive_u64(object, "pollMs", 1_000, "watch.triggers.pollMs", issues),
    };
    if result.active_threshold_ms >= result.typing_pause_ms {
        issues.push(issue(
            "watch.triggers.activeThresholdMs",
            "typingPauseMs より小さくしてください。",
        ));
    }
    result
}

fn parse_battery(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> BatteryConfig {
    issues.extend(unknown_keys(
        object,
        &["enabled", "multiplier"],
        "watch.battery",
    ));
    BatteryConfig {
        enabled: boolean(object, "enabled", true, "watch.battery.enabled", issues),
        multiplier: positive_number(
            object,
            "multiplier",
            2.0,
            "watch.battery.multiplier",
            issues,
        ),
    }
}

fn parse_ocr_gate(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> OcrGateConfig {
    issues.extend(unknown_keys(
        object,
        &["enabled", "level", "timeoutMs", "executable"],
        "watch.ocrGate",
    ));
    let level = match object.get("level") {
        None => "accurate".to_owned(),
        Some(Value::String(value)) if value == "fast" || value == "accurate" => value.clone(),
        Some(_) => {
            issues.push(issue(
                "watch.ocrGate.level",
                "fast または accurate で指定してください。",
            ));
            "accurate".to_owned()
        }
    };
    OcrGateConfig {
        enabled: boolean(object, "enabled", true, "watch.ocrGate.enabled", issues),
        level,
        timeout_ms: positive_u64(
            object,
            "timeoutMs",
            3_000,
            "watch.ocrGate.timeoutMs",
            issues,
        ),
        executable: executable(object, "executable", "watch.ocrGate.executable", issues),
    }
}

fn parse_observer(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> ObserverConfig {
    if object.contains_key("vision") || object.contains_key("hearing") {
        issues.extend(unknown_keys(object, &["vision", "hearing"], "observer"));
        let vision = parse_observer_profile_section(
            object.get("vision"),
            ObserverProfile::default(),
            "observer.vision",
            issues,
        );
        let hearing = match object.get("hearing") {
            Some(value) => parse_observer_profile_section(
                Some(value),
                ObserverProfile::default(),
                "observer.hearing",
                issues,
            ),
            None => vision.clone(),
        };
        return ObserverConfig { vision, hearing };
    }
    let vision = parse_observer_profile(object, "observer", issues);
    ObserverConfig {
        hearing: vision.clone(),
        vision,
    }
}

fn parse_observer_profile_section(
    value: Option<&Value>,
    default: ObserverProfile,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> ObserverProfile {
    let Some(value) = value else { return default };
    let Some(object) = value.as_object() else {
        issues.push(issue(path, "設定はオブジェクトで指定してください。"));
        return default;
    };
    parse_observer_profile(object, path, issues)
}

fn parse_observer_profile(
    object: &Map<String, Value>,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
) -> ObserverProfile {
    issues.extend(unknown_keys(
        object,
        &[
            "provider",
            "model",
            "effort",
            "stallTimeoutMs",
            "timeoutMs",
            "dailyCallLimit",
            "executable",
            "textTotalMaxChars",
            "changesMaxCount",
            "intervalMs",
        ],
        path,
    ));
    let path_value = |field: &str| format!("{path}.{field}");
    ObserverProfile::new(
        AgentConfig {
            provider: provider(object, "provider", "codex", &path_value("provider"), issues),
            model: string(object, "model", "default", &path_value("model"), issues),
            effort: effort(object, "effort", "default", &path_value("effort"), issues),
            executable: executable(object, "executable", &path_value("executable"), issues),
            stall_timeout_ms: positive_u64(
                object,
                "stallTimeoutMs",
                120_000,
                &path_value("stallTimeoutMs"),
                issues,
            ),
            timeout_ms: positive_u64(
                object,
                "timeoutMs",
                600_000,
                &path_value("timeoutMs"),
                issues,
            ),
            daily_call_limit: nonnegative_u32(
                object,
                "dailyCallLimit",
                default_observer_daily_limit(),
                &path_value("dailyCallLimit"),
                issues,
            ),
            text_total_max_chars: positive_usize(
                object,
                "textTotalMaxChars",
                2_000,
                &path_value("textTotalMaxChars"),
                issues,
            ),
            changes_max_count: positive_usize(
                object,
                "changesMaxCount",
                8,
                &path_value("changesMaxCount"),
                issues,
            ),
        },
        positive_u64(
            object,
            "intervalMs",
            60_000,
            &path_value("intervalMs"),
            issues,
        ),
    )
}

fn parse_companion(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> CompanionConfig {
    issues.extend(unknown_keys(
        object,
        &[
            "emotionsEnabled",
            "provider",
            "model",
            "effort",
            "proactiveModel",
            "proactiveEffort",
            "persona",
            "displayName",
            "assertiveness",
            "stallTimeoutMs",
            "timeoutMs",
            "dailyProactiveLimit",
            "executable",
            "wakeCoalesceMax",
            "sessionMaxCalls",
            "stuckAfterMs",
            "pendingDeliveryLimit",
            "pendingDeliveryMaxBytes",
            "contextRefreshCalls",
            "reviewTime",
            "reminders",
            "quietReportEvery",
            "proactiveQuietMinutes",
            "proactiveIdleMs",
        ],
        "companion",
    ));
    let provider = provider(object, "provider", "codex", "companion.provider", issues);
    let model = string(object, "model", "default", "companion.model", issues);
    let proactive_model =
        string_or_empty(object, "proactiveModel", "companion.proactiveModel", issues);
    CompanionConfig {
        emotions_enabled: boolean(
            object,
            "emotionsEnabled",
            true,
            "companion.emotionsEnabled",
            issues,
        ),
        provider,
        model,
        effort: effort(object, "effort", "default", "companion.effort", issues),
        proactive_model,
        proactive_effort: effort_or_empty(
            object,
            "proactiveEffort",
            "companion.proactiveEffort",
            issues,
        ),
        executable: executable(object, "executable", "companion.executable", issues),
        persona: persona(object, "persona", "coo-chan", "companion.persona", issues),
        display_name: parse_display_name(object, issues),
        assertiveness: enum_string(
            object,
            "assertiveness",
            "normal",
            &["low", "normal", "high"],
            "companion.assertiveness",
            issues,
        ),
        stall_timeout_ms: positive_u64(
            object,
            "stallTimeoutMs",
            120_000,
            "companion.stallTimeoutMs",
            issues,
        ),
        timeout_ms: positive_u64(
            object,
            "timeoutMs",
            1_800_000,
            "companion.timeoutMs",
            issues,
        ),
        daily_proactive_limit: optional_nonnegative_u32(
            object,
            "dailyProactiveLimit",
            "companion.dailyProactiveLimit",
            issues,
        ),
        wake_coalesce_max: positive_usize(
            object,
            "wakeCoalesceMax",
            5,
            "companion.wakeCoalesceMax",
            issues,
        ),
        session_max_calls: positive_usize(
            object,
            "sessionMaxCalls",
            60,
            "companion.sessionMaxCalls",
            issues,
        ),
        stuck_after_ms: positive_u64(
            object,
            "stuckAfterMs",
            900_000,
            "companion.stuckAfterMs",
            issues,
        ),
        pending_delivery_limit: positive_usize(
            object,
            "pendingDeliveryLimit",
            20,
            "companion.pendingDeliveryLimit",
            issues,
        ),
        pending_delivery_max_bytes: positive_usize(
            object,
            "pendingDeliveryMaxBytes",
            21_053_440,
            "companion.pendingDeliveryMaxBytes",
            issues,
        ),
        context_refresh_calls: positive_usize(
            object,
            "contextRefreshCalls",
            20,
            "companion.contextRefreshCalls",
            issues,
        ),
        review_time: review_time(object, issues),
        reminders: parse_reminders(object.get("reminders"), issues),
        quiet_report_every: None,
        proactive_quiet_minutes: proactive_quiet_minutes(object, issues),
        proactive_idle_ms: positive_u64(
            object,
            "proactiveIdleMs",
            600_000,
            "companion.proactiveIdleMs",
            issues,
        ),
    }
}

fn proactive_quiet_minutes(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> u64 {
    let value = positive_u64(
        object,
        "proactiveQuietMinutes",
        1,
        "companion.proactiveQuietMinutes",
        issues,
    );
    if value > 1_440 {
        issues.push(issue(
            "companion.proactiveQuietMinutes",
            "1以上1440以下の整数で指定してください。",
        ));
        1
    } else {
        value
    }
}

fn parse_display_name(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> String {
    match object.get("displayName") {
        None => "Coo".to_owned(),
        Some(Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                "Coo".to_owned()
            } else {
                value.to_owned()
            }
        }
        Some(_) => {
            issues.push(issue("companion.displayName", "文字列で指定してください。"));
            "Coo".to_owned()
        }
    }
}

fn parse_notification(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> NotificationConfig {
    issues.extend(unknown_keys(
        object,
        &["mode", "minPriority", "bubbleDurationMs"],
        "notification",
    ));
    let mode = enum_string(
        object,
        "mode",
        "bubble",
        &["bubble", "os", "both"],
        "notification.mode",
        issues,
    );
    let min_priority = enum_string(
        object,
        "minPriority",
        "info",
        &["info", "warning", "critical"],
        "notification.minPriority",
        issues,
    );
    NotificationConfig {
        mode,
        min_priority,
        bubble_duration_ms: positive_u64(
            object,
            "bubbleDurationMs",
            30_000,
            "notification.bubbleDurationMs",
            issues,
        ),
    }
}

fn parse_retention(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> RetentionConfig {
    issues.extend(unknown_keys(
        object,
        &["observationDays", "conversationDays"],
        "retention",
    ));
    RetentionConfig {
        observation_days: positive_u64(
            object,
            "observationDays",
            7,
            "retention.observationDays",
            issues,
        ),
        conversation_days: positive_u64(
            object,
            "conversationDays",
            30,
            "retention.conversationDays",
            issues,
        ),
    }
}

fn parse_nested<T>(
    value: Option<&Value>,
    default: T,
    path: &str,
    issues: &mut Vec<ConfigValidationIssue>,
    parser: fn(&Map<String, Value>, &mut Vec<ConfigValidationIssue>) -> T,
) -> T {
    let Some(value) = value else { return default };
    let Some(object) = value.as_object() else {
        issues.push(issue(path, "設定はオブジェクトで指定してください。"));
        return default;
    };
    parser(object, issues)
}
