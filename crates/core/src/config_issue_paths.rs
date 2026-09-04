use std::collections::BTreeSet;

const FIXED_ISSUE_PATH_PATTERNS: &[&str] = &[
    "app",
    "app.checkForUpdates",
    "app.launchAtLogin",
    "audio",
    "audio.debugDumpDir",
    "audio.enabled",
    "audio.mic",
    "audio.speaker",
    "bubble",
    "bubble.alwaysShow",
    "bubble.display",
    "bubble.keepLatest",
    "bubble.maxStack",
    "bubble.position",
    "chat",
    "chat.whileThinking",
    "companion",
    "companion.assertiveness",
    "companion.contextRefreshCalls",
    "companion.dailyProactiveLimit",
    "companion.displayName",
    "companion.effort",
    "companion.executable",
    "companion.model",
    "companion.pendingDeliveryLimit",
    "companion.pendingDeliveryMaxBytes",
    "companion.persona",
    "companion.proactiveQuietMinutes",
    "companion.provider",
    "companion.reminders",
    "companion.reminders[n]",
    "companion.reminders[n].id",
    "companion.reminders[n].theme",
    "companion.reminders[n].time",
    "companion.reviewTime",
    "companion.sessionMaxCalls",
    "companion.stuckAfterMs",
    "companion.timeoutMs",
    "companion.wakeCoalesceMax",
    "config",
    "configVersion",
    "debug",
    "debug.enabled",
    "keymap",
    "keymap.captureRegion",
    "keymap.copyLastReply",
    "keymap.microphone",
    "keymap.sendKey",
    "keymap.sendText",
    "keymap.togglePanel",
    "keymap.toggleWatch",
    "memory",
    "memory.candidateLimit",
    "memory.candidateMaxBytes",
    "memory.dailyRetentionDays",
    "memory.enabled",
    "memory.factLimit",
    "memory.factMaxBytes",
    "memory.factPromptDailyLimit",
    "memory.graceMinutes",
    "memory.jobRetentionDays",
    "memory.promptMaxBytes",
    "memory.providerConsent",
    "memory.sourceMaxBytes",
    "memory.storageMaxBytes",
    "memory.weeklyRetentionWeeks",
    "notification",
    "notification.bubbleDurationMs",
    "notification.minPriority",
    "notification.mode",
    "notification.showPriority",
    "observer",
    "observer.changesMaxCount",
    "observer.dailyCallLimit",
    "observer.effort",
    "observer.executable",
    "observer.model",
    "observer.provider",
    "observer.textExcerptMaxChars",
    "observer.textExcerptMaxCount",
    "observer.textTotalMaxChars",
    "observer.timeoutMs",
    "popup",
    "popup.quickActions",
    "popup.quickActions.image",
    "popup.quickActions.image[n]",
    "popup.quickActions.image[n].label",
    "popup.quickActions.image[n].message",
    "popup.quickActions.text",
    "popup.quickActions.text[n]",
    "popup.quickActions.text[n].label",
    "popup.quickActions.text[n].message",
    "retention",
    "retention.conversationDays",
    "retention.observationDays",
    "speech",
    "speech.confirmBeforeSend",
    "speech.inputDevice",
    "speech.locale",
    "speech.mode",
    "ui",
    "ui.avatarColor",
    "ui.avatarPath",
    "ui.font",
    "ui.theme",
    "ui.thoughtBubble",
    "watch",
    "watch.apps",
    "watch.apps[n]",
    "watch.apps[n].bundleId",
    "watch.apps[n].enabled",
    "watch.apps[n].name",
    "watch.battery",
    "watch.battery.enabled",
    "watch.battery.multiplier",
    "watch.downscaleWidth",
    "watch.enabled",
    "watch.framesPerSend",
    "watch.fullscreen",
    "watch.ocrGate",
    "watch.ocrGate.enabled",
    "watch.ocrGate.executable",
    "watch.ocrGate.level",
    "watch.ocrGate.timeoutMs",
    "watch.sendDebounceMs",
    "watch.sendIntervalMs",
    "watch.triggers",
    "watch.triggers.activeThresholdMs",
    "watch.triggers.appSwitch",
    "watch.triggers.appSwitchSettleMs",
    "watch.triggers.maxIntervalMs",
    "watch.triggers.minSpacingMs",
    "watch.triggers.pollMs",
    "watch.triggers.typingPauseMs",
];

const UNKNOWN_KEY_SCOPES: &[&str] = &[
    "app",
    "audio",
    "bubble",
    "chat",
    "companion",
    "companion.reminders[n]",
    "config",
    "debug",
    "keymap",
    "memory",
    "notification",
    "observer",
    "popup",
    "popup.quickActions",
    "popup.quickActions.image[n]",
    "popup.quickActions.text[n]",
    "retention",
    "speech",
    "ui",
    "watch",
    "watch.apps[n]",
    "watch.battery",
    "watch.ocrGate",
    "watch.triggers",
];

/// 設定 parser / validator が返し得る path の有限な契約。
///
/// `[n]` は任意の配列添字、`<unknown>` は未知キーを表す。
pub fn config_issue_path_patterns() -> Vec<String> {
    let mut paths = FIXED_ISSUE_PATH_PATTERNS
        .iter()
        .map(|path| (*path).to_owned())
        .collect::<BTreeSet<_>>();
    paths.extend(UNKNOWN_KEY_SCOPES.iter().map(|scope| {
        if *scope == "config" {
            "<unknown>".to_owned()
        } else {
            format!("{scope}.<unknown>")
        }
    }));
    paths.into_iter().collect()
}

pub(super) fn is_known_issue_path(path: &str) -> bool {
    let normalized = normalize_array_indices(path);
    FIXED_ISSUE_PATH_PATTERNS.contains(&normalized.as_str())
}

pub(super) fn is_known_unknown_scope(scope: &str) -> bool {
    let normalized = normalize_array_indices(scope);
    UNKNOWN_KEY_SCOPES.contains(&normalized.as_str())
}

fn normalize_array_indices(path: &str) -> String {
    let mut normalized = String::with_capacity(path.len());
    let mut characters = path.chars().peekable();
    while let Some(character) = characters.next() {
        normalized.push(character);
        if character != '[' {
            continue;
        }
        let mut saw_digit = false;
        while characters.peek().is_some_and(char::is_ascii_digit) {
            saw_digit = true;
            characters.next();
        }
        if saw_digit && characters.peek() == Some(&']') {
            normalized.push('n');
        }
    }
    normalized
}

