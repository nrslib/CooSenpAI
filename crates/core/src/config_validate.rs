use super::{
    is_valid_avatar_path, is_valid_persona_name, issue, shortcut_identity, Config, ConfigError,
    ConfigValidationIssue, CONFIG_VERSION, PENDING_DELIVERY_ITEM_MAX_BYTES,
};
use std::collections::HashSet;
use std::path::Path;

pub fn validate_config(config: &Config) -> Result<(), ConfigError> {
    let mut issues = Vec::<ConfigValidationIssue>::new();
    validate_positive_fields(config, &mut issues);
    validate_watch(config, &mut issues);
    validate_notification(config, &mut issues);
    if !(1..=10).contains(&config.bubble.max_stack) {
        issues.push(issue(
            "bubble.maxStack",
            "1以上10以下の整数で指定してください。",
        ));
    }
    if !matches!(
        config.bubble.position.as_str(),
        "bottom-right" | "top-right" | "bottom-left" | "top-left"
    ) {
        issues.push(issue(
            "bubble.position",
            "bottom-right / top-right / bottom-left / top-left のいずれかで指定してください。",
        ));
    }
    if !matches!(config.bubble.display.as_str(), "main" | "cursor" | "front") {
        issues.push(issue(
            "bubble.display",
            "main / cursor / front のいずれかで指定してください。",
        ));
    }
    validate_providers(config, &mut issues);
    validate_companion(config, &mut issues);
    validate_presence(config, &mut issues);
    validate_audio(config, &mut issues);
    validate_speech(config, &mut issues);
    validate_popup(config, &mut issues);
    validate_ui(config, &mut issues);
    if !matches!(config.chat.while_thinking.as_str(), "queue" | "append") {
        issues.push(issue(
            "chat.whileThinking",
            "queue または append で指定してください。",
        ));
    }
    validate_memory(config, &mut issues);
    validate_executables(config, &mut issues);
    validate_watch_targets(config, &mut issues);
    if config.config_version != CONFIG_VERSION {
        issues.push(issue(
            "configVersion",
            format!("{CONFIG_VERSION} で指定してください。"),
        ));
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Validation(issues))
    }
}

fn validate_popup(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    for (kind, actions) in [
        ("text", &config.popup.quick_actions.text),
        ("image", &config.popup.quick_actions.image),
    ] {
        let path = format!("popup.quickActions.{kind}");
        let mut labels = HashSet::new();
        if actions.len() > super::POPUP_QUICK_ACTION_LIMIT {
            issues.push(issue(
                &path,
                format!(
                    "{} 件以下で指定してください。",
                    super::POPUP_QUICK_ACTION_LIMIT
                ),
            ));
        }
        for (index, action) in actions.iter().enumerate() {
            let label_path = format!("{path}[{index}].label");
            let label = action.label.trim();
            let valid_label = !label.is_empty()
                && label.chars().count() <= super::POPUP_QUICK_ACTION_LABEL_MAX_CHARS
                && label.chars().all(|character| !character.is_control());
            if !valid_label {
                issues.push(issue(
                    &label_path,
                    format!(
                        "1以上{}以下の表示文字列を指定してください。",
                        super::POPUP_QUICK_ACTION_LABEL_MAX_CHARS
                    ),
                ));
            } else if !labels.insert(label) {
                issues.push(issue(
                    &label_path,
                    "同じ種類の定型文内で表示ラベルを重複させないでください。",
                ));
            }
            let message_path = format!("{path}[{index}].message");
            let message = action.message.trim();
            if message.is_empty() || message.len() > super::POPUP_QUICK_ACTION_MESSAGE_MAX_BYTES {
                issues.push(issue(
                    message_path,
                    format!(
                        "1以上{}以下の UTF-8 byte で指定してください。",
                        super::POPUP_QUICK_ACTION_MESSAGE_MAX_BYTES
                    ),
                ));
            }
        }
    }
}

