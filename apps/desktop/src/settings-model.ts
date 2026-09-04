import type { ProviderModelOptions, ProviderName } from "./types.js";

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
  readonly revealAdvanced: boolean;
}

const CONTROL_TARGETS = new Map<string, boolean>([
  ["companion.displayName", false], ["companion.persona", false], ["companion.assertiveness", false],
  ["companion.reviewTime", true], ["chat.whileThinking", true],
  ["observer.provider", false], ["observer.model", false], ["observer.effort", false],
  ["observer.executable", true], ["observer.timeoutMs", true], ["observer.dailyCallLimit", true],
  ["companion.provider", false], ["companion.model", false], ["companion.effort", false],
  ["companion.executable", true], ["companion.timeoutMs", true], ["companion.dailyProactiveLimit", true],
  ["companion.proactiveQuietMinutes", true],
  ["watch.sendIntervalMs", true], ["watch.sendDebounceMs", true], ["watch.framesPerSend", true],
  ["watch.downscaleWidth", true], ["watch.triggers.typingPauseMs", true],
  ["watch.triggers.activeThresholdMs", true], ["watch.triggers.appSwitch", true],
  ["watch.triggers.appSwitchSettleMs", true], ["watch.triggers.maxIntervalMs", true],
  ["watch.triggers.minSpacingMs", true], ["watch.triggers.pollMs", true],
  ["watch.battery.enabled", true], ["watch.battery.multiplier", true],
  ["watch.ocrGate.enabled", true], ["watch.ocrGate.level", true],
  ["watch.ocrGate.timeoutMs", true], ["watch.ocrGate.executable", true],
  ["observer.textExcerptMaxChars", true], ["observer.textExcerptMaxCount", true],
  ["observer.textTotalMaxChars", true], ["observer.changesMaxCount", true],
  ["companion.wakeCoalesceMax", true], ["companion.sessionMaxCalls", true],
  ["companion.stuckAfterMs", true], ["companion.pendingDeliveryLimit", true],
  ["companion.pendingDeliveryMaxBytes", true], ["companion.contextRefreshCalls", true],
  ["memory.graceMinutes", true], ["memory.dailyRetentionDays", true],
  ["memory.weeklyRetentionWeeks", true], ["memory.factPromptDailyLimit", true],
  ["memory.enabled", false], ["memory.providerConsent", false], ["watch.fullscreen", false],
  ["audio.enabled", false], ["audio.speaker", false],
  ["notification.mode", false], ["bubble.position", false], ["bubble.display", false],
  ["bubble.keepLatest", false],
  ["notification.minPriority", true], ["notification.bubbleDurationMs", true],
  ["bubble.maxStack", true], ["retention.observationDays", true],
  ["retention.conversationDays", true], ["speech.locale", true], ["speech.mode", false],
  ["speech.inputDevice", false], ["keymap.captureRegion", false], ["keymap.sendText", false],
  ["keymap.copyLastReply", false], ["keymap.microphone", false],
  ["keymap.togglePanel", false], ["keymap.toggleWatch", false], ["keymap.sendKey", false],
  ["ui.avatarColor", false], ["ui.theme", false], ["ui.font", false],
  ["ui.avatarPath", false],
  ["ui.thoughtBubble", false], ["notification.showPriority", true],
  ["speech.confirmBeforeSend", false], ["debug.enabled", true], ["app.checkForUpdates", false], ["app.launchAtLogin", false],
]);

export function settingsIssueTarget(path: string): SettingsIssueTarget {
  const advanced = CONTROL_TARGETS.get(path);
  if (advanced !== undefined) return { id: `setting-${path.replaceAll(".", "-")}`, revealAdvanced: advanced };
  if (/^popup\.quickActions\.(text|image)\[\d+\]\.(label|message)$/u.test(path)) {
    return { id: `setting-${path.replaceAll(".", "-")}`, revealAdvanced: false };
  }
  if (inPathFamily(path, "companion.reminders")) return { id: "settings-companion", revealAdvanced: true };
  if (inPathFamily(path, "companion")) return { id: "settings-companion", revealAdvanced: false };
  if (inPathFamily(path, "observer")) return { id: "settings-ai", revealAdvanced: false };
  if (inPathFamily(path, "watch.apps")) return { id: "settings-watch-targets", revealAdvanced: false };
  if (path === "watch") return { id: "settings-watch-targets", revealAdvanced: false };
  if (inPathFamily(path, "watch")) return { id: "settings-watch-detail", revealAdvanced: true };
  if (inPathFamily(path, "audio")) return { id: "settings-audio", revealAdvanced: false };
  if (inPathFamily(path, "chat")) return { id: "settings-companion", revealAdvanced: true };
  if (inPathFamily(path, "memory")) return { id: "settings-memory", revealAdvanced: path !== "memory" };
  if (inPathFamily(path, "speech")) return { id: "settings-speech", revealAdvanced: false };
  if (inPathFamily(path, "keymap")) return { id: "settings-keyboard", revealAdvanced: false };
  if (inPathFamily(path, "popup")) return { id: "settings-popup", revealAdvanced: false };
  if (inPathFamily(path, "retention")) return { id: "settings-retention", revealAdvanced: true };
  if (inPathFamily(path, "notification") || inPathFamily(path, "bubble")) {
    return { id: "settings-notification", revealAdvanced: false };
  }
  if (inPathFamily(path, "ui")) return { id: "settings-appearance", revealAdvanced: false };
  if (inPathFamily(path, "debug")) return { id: "settings-debug", revealAdvanced: true };
  return { id: "settings-app", revealAdvanced: false };
}

function inPathFamily(path: string, family: string): boolean {
  return path === family || path.startsWith(`${family}.`) || path.startsWith(`${family}[`);
}
