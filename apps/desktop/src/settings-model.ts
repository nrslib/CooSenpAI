import type { ProviderModelOptions, ProviderName } from "./types.js";

export const CONFIG_REVISION_CONFLICT_MESSAGE = "設定が別の場所で変更されました。読み直してください";

export function isConfigRevisionConflict(message: string): boolean {
  return message === CONFIG_REVISION_CONFLICT_MESSAGE;
}

export function settingsIssueHeading(hasSaveIssues: boolean): string {
  return hasSaveIssues ? "設定を保存できませんでした" : "設定を確認してください";
}

export function modelAfterProviderChange(
  provider: ProviderName,
  options: readonly ProviderModelOptions[],
): string | undefined {
  return options.find((option) => option.provider === provider)?.defaultModel;
}

export function unavailableProviderMessage(detail?: string): string {
  return detail ?? "provider bridge から model 一覧を取得できていません";
}

export interface SettingsIssueTarget {
  readonly id: string;
}

const CONTROL_PATHS = new Set<string>([
  "companion.displayName", "companion.persona", "companion.assertiveness",
  "companion.reviewTime", "chat.whileThinking",
  "observer.provider", "observer.model", "observer.effort",
  "observer.executable", "observer.timeoutMs", "observer.dailyCallLimit",
  "companion.provider", "companion.model", "companion.effort",
  "companion.executable", "companion.timeoutMs", "companion.dailyProactiveLimit",
  "companion.proactiveQuietMinutes",
  "watch.sendIntervalMs", "watch.sendDebounceMs", "watch.framesPerSend",
  "watch.downscaleWidth", "watch.triggers.typingPauseMs",
  "watch.triggers.activeThresholdMs", "watch.triggers.appSwitch",
  "watch.triggers.appSwitchSettleMs", "watch.triggers.maxIntervalMs",
  "watch.triggers.minSpacingMs", "watch.triggers.pollMs",
  "watch.battery.enabled", "watch.battery.multiplier",
  "watch.ocrGate.enabled", "watch.ocrGate.level",
  "watch.ocrGate.timeoutMs", "watch.ocrGate.executable",
  "observer.textExcerptMaxChars", "observer.textExcerptMaxCount",
  "observer.textTotalMaxChars", "observer.changesMaxCount",
  "companion.wakeCoalesceMax", "companion.sessionMaxCalls",
  "companion.stuckAfterMs", "companion.pendingDeliveryLimit",
  "companion.pendingDeliveryMaxBytes", "companion.contextRefreshCalls",
  "memory.graceMinutes", "memory.dailyRetentionDays",
  "memory.weeklyRetentionWeeks", "memory.factPromptDailyLimit",
  "memory.enabled", "memory.providerConsent", "watch.fullscreen",
  "audio.enabled", "audio.speaker",
  "notification.mode", "bubble.position", "bubble.display",
  "bubble.keepLatest",
  "notification.minPriority", "notification.bubbleDurationMs",
  "bubble.maxStack", "retention.observationDays",
  "retention.conversationDays", "speech.locale", "speech.mode",
  "speech.inputDevice", "keymap.captureRegion", "keymap.sendText",
  "keymap.copyLastReply", "keymap.microphone",
  "keymap.togglePanel", "keymap.toggleWatch", "keymap.sendKey",
  "ui.avatarColor", "ui.theme", "ui.font",
  "ui.avatarPath",
  "ui.thoughtBubble", "notification.showPriority",
  "speech.confirmBeforeSend", "debug.enabled", "app.checkForUpdates", "app.launchAtLogin",
]);

export function settingsIssueTarget(path: string): SettingsIssueTarget {
  if (CONTROL_PATHS.has(path)) return { id: `setting-${path.replaceAll(".", "-")}` };
  if (/^popup\.quickActions\.(text|image)\[\d+\]\.(label|message)$/u.test(path)) {
    return { id: `setting-${path.replaceAll(".", "-")}` };
  }
  if (inPathFamily(path, "companion.reminders")) return { id: "settings-general-detail" };
  if (inPathFamily(path, "companion")) return { id: "settings-companion" };
  if (inPathFamily(path, "observer")) return { id: "settings-ai" };
  if (inPathFamily(path, "watch.apps")) return { id: "settings-watch-targets" };
  if (path === "watch") return { id: "settings-watch-targets" };
  if (inPathFamily(path, "watch")) return { id: "settings-watch-detail" };
  if (inPathFamily(path, "audio")) return { id: "settings-audio" };
  if (inPathFamily(path, "chat")) return { id: "settings-general-detail" };
  if (inPathFamily(path, "memory")) return { id: "settings-memory" };
  if (inPathFamily(path, "speech")) return { id: "settings-speech" };
  if (inPathFamily(path, "keymap")) return { id: "settings-keyboard" };
  if (inPathFamily(path, "popup")) return { id: "settings-popup" };
  if (inPathFamily(path, "retention")) return { id: "settings-retention" };
  if (inPathFamily(path, "notification") || inPathFamily(path, "bubble")) {
    return { id: "settings-notification" };
  }
  if (inPathFamily(path, "ui")) return { id: "settings-appearance" };
  if (inPathFamily(path, "debug")) return { id: "settings-debug" };
  return { id: "settings-app" };
}

function inPathFamily(path: string, family: string): boolean {
  return path === family || path.startsWith(`${family}.`) || path.startsWith(`${family}[`);
}