fn validate_speech(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    if config.speech.locale.trim().is_empty()
        || config.speech.locale.len() > 64
        || config.speech.locale.chars().any(char::is_control)
    {
        issues.push(issue(
            "speech.locale",
            "1以上64以下のロケール識別子または system を指定してください。",
        ));
    }
    if !matches!(config.speech.mode.as_str(), "pushToTalk" | "toggle") {
        issues.push(issue(
            "speech.mode",
            "pushToTalk または toggle で指定してください。",
        ));
    }
    if config.speech.mode == "pushToTalk"
        && config
            .keymap
            .microphone
            .as_deref()
            .is_some_and(|shortcut| !push_to_talk_key_supported(shortcut))
    {
        issues.push(issue(
            "keymap.microphone",
            "pushToTalk では主キーに英数字、F1〜F20、矢印、Space などの検知可能なキーを指定してください。",
        ));
    }
    if config.speech.input_device.trim().is_empty()
        || config.speech.input_device.len() > 512
        || config.speech.input_device.chars().any(char::is_control)
    {
        issues.push(issue(
            "speech.inputDevice",
            "1以上512以下の入力デバイス ID または default を指定してください。",
        ));
    }
}

fn validate_audio(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    if config
        .audio
        .debug_dump_dir
        .as_deref()
        .is_some_and(|directory| directory.len() > 4_096 || directory.chars().any(char::is_control))
    {
        issues.push(issue(
            "audio.debugDumpDir",
            "制御文字を含まない4096 byte以下のパスまたは null を指定してください。",
        ));
    }
}

fn push_to_talk_key_supported(shortcut: &str) -> bool {
    let Some(key) = shortcut.split('+').next_back().map(str::trim) else {
        return false;
    };
    let key = key.to_ascii_uppercase();
    if key.len() == 1
        && key
            .chars()
            .next()
            .is_some_and(|value| value.is_ascii_alphanumeric() || "`\\[],=-.';/".contains(value))
    {
        return true;
    }
    matches!(
        key.as_str(),
        "BACKQUOTE"
            | "BACKSLASH"
            | "BRACKETLEFT"
            | "BRACKETRIGHT"
            | "COMMA"
            | "EQUAL"
            | "MINUS"
            | "PERIOD"
            | "QUOTE"
            | "SEMICOLON"
            | "SLASH"
            | "BACKSPACE"
            | "CAPSLOCK"
            | "ENTER"
            | "RETURN"
            | "SPACE"
            | "TAB"
            | "DELETE"
            | "END"
            | "HOME"
            | "INSERT"
            | "PAGEDOWN"
            | "PAGEUP"
            | "PRINTSCREEN"
            | "ARROWDOWN"
            | "DOWN"
            | "ARROWLEFT"
            | "LEFT"
            | "ARROWRIGHT"
            | "RIGHT"
            | "ARROWUP"
            | "UP"
            | "AUDIOVOLUMEDOWN"
            | "VOLUMEDOWN"
            | "AUDIOVOLUMEMUTE"
            | "VOLUMEMUTE"
            | "AUDIOVOLUMEUP"
            | "VOLUMEUP"
            | "NUMLOCK"
            | "NUMPAD0"
            | "NUM0"
            | "NUMPAD1"
            | "NUM1"
            | "NUMPAD2"
            | "NUM2"
            | "NUMPAD3"
            | "NUM3"
            | "NUMPAD4"
            | "NUM4"
            | "NUMPAD5"
            | "NUM5"
            | "NUMPAD6"
            | "NUM6"
            | "NUMPAD7"
            | "NUM7"
            | "NUMPAD8"
            | "NUM8"
            | "NUMPAD9"
            | "NUM9"
            | "NUMPADADD"
            | "NUMADD"
            | "NUMPADPLUS"
            | "NUMPLUS"
            | "NUMPADDECIMAL"
            | "NUMDECIMAL"
            | "NUMPADDIVIDE"
            | "NUMDIVIDE"
            | "NUMPADENTER"
            | "NUMENTER"
            | "NUMPADEQUAL"
            | "NUMEQUAL"
            | "NUMPADMULTIPLY"
            | "NUMMULTIPLY"
            | "NUMPADSUBTRACT"
            | "NUMSUBTRACT"
            | "F1"
            | "F2"
            | "F3"
            | "F4"
            | "F5"
            | "F6"
            | "F7"
            | "F8"
            | "F9"
            | "F10"
            | "F11"
            | "F12"
            | "F13"
            | "F14"
            | "F15"
            | "F16"
            | "F17"
            | "F18"
            | "F19"
            | "F20"
    )
}

