use super::{
    issue, AgentConfig, AppConfig, AudioConfig, BatteryConfig, BubbleConfig, ChatConfig,
    CompanionConfig, Config, ConfigError, ConfigValidationIssue, DebugConfig, MemoryConfig,
    NotificationConfig, OcrGateConfig, PopupConfig, RetentionConfig, SpeechConfig, TriggerConfig,
    UiConfig, WatchConfig,
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
    boolean, effort, enum_string, executable, frames_per_send, nonnegative_u32, nonnegative_u64,
    optional_nonnegative_u32, parse_audio, parse_chat, parse_debug, parse_speech, parse_ui,
    persona, positive_number, positive_u32, positive_u64, positive_usize, provider, string,
    unknown_keys,
};
use keymap::parse_keymap;
use memory::parse_memory;
use popup::parse_popup;
use presence::{parse_app, parse_reminders, review_time};
use serde_json::{Map, Value};
use watch_apps::parse_watch_apps;

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
            "audio",
            "speech",
            "ui",
            "keymap",
            "popup",
            "app",
        ],
        "config",
    );
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
        AgentConfig::default(),
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
            audio,
            speech,
            ui,
            keymap,
            popup,
            app,
        },
        issues,
    )
}

fn parse_bubble(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> BubbleConfig {
    issues.extend(unknown_keys(
        object,
        &[
            "alwaysShow",
            "keepLatest",
            "maxStack",
            "position",
            "display",
        ],
        "bubble",
    ));
    BubbleConfig {
        always_show: boolean(object, "alwaysShow", false, "bubble.alwaysShow", issues),
        keep_latest: boolean(object, "keepLatest", false, "bubble.keepLatest", issues),
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
            "sendIntervalMs",
            "sendDebounceMs",
            "framesPerSend",
            "changeThreshold",
            "changedPixelThreshold",
            "downscaleWidth",
            "triggers",
            "battery",
            "ocrGate",
            "enabled",
            "fullscreen",
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
        fullscreen: boolean(object, "fullscreen", true, "watch.fullscreen", issues),
        apps: parse_watch_apps(object.get("apps"), issues),
        send_interval_ms: positive_u64(
            object,
            "sendIntervalMs",
            60_000,
            "watch.sendIntervalMs",
            issues,
        ),
        send_debounce_ms: positive_u64(
            object,
            "sendDebounceMs",
            2_000,
            "watch.sendDebounceMs",
            issues,
        ),
        frames_per_send: frames_per_send(object, issues),
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
) -> AgentConfig {
    issues.extend(unknown_keys(
        object,
        &[
            "provider",
            "model",
            "effort",
            "timeoutMs",
            "dailyCallLimit",
            "executable",
            "textExcerptMaxChars",
            "textExcerptMaxCount",
            "textTotalMaxChars",
            "changesMaxCount",
        ],
        "observer",
    ));
    let provider = provider(object, "provider", "codex", "observer.provider", issues);
    let model = string(object, "model", "default", "observer.model", issues);
    AgentConfig {
        provider,
        model,
        effort: effort(object, "effort", "default", "observer.effort", issues),
        executable: executable(object, "executable", "observer.executable", issues),
        timeout_ms: positive_u64(object, "timeoutMs", 120_000, "observer.timeoutMs", issues),
        daily_call_limit: nonnegative_u32(
            object,
            "dailyCallLimit",
            120,
            "observer.dailyCallLimit",
            issues,
        ),
        text_excerpt_max_chars: positive_usize(
            object,
            "textExcerptMaxChars",
            600,
            "observer.textExcerptMaxChars",
            issues,
        ),
        text_excerpt_max_count: positive_usize(
            object,
            "textExcerptMaxCount",
            6,
            "observer.textExcerptMaxCount",
            issues,
        ),
        text_total_max_chars: positive_usize(
            object,
            "textTotalMaxChars",
            2_000,
            "observer.textTotalMaxChars",
            issues,
        ),
        changes_max_count: positive_usize(
            object,
            "changesMaxCount",
            8,
            "observer.changesMaxCount",
            issues,
        ),
    }
}

fn parse_companion(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> CompanionConfig {
    issues.extend(unknown_keys(
        object,
        &[
            "provider",
            "model",
            "effort",
            "persona",
            "displayName",
            "assertiveness",
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
        ],
        "companion",
    ));
    let provider = provider(object, "provider", "codex", "companion.provider", issues);
    let model = string(object, "model", "default", "companion.model", issues);
    CompanionConfig {
        provider,
        model,
        effort: effort(object, "effort", "default", "companion.effort", issues),
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
        timeout_ms: positive_u64(object, "timeoutMs", 120_000, "companion.timeoutMs", issues),
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
    }
}

fn proactive_quiet_minutes(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> u64 {
    let value = positive_u64(
        object,
        "proactiveQuietMinutes",
        3,
        "companion.proactiveQuietMinutes",
        issues,
    );
    if value > 1_440 {
        issues.push(issue(
            "companion.proactiveQuietMinutes",
            "1以上1440以下の整数で指定してください。",
        ));
        3
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
        &["mode", "minPriority", "bubbleDurationMs", "showPriority"],
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
        show_priority: boolean(
            object,
            "showPriority",
            false,
            "notification.showPriority",
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
