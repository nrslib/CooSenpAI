import { t } from "./i18n/index.js";
import { toForm, toPatch, type FormState } from "./settings-form.js";
import type { ConfigPatch, CooSenpaiConfig, IpcResult } from "./types.js";

export type ConfigForm = Omit<FormState, "avatarImage" | "avatarFileName" | "avatarImageLoadFailed">;
export type FormFields = { readonly [K in keyof ConfigForm]: { readonly value: ConfigForm[K]; readonly patch: ConfigPatch } };

// フォーム項目を追加したら、保存先の対応がなければ型チェックを失敗させる。
const paths = {
  workApprovalMode: "work.approvalMode", workAllowedRoots: "work.allowedRoots",
  emotionsEnabled: "companion.emotionsEnabled", displayName: "companion.displayName",
  avatarColor: "ui.avatarColor", avatarPath: "ui.avatarPath", persona: "companion.persona",
  watchFullscreen: "watch.fullscreen", watchApps: "watch.apps",
  voiceOutputEnabled: "voiceOutput.enabled", voiceOutputProvider: "voiceOutput.provider",
  voiceOutputRate: "voiceOutput.rate", voicevoxStyleId: "voiceOutput.voicevoxStyleId",
  audioEnabled: "audio.enabled", audioMic: "audio.mic", audioSpeaker: "audio.speaker",
  providerObserver: "observer.provider", providerCompanion: "companion.provider",
  observerModel: "observer.model", companionModel: "companion.model",
  observerEffort: "observer.effort", companionEffort: "companion.effort",
  observerExecutable: "observer.executable", companionExecutable: "companion.executable",
  captureShortcut: "keymap.captureRegion", microphoneShortcut: "keymap.microphone",
  togglePanelShortcut: "keymap.togglePanel", toggleAvatarShortcut: "keymap.toggleAvatar", toggleWatchShortcut: "keymap.toggleWatch",
  sendTextShortcut: "keymap.sendText", copyLastReplyShortcut: "keymap.copyLastReply",
  textQuickActions: "popup.quickActions.text", imageQuickActions: "popup.quickActions.image",
  assertiveness: "companion.assertiveness",
  sendIntervalMs: "watch.sendIntervalMs", sendDebounceMs: "watch.sendDebounceMs",
  framesPerSend: "watch.framesPerSend", downscaleWidth: "watch.downscaleWidth",
  typingPauseMs: "watch.triggers.typingPauseMs", activeThresholdMs: "watch.triggers.activeThresholdMs",
  appSwitch: "watch.triggers.appSwitch", appSwitchSettleMs: "watch.triggers.appSwitchSettleMs",
  maxIntervalMs: "watch.triggers.maxIntervalMs", minSpacingMs: "watch.triggers.minSpacingMs", pollMs: "watch.triggers.pollMs",
  batteryEnabled: "watch.battery.enabled", batteryMultiplier: "watch.battery.multiplier",
  ocrGateEnabled: "watch.ocrGate.enabled", ocrGateLevel: "watch.ocrGate.level",
  ocrGateTimeoutMs: "watch.ocrGate.timeoutMs", ocrGateExecutable: "watch.ocrGate.executable",
  observerTimeoutMs: "observer.timeoutMs", companionTimeoutMs: "companion.timeoutMs",
  observerLimit: "observer.dailyCallLimit", observerTextExcerptMaxChars: "observer.textExcerptMaxChars",
  observerTextExcerptMaxCount: "observer.textExcerptMaxCount", observerTextTotalMaxChars: "observer.textTotalMaxChars",
  observerChangesMaxCount: "observer.changesMaxCount", companionLimit: "companion.dailyProactiveLimit",
  proactiveQuietMinutes: "companion.proactiveQuietMinutes", companionWakeCoalesceMax: "companion.wakeCoalesceMax",
  companionSessionMaxCalls: "companion.sessionMaxCalls", companionStuckAfterMs: "companion.stuckAfterMs",
  pendingDeliveryLimit: "companion.pendingDeliveryLimit", pendingDeliveryMaxBytes: "companion.pendingDeliveryMaxBytes",
  contextRefreshCalls: "companion.contextRefreshCalls",
  memoryEnabled: "memory.enabled", memoryProviderConsent: "memory.providerConsent",
  memoryGraceMinutes: "memory.graceMinutes", memoryDailyRetentionDays: "memory.dailyRetentionDays",
  memoryWeeklyRetentionWeeks: "memory.weeklyRetentionWeeks", factPromptDailyLimit: "memory.factPromptDailyLimit",
  sendKey: "keymap.sendKey", whileThinking: "chat.whileThinking", speechLocale: "speech.locale", speechMode: "speech.mode",
  speechConfirmBeforeSend: "speech.confirmBeforeSend", speechInputDevice: "speech.inputDevice",
  bubbleMaxStack: "bubble.maxStack", bubbleKeepLatest: "bubble.keepLatest", bubblePosition: "bubble.position", bubbleDisplay: "bubble.display",
  uiTheme: "ui.theme", uiFont: "ui.font", language: "ui.language", thoughtBubble: "ui.thoughtBubble",
  reviewTime: "companion.reviewTime", reminders: "companion.reminders",
  notificationMode: "notification.mode", minPriority: "notification.minPriority",
  bubbleDurationMs: "notification.bubbleDurationMs", showPriority: "notification.showPriority",
  observationDays: "retention.observationDays", conversationDays: "retention.conversationDays",
  debugEnabled: "debug.enabled", checkForUpdates: "app.checkForUpdates", launchAtLogin: "app.launchAtLogin",
} satisfies Record<keyof ConfigForm, string>;

export function formFields(form: FormState): FormFields {
  const normalized = toPatch(form);
  return Object.fromEntries((Object.keys(paths) as (keyof ConfigForm)[]).map((key) => {
    const path = paths[key].split(".");
    const value = path.reduce<unknown>((node, part) => (node as ConfigPatch)[part], normalized);
    if (value === undefined) throw new Error(t(form.language, "settings.missingSavePath", { path: paths[key] }));
    const patch = path.reduceRight<unknown>((leaf, part) => ({ [part]: leaf }), value) as ConfigPatch;
    return [key, { value: form[key], patch }];
  })) as FormFields;
}

export function fieldValues(fields: FormFields): ConfigForm {
  return Object.fromEntries(Object.entries(fields).map(([key, field]) => [key, field.value])) as ConfigForm;
}

export function configFields(config: CooSenpaiConfig): FormFields {
  return formFields(toForm(config, false));
}

export function configResult(result: IpcResult<CooSenpaiConfig>) {
  return result.ok ? { ...result, value: { config: result.value, fields: configFields(result.value) } } : result;
}