fn validate_watch_targets(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    let mut bundle_ids = HashSet::new();
    for (index, app) in config.watch.apps.iter().enumerate() {
        if app.bundle_id.trim().is_empty() || app.bundle_id.chars().any(char::is_control) {
            issues.push(issue(
                format!("watch.apps[{index}].bundleId"),
                "空でない制御文字を含まない bundle ID を指定してください。",
            ));
        } else if !bundle_ids.insert(app.bundle_id.as_str()) {
            issues.push(issue(
                format!("watch.apps[{index}].bundleId"),
                "同じ bundle ID を重複して指定できません。",
            ));
        }
        if app.name.trim().is_empty() || app.name.chars().any(char::is_control) {
            issues.push(issue(
                format!("watch.apps[{index}].name"),
                "空でない制御文字を含まないアプリ名を指定してください。",
            ));
        }
    }
    if !matches!(config.keymap.send_key.as_str(), "enter" | "cmdEnter") {
        issues.push(issue(
            "keymap.sendKey",
            "enter または cmdEnter で指定してください。",
        ));
    }
    let mut shortcuts = HashSet::new();
    for (path, shortcut) in [
        ("keymap.captureRegion", &config.keymap.capture_region),
        ("keymap.microphone", &config.keymap.microphone),
        ("keymap.togglePanel", &config.keymap.toggle_panel),
        ("keymap.toggleWatch", &config.keymap.toggle_watch),
        ("keymap.sendText", &config.keymap.send_text),
        ("keymap.copyLastReply", &config.keymap.copy_last_reply),
    ] {
        if shortcut.as_ref().is_some_and(|value| {
            value.trim().is_empty() || value.len() > 64 || value.chars().any(char::is_control)
        }) {
            issues.push(issue(
                path,
                "1以上64以下のショートカット文字列または null で指定してください。",
            ));
        } else if let Some(shortcut) = shortcut {
            let Some(identity) = shortcut_identity(shortcut) else {
                issues.push(issue(
                    path,
                    "Tauri が解釈できるショートカットを指定してください。",
                ));
                continue;
            };
            if identity == "escape" || identity.ends_with("+escape") {
                issues.push(issue(
                    path,
                    "Escape は録音の取り消しに使うため設定できません。",
                ));
                continue;
            }
            if !shortcuts.insert(identity) {
                issues.push(issue(
                    path,
                    "同じショートカットを複数の操作に設定できません。",
                ));
            }
        }
    }
}

fn validate_ui(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    if config.ui.avatar_color.as_ref().is_some_and(|value| {
        value.len() != 7
            || !value.starts_with('#')
            || !value[1..]
                .chars()
                .all(|character| character.is_ascii_hexdigit())
    }) {
        issues.push(issue("ui.avatarColor", "#RRGGBB 形式で指定してください。"));
    }
    if config
        .ui
        .avatar_path
        .as_ref()
        .is_some_and(|value| !is_valid_avatar_path(value))
    {
        issues.push(issue(
            "ui.avatarPath",
            "state 配下の png / jpg / jpeg ファイルパスで指定してください。",
        ));
    }
    if !matches!(config.ui.theme.as_str(), "system" | "light" | "dark") {
        issues.push(issue(
            "ui.theme",
            "system、light、dark のいずれかで指定してください。",
        ));
    }
    if config.ui.font.trim().is_empty() || config.ui.font.chars().any(char::is_control) {
        issues.push(issue(
            "ui.font",
            "空でない制御文字を含まないフォント名で指定してください。",
        ));
    }
}

fn validate_positive_fields(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    for (path, value) in [
        ("watch.sendIntervalMs", config.watch.send_interval_ms),
        ("watch.sendDebounceMs", config.watch.send_debounce_ms),
        (
            "watch.downscaleWidth",
            u64::from(config.watch.downscale_width),
        ),
        (
            "watch.triggers.typingPauseMs",
            config.watch.triggers.typing_pause_ms,
        ),
        (
            "watch.triggers.activeThresholdMs",
            config.watch.triggers.active_threshold_ms,
        ),
        (
            "watch.triggers.appSwitchSettleMs",
            config.watch.triggers.app_switch_settle_ms,
        ),
        (
            "watch.triggers.maxIntervalMs",
            config.watch.triggers.max_interval_ms,
        ),
        (
            "watch.triggers.minSpacingMs",
            config.watch.triggers.min_spacing_ms,
        ),
        ("watch.triggers.pollMs", config.watch.triggers.poll_ms),
        ("watch.ocrGate.timeoutMs", config.watch.ocr_gate.timeout_ms),
        ("observer.timeoutMs", config.observer.timeout_ms),
        ("companion.timeoutMs", config.companion.timeout_ms),
        ("companion.stuckAfterMs", config.companion.stuck_after_ms),
        (
            "companion.proactiveQuietMinutes",
            config.companion.proactive_quiet_minutes,
        ),
        (
            "notification.bubbleDurationMs",
            config.notification.bubble_duration_ms,
        ),
        (
            "retention.observationDays",
            config.retention.observation_days,
        ),
        (
            "retention.conversationDays",
            config.retention.conversation_days,
        ),
    ] {
        if value == 0 {
            issues.push(issue(path, "正の整数で指定してください。"));
        }
    }
}

fn validate_watch(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    if !(1..=12).contains(&config.watch.frames_per_send) {
        issues.push(issue(
            "watch.framesPerSend",
            "1以上12以下の整数で指定してください。",
        ));
    }
    if config.watch.triggers.active_threshold_ms >= config.watch.triggers.typing_pause_ms {
        issues.push(issue(
            "watch.triggers.activeThresholdMs",
            "typingPauseMs より小さくしてください。",
        ));
    }
    if config.watch.triggers.poll_ms > config.watch.triggers.min_spacing_ms
        || config.watch.triggers.min_spacing_ms > config.watch.triggers.max_interval_ms
    {
        issues.push(issue(
            "watch.triggers.minSpacingMs",
            "pollMs 以上かつ maxIntervalMs 以下で指定してください。",
        ));
    }
    if !config.watch.battery.multiplier.is_finite() || config.watch.battery.multiplier <= 0.0 {
        issues.push(issue(
            "watch.battery.multiplier",
            "正の数で指定してください。",
        ));
    }
    if !matches!(config.watch.ocr_gate.level.as_str(), "fast" | "accurate") {
        issues.push(issue(
            "watch.ocrGate.level",
            "fast または accurate で指定してください。",
        ));
    }
}

fn validate_notification(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    if !matches!(config.notification.mode.as_str(), "bubble" | "os" | "both") {
        issues.push(issue(
            "notification.mode",
            "bubble、os、または both で指定してください。",
        ));
    }
    if !matches!(
        config.notification.min_priority.as_str(),
        "info" | "warning" | "critical"
    ) {
        issues.push(issue(
            "notification.minPriority",
            "info、warning、または critical で指定してください。",
        ));
    }
}

fn validate_providers(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    for (name, provider, model, effort) in [
        (
            "observer",
            &config.observer.provider,
            &config.observer.model,
            &config.observer.effort,
        ),
        (
            "companion",
            &config.companion.provider,
            &config.companion.model,
            &config.companion.effort,
        ),
    ] {
        if !matches!(provider.as_str(), "codex" | "claude" | "opencode") {
            issues.push(issue(
                format!("{name}.provider"),
                "codex、claude、opencode のいずれかで指定してください。",
            ));
        }
        if model.is_empty() {
            issues.push(issue(
                format!("{name}.model"),
                "空でない文字列で指定してください。",
            ));
        }
        if effort.trim().is_empty() {
            issues.push(issue(
                format!("{name}.effort"),
                "空白以外の文字列で指定してください。",
            ));
        }
    }
}

fn validate_companion(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    for (name, value) in [
        (
            "observer.textExcerptMaxChars",
            config.observer.text_excerpt_max_chars,
        ),
        (
            "observer.textExcerptMaxCount",
            config.observer.text_excerpt_max_count,
        ),
        (
            "observer.textTotalMaxChars",
            config.observer.text_total_max_chars,
        ),
        (
            "observer.changesMaxCount",
            config.observer.changes_max_count,
        ),
        (
            "companion.wakeCoalesceMax",
            config.companion.wake_coalesce_max,
        ),
        (
            "companion.sessionMaxCalls",
            config.companion.session_max_calls,
        ),
        (
            "companion.pendingDeliveryLimit",
            config.companion.pending_delivery_limit,
        ),
        (
            "companion.pendingDeliveryMaxBytes",
            config.companion.pending_delivery_max_bytes,
        ),
        (
            "companion.contextRefreshCalls",
            config.companion.context_refresh_calls,
        ),
    ] {
        if value == 0 {
            issues.push(issue(name, "正の整数で指定してください。"));
        }
    }
    if config.companion.pending_delivery_max_bytes < PENDING_DELIVERY_ITEM_MAX_BYTES {
        issues.push(issue(
            "companion.pendingDeliveryMaxBytes",
            format!("{PENDING_DELIVERY_ITEM_MAX_BYTES} 以上の整数で指定してください。"),
        ));
    }
    if !is_valid_persona_name(&config.companion.persona) {
        issues.push(issue("companion.persona", "性格のIDが不正です。"));
    }
    if config.companion.display_name.trim().is_empty()
        || config.companion.display_name.chars().count() > 20
        || config.companion.display_name.chars().any(char::is_control)
    {
        issues.push(issue(
            "companion.displayName",
            "1以上20以下の制御文字を含まない名前で指定してください。",
        ));
    }
    if !matches!(
        config.companion.assertiveness.as_str(),
        "low" | "normal" | "high"
    ) {
        issues.push(issue(
            "companion.assertiveness",
            "low、normal、high のいずれかで指定してください。",
        ));
    }
}

fn validate_presence(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    if !config.companion.review_time.is_empty() && !valid_hh_mm(&config.companion.review_time) {
        issues.push(issue(
            "companion.reviewTime",
            "空文字または HH:MM 形式で指定してください。",
        ));
    }
    if config.companion.reminders.len() > 10 {
        issues.push(issue("companion.reminders", "10件以下で指定してください。"));
    }
    let mut reminder_ids = HashSet::new();
    for (index, reminder) in config.companion.reminders.iter().enumerate() {
        let valid_id = !reminder.id.is_empty()
            && reminder.id.len() <= 128
            && reminder
                .id
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || value == b'-');
        if !valid_id {
            issues.push(issue(
                format!("companion.reminders[{index}].id"),
                "1以上128以下の英数字とハイフンで指定してください。",
            ));
        } else if !reminder_ids.insert(reminder.id.as_str()) {
            issues.push(issue(
                format!("companion.reminders[{index}].id"),
                "同じIDを複数の予定に指定できません。",
            ));
        }
        if !valid_hh_mm(&reminder.time) {
            issues.push(issue(
                format!("companion.reminders[{index}].time"),
                "HH:MM 形式で指定してください。",
            ));
        }
        if reminder.theme.trim().is_empty()
            || reminder.theme.chars().count() > 500
            || reminder.theme.chars().any(char::is_control)
        {
            issues.push(issue(
                format!("companion.reminders[{index}].theme"),
                "1以上500以下の制御文字を含まない文字列で指定してください。",
            ));
        }
    }
    if config.companion.proactive_quiet_minutes > 1_440 {
        issues.push(issue(
            "companion.proactiveQuietMinutes",
            "1以上1440以下の整数で指定してください。",
        ));
    }
    if config.memory.fact_prompt_daily_limit > 100 {
        issues.push(issue(
            "memory.factPromptDailyLimit",
            "0以上100以下の整数で指定してください。",
        ));
    }
}

fn valid_hh_mm(value: &str) -> bool {
    let Some((hour, minute)) = value.split_once(':') else {
        return false;
    };
    hour.len() == 2
        && minute.len() == 2
        && hour.parse::<u8>().is_ok_and(|value| value < 24)
        && minute.parse::<u8>().is_ok_and(|value| value < 60)
}

fn validate_memory(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    for (path, value, minimum, maximum) in [
        (
            "memory.dailyRetentionDays",
            config.memory.daily_retention_days,
            7,
            3_650,
        ),
        (
            "memory.weeklyRetentionWeeks",
            config.memory.weekly_retention_weeks,
            1,
            520,
        ),
        (
            "memory.jobRetentionDays",
            config.memory.job_retention_days,
            1,
            365,
        ),
    ] {
        if !(minimum..=maximum).contains(&value) {
            issues.push(issue(
                path,
                format!("{minimum}以上{maximum}以下の整数で指定してください。"),
            ));
        }
    }
    if config.memory.grace_minutes > 1_440 {
        issues.push(issue(
            "memory.graceMinutes",
            "0以上1440以下の整数で指定してください。",
        ));
    }
    for (path, value, minimum, maximum) in [
        (
            "memory.sourceMaxBytes",
            config.memory.source_max_bytes,
            16_384,
            1_048_576,
        ),
        (
            "memory.promptMaxBytes",
            config.memory.prompt_max_bytes,
            4_096,
            16_384,
        ),
        ("memory.factLimit", config.memory.fact_limit, 1, 1_000),
        (
            "memory.factMaxBytes",
            config.memory.fact_max_bytes,
            65_536,
            2_097_152,
        ),
        (
            "memory.candidateLimit",
            config.memory.candidate_limit,
            1,
            50,
        ),
        (
            "memory.candidateMaxBytes",
            config.memory.candidate_max_bytes,
            4_096,
            65_536,
        ),
    ] {
        if !(minimum..=maximum).contains(&value) {
            issues.push(issue(
                path,
                format!("{minimum}以上{maximum}以下の整数で指定してください。"),
            ));
        }
    }
    if config.memory.storage_max_bytes < 1_048_576 {
        issues.push(issue(
            "memory.storageMaxBytes",
            "1048576以上の整数で指定してください。",
        ));
    }
}

fn validate_executables(config: &Config, issues: &mut Vec<ConfigValidationIssue>) {
    for (name, executable) in [
        (
            "watch.ocrGate.executable",
            &config.watch.ocr_gate.executable,
        ),
        ("observer.executable", &config.observer.executable),
        ("companion.executable", &config.companion.executable),
    ] {
        if executable
            .as_deref()
            .is_some_and(|value| !Path::new(value).is_absolute())
        {
            issues.push(issue(
                name,
                "実行ファイルは絶対パスまたは null で指定してください。",
            ));
        }
    }
}

