import type { CompanionReminder, ConfigPatch, CooSenpaiConfig, PopupQuickAction, ProviderName, WatchAppConfig, WorkAllowedRoot } from "./types.js";
import { t, type Locale, type TranslationKey } from "./i18n/index.js";

export const DEFAULT_AVATAR_COLOR = "#efead8";

export interface SettingsAppearancePreview {
  readonly theme: CooSenpaiConfig["ui"]["theme"];
  readonly font: string;
  readonly avatarColor: string;
  readonly bubblePosition: CooSenpaiConfig["bubble"]["position"];
  readonly bubbleDisplay: CooSenpaiConfig["bubble"]["display"];
}

export interface FormState {
  workApprovalMode: "manual" | "auto";
  workAllowedRoots: readonly WorkAllowedRoot[];
  emotionsEnabled: boolean;
  displayName: string;
  avatarColor: string;
  avatarPath: string | null;
  avatarImage?: readonly number[];
  avatarFileName?: string;
  avatarImageLoadFailed: boolean;
  persona: string;
  watchFullscreen: boolean;
  watchApps: readonly WatchAppConfig[];
  voiceOutputEnabled: boolean;
  voiceOutputProvider: CooSenpaiConfig["voiceOutput"]["provider"];
  voiceOutputRate: string;
  voicevoxStyleId: number | null;
  audioEnabled: boolean;
  audioMic: boolean;
  audioSpeaker: boolean;
  providerObserver: ProviderName;
  providerCompanion: ProviderName;
  observerModel: string;
  companionModel: string;
  observerEffort: string;
  companionEffort: string;
  observerExecutable: string;
  companionExecutable: string;
  captureShortcut: string;
  microphoneShortcut: string;
  togglePanelShortcut: string;
  toggleAvatarShortcut: string;
  toggleWatchShortcut: string;
  sendTextShortcut: string;
  copyLastReplyShortcut: string;
  textQuickActions: PopupQuickAction[];
  imageQuickActions: PopupQuickAction[];
  assertiveness: "low" | "normal" | "high";
  sendIntervalMs: string;
  sendDebounceMs: string;
  framesPerSend: string;
  downscaleWidth: string;
  typingPauseMs: string;
  activeThresholdMs: string;
  appSwitch: boolean;
  appSwitchSettleMs: string;
  maxIntervalMs: string;
  minSpacingMs: string;
  pollMs: string;
  batteryEnabled: boolean;
  batteryMultiplier: string;
  ocrGateEnabled: boolean;
  ocrGateLevel: "fast" | "accurate";
  ocrGateTimeoutMs: string;
  ocrGateExecutable: string;
  observerTimeoutMs: string;
  companionTimeoutMs: string;
  observerLimit: string;
  observerTextExcerptMaxChars: string;
  observerTextExcerptMaxCount: string;
  observerTextTotalMaxChars: string;
  observerChangesMaxCount: string;
  companionLimit: string;
  proactiveQuietMinutes: string;
  companionWakeCoalesceMax: string;
  companionSessionMaxCalls: string;
  companionStuckAfterMs: string;
  pendingDeliveryLimit: string;
  pendingDeliveryMaxBytes: string;
  contextRefreshCalls: string;
  memoryEnabled: boolean;
  memoryProviderConsent: boolean;
  memoryGraceMinutes: string;
  memoryDailyRetentionDays: string;
  memoryWeeklyRetentionWeeks: string;
  sendKey: "enter" | "cmdEnter";
  whileThinking: "queue" | "append";
  speechLocale: string;
  speechMode: "pushToTalk" | "toggle";
  speechConfirmBeforeSend: boolean;
  speechInputDevice: string;
  bubbleMaxStack: string;
  bubbleKeepLatest: boolean;
  bubblePosition: "bottom-right" | "top-right" | "bottom-left" | "top-left";
  bubbleDisplay: "main" | "cursor" | "front";
  uiTheme: "system" | "light" | "dark";
  uiFont: string;
  language: "ja" | "en";
  thoughtBubble: boolean;
  reviewTime: string;
  reminders: CompanionReminder[];
  factPromptDailyLimit: string;
  notificationMode: "bubble" | "os" | "both";
  minPriority: "info" | "warning" | "critical";
  bubbleDurationMs: string;
  showPriority: boolean;
  observationDays: string;
  conversationDays: string;
  debugEnabled: boolean;
  checkForUpdates: boolean;
  launchAtLogin: boolean;
}

export type SettingsUpdate = <K extends keyof FormState>(key: K, value: FormState[K]) => void;
export type SettingsErrorFor = (path: string) => string | undefined;

const tuningDefaults = {
  sendIntervalMs: "60000", sendDebounceMs: "2000", framesPerSend: "4", downscaleWidth: "1280",
  typingPauseMs: "2000", activeThresholdMs: "1000", appSwitch: true, appSwitchSettleMs: "1500",
  maxIntervalMs: "60000", minSpacingMs: "5000", pollMs: "1000", batteryEnabled: true,
  batteryMultiplier: "2", ocrGateEnabled: true, ocrGateLevel: "accurate" as const, ocrGateTimeoutMs: "3000",
  observerTextExcerptMaxChars: "600", observerTextExcerptMaxCount: "6",
  observerTextTotalMaxChars: "2000", observerChangesMaxCount: "8",
  companionWakeCoalesceMax: "5", companionSessionMaxCalls: "60", companionStuckAfterMs: "900000",
  pendingDeliveryLimit: "20", pendingDeliveryMaxBytes: "21053440",
};

export const tuningHelp: Readonly<Record<string, { readonly defaultValue: string; readonly defaultValueKey?: TranslationKey; readonly descriptionKey: TranslationKey }>> = {
  "watch.sendIntervalMs": { defaultValue: "60000", descriptionKey: "settings.tuning.sendInterval" },
  "watch.sendDebounceMs": { defaultValue: "2000", descriptionKey: "settings.tuning.sendDebounce" },
  "watch.framesPerSend": { defaultValue: "4", descriptionKey: "settings.tuning.framesPerSend" },
  "watch.downscaleWidth": { defaultValue: "1280", descriptionKey: "settings.tuning.downscaleWidth" },
  "watch.triggers.typingPauseMs": { defaultValue: "2000", descriptionKey: "settings.tuning.typingPause" },
  "watch.triggers.activeThresholdMs": { defaultValue: "1000", descriptionKey: "settings.tuning.activeThreshold" },
  "watch.triggers.appSwitchSettleMs": { defaultValue: "1500", descriptionKey: "settings.tuning.appSwitchSettle" },
  "watch.triggers.maxIntervalMs": { defaultValue: "60000", descriptionKey: "settings.tuning.maxInterval" },
  "watch.triggers.minSpacingMs": { defaultValue: "5000", descriptionKey: "settings.tuning.minSpacing" },
  "watch.triggers.pollMs": { defaultValue: "1000", descriptionKey: "settings.tuning.poll" },
  "watch.triggers.appSwitch": { defaultValue: "enabled", defaultValueKey: "settings.tuning.enabled", descriptionKey: "settings.tuning.appSwitch" },
  "watch.battery.enabled": { defaultValue: "enabled", defaultValueKey: "settings.tuning.enabled", descriptionKey: "settings.tuning.batteryEnabled" },
  "watch.battery.multiplier": { defaultValue: "2", descriptionKey: "settings.tuning.batteryMultiplier" },
  "watch.ocrGate.enabled": { defaultValue: "enabled", defaultValueKey: "settings.tuning.enabled", descriptionKey: "settings.tuning.ocrEnabled" },
  "watch.ocrGate.level": { defaultValue: "accurate", descriptionKey: "settings.tuning.ocrLevel" },
  "watch.ocrGate.timeoutMs": { defaultValue: "3000", descriptionKey: "settings.tuning.ocrTimeout" },
  "observer.textExcerptMaxChars": { defaultValue: "600", descriptionKey: "settings.tuning.textExcerptMaxChars" },
  "observer.textExcerptMaxCount": { defaultValue: "6", descriptionKey: "settings.tuning.textExcerptMaxCount" },
  "observer.textTotalMaxChars": { defaultValue: "2000", descriptionKey: "settings.tuning.textTotalMaxChars" },
  "observer.changesMaxCount": { defaultValue: "8", descriptionKey: "settings.tuning.changesMaxCount" },
  "companion.dailyProactiveLimit": { defaultValue: "unlimited", defaultValueKey: "settings.tuning.unlimited", descriptionKey: "settings.tuning.dailyProactiveLimit" },
  "companion.wakeCoalesceMax": { defaultValue: "5", descriptionKey: "settings.tuning.wakeCoalesceMax" },
  "companion.sessionMaxCalls": { defaultValue: "60", descriptionKey: "settings.tuning.sessionMaxCalls" },
  "companion.stuckAfterMs": { defaultValue: "900000", descriptionKey: "settings.tuning.stuckAfterMs" },
  "companion.pendingDeliveryLimit": { defaultValue: "20", descriptionKey: "settings.tuning.pendingDeliveryLimit" },
  "companion.pendingDeliveryMaxBytes": { defaultValue: "21053440", descriptionKey: "settings.tuning.pendingDeliveryMaxBytes" },
  "companion.assertiveness": { defaultValue: "normal", descriptionKey: "settings.tuning.assertiveness" },
};

export function toForm(config: CooSenpaiConfig, avatarImageLoadFailed: boolean): FormState {
  return {
    workApprovalMode: config.work.approvalMode,
    workAllowedRoots: config.work.allowedRoots.map((root) => ({ ...root })),
    displayName: config.companion.displayName,
    emotionsEnabled: config.companion.emotionsEnabled,
    avatarColor: config.ui.avatarColor ?? DEFAULT_AVATAR_COLOR,
    avatarPath: config.ui.avatarPath ?? null,
    avatarImageLoadFailed,
    persona: config.companion.persona,
    watchFullscreen: config.watch.fullscreen,
    watchApps: config.watch.apps.map((app) => ({ ...app })),
    voiceOutputEnabled: config.voiceOutput.enabled,
    voiceOutputProvider: config.voiceOutput.provider,
    voiceOutputRate: String(config.voiceOutput.rate),
    voicevoxStyleId: config.voiceOutput.voicevoxStyleId,
    audioEnabled: config.audio.enabled,
    audioMic: config.audio.mic,
    audioSpeaker: config.audio.speaker,
    ...toBaseForm(config),
    contextRefreshCalls: String(config.companion.contextRefreshCalls),
    memoryEnabled: config.memory.enabled,
    memoryProviderConsent: config.memory.providerConsent,
    memoryGraceMinutes: String(config.memory.graceMinutes),
    memoryDailyRetentionDays: String(config.memory.dailyRetentionDays),
    memoryWeeklyRetentionWeeks: String(config.memory.weeklyRetentionWeeks),
    sendKey: config.keymap.sendKey,
    whileThinking: config.chat.whileThinking,
    speechLocale: config.speech.locale,
    speechMode: config.speech.mode,
    speechConfirmBeforeSend: config.speech.confirmBeforeSend,
    speechInputDevice: config.speech.inputDevice,
    sendTextShortcut: config.keymap.sendText ?? "",
    copyLastReplyShortcut: config.keymap.copyLastReply ?? "",
    textQuickActions: [...config.popup.quickActions.text],
    imageQuickActions: [...config.popup.quickActions.image],
    bubbleMaxStack: String(config.bubble.maxStack),
    bubbleKeepLatest: config.bubble.keepLatest,
    bubblePosition: config.bubble.position,
    bubbleDisplay: config.bubble.display,
    uiTheme: config.ui.theme,
    uiFont: config.ui.font,
    language: config.ui.language,
    thoughtBubble: config.ui.thoughtBubble,
    reviewTime: config.companion.reviewTime,
    reminders: [...config.companion.reminders],
    factPromptDailyLimit: String(config.memory.factPromptDailyLimit),
    notificationMode: config.notification.mode,
    minPriority: config.notification.minPriority,
    bubbleDurationMs: String(config.notification.bubbleDurationMs),
    showPriority: config.notification.showPriority,
    observationDays: String(config.retention.observationDays),
    conversationDays: String(config.retention.conversationDays),
    debugEnabled: config.debug.enabled,
    checkForUpdates: config.app.checkForUpdates,
    launchAtLogin: config.app.launchAtLogin,
  };
}

function toBaseForm(source: CooSenpaiConfig): Omit<FormState, "workApprovalMode" | "workAllowedRoots" | "voiceOutputEnabled" | "voiceOutputProvider" | "voiceOutputRate" | "voicevoxStyleId" | "emotionsEnabled" | "displayName" | "avatarColor" | "avatarPath" | "avatarImage" | "avatarFileName" | "avatarImageLoadFailed" | "persona" | "watchFullscreen" | "watchApps" | "audioEnabled" | "audioMic" | "audioSpeaker" | "contextRefreshCalls" | "memoryEnabled" | "memoryProviderConsent" | "memoryGraceMinutes" | "memoryDailyRetentionDays" | "memoryWeeklyRetentionWeeks" | "sendKey" | "whileThinking" | "speechLocale" | "speechMode" | "speechConfirmBeforeSend" | "speechInputDevice" | "sendTextShortcut" | "copyLastReplyShortcut" | "textQuickActions" | "imageQuickActions" | "bubbleMaxStack" | "bubbleKeepLatest" | "bubblePosition" | "bubbleDisplay" | "uiTheme" | "uiFont" | "language" | "thoughtBubble" | "reviewTime" | "reminders" | "factPromptDailyLimit" | "launchAtLogin"> {
  const config = source;
  return { providerObserver: config.observer.provider, providerCompanion: config.companion.provider, observerModel: config.observer.model, companionModel: config.companion.model, observerEffort: config.observer.effort, companionEffort: config.companion.effort, observerExecutable: config.observer.executable ?? "", companionExecutable: config.companion.executable ?? "", captureShortcut: config.keymap.captureRegion ?? "", microphoneShortcut: config.keymap.microphone ?? "", togglePanelShortcut: config.keymap.togglePanel ?? "", toggleAvatarShortcut: config.keymap.toggleAvatar ?? "", toggleWatchShortcut: config.keymap.toggleWatch ?? "", assertiveness: config.companion.assertiveness, sendIntervalMs: String(config.watch.sendIntervalMs), sendDebounceMs: String(config.watch.sendDebounceMs), framesPerSend: String(config.watch.framesPerSend), downscaleWidth: String(config.watch.downscaleWidth), typingPauseMs: String(config.watch.triggers.typingPauseMs), activeThresholdMs: String(config.watch.triggers.activeThresholdMs), appSwitch: config.watch.triggers.appSwitch, appSwitchSettleMs: String(config.watch.triggers.appSwitchSettleMs), maxIntervalMs: String(config.watch.triggers.maxIntervalMs), minSpacingMs: String(config.watch.triggers.minSpacingMs), pollMs: String(config.watch.triggers.pollMs), batteryEnabled: config.watch.battery.enabled, batteryMultiplier: String(config.watch.battery.multiplier), ocrGateEnabled: config.watch.ocrGate.enabled, ocrGateLevel: config.watch.ocrGate.level, ocrGateTimeoutMs: String(config.watch.ocrGate.timeoutMs), ocrGateExecutable: config.watch.ocrGate.executable ?? "", observerTimeoutMs: String(config.observer.timeoutMs), companionTimeoutMs: String(config.companion.timeoutMs), observerLimit: String(config.observer.dailyCallLimit), observerTextExcerptMaxChars: String(config.observer.textExcerptMaxChars), observerTextExcerptMaxCount: String(config.observer.textExcerptMaxCount), observerTextTotalMaxChars: String(config.observer.textTotalMaxChars), observerChangesMaxCount: String(config.observer.changesMaxCount), companionLimit: config.companion.dailyProactiveLimit === null ? "" : String(config.companion.dailyProactiveLimit), proactiveQuietMinutes: String(config.companion.proactiveQuietMinutes), companionWakeCoalesceMax: String(config.companion.wakeCoalesceMax), companionSessionMaxCalls: String(config.companion.sessionMaxCalls), companionStuckAfterMs: String(config.companion.stuckAfterMs), pendingDeliveryLimit: String(config.companion.pendingDeliveryLimit), pendingDeliveryMaxBytes: String(config.companion.pendingDeliveryMaxBytes), notificationMode: config.notification.mode, minPriority: config.notification.minPriority, bubbleDurationMs: String(config.notification.bubbleDurationMs), showPriority: config.notification.showPriority, observationDays: String(config.retention.observationDays), conversationDays: String(config.retention.conversationDays), debugEnabled: config.debug.enabled, checkForUpdates: config.app.checkForUpdates };
}

export function toPatch(form: FormState): ConfigPatch {
  const base = toBasePatch(form);
  return {
    ...base,
    work: { approvalMode: form.workApprovalMode, allowedRoots: form.workAllowedRoots },
    voiceOutput: { enabled: form.voiceOutputEnabled, provider: form.voiceOutputProvider, rate: Number(form.voiceOutputRate), voicevoxStyleId: form.voicevoxStyleId },
    audio: { enabled: form.audioEnabled, mic: form.audioMic, speaker: form.audioSpeaker },
    companion: {
      ...(base.companion as Record<string, unknown>),
      displayName: form.displayName.trim() || "Coo",
      persona: form.persona,
      emotionsEnabled: form.emotionsEnabled,
      contextRefreshCalls: Number(form.contextRefreshCalls),
      reviewTime: form.reviewTime,
      reminders: form.reminders,
      proactiveQuietMinutes: Number(form.proactiveQuietMinutes),
    },
    memory: {
      enabled: form.memoryEnabled,
      providerConsent: form.memoryProviderConsent,
      graceMinutes: Number(form.memoryGraceMinutes),
      dailyRetentionDays: Number(form.memoryDailyRetentionDays),
      weeklyRetentionWeeks: Number(form.memoryWeeklyRetentionWeeks),
      factPromptDailyLimit: Number(form.factPromptDailyLimit),
    },
    chat: { whileThinking: form.whileThinking },
    speech: { locale: form.speechLocale, mode: form.speechMode, confirmBeforeSend: form.speechConfirmBeforeSend, inputDevice: form.speechInputDevice },
    keymap: { captureRegion: form.captureShortcut || null, microphone: form.microphoneShortcut || null, togglePanel: form.togglePanelShortcut || null, toggleAvatar: form.toggleAvatarShortcut || null, toggleWatch: form.toggleWatchShortcut || null, sendText: form.sendTextShortcut || null, copyLastReply: form.copyLastReplyShortcut || null, sendKey: form.sendKey },
    popup: { quickActions: { text: form.textQuickActions, image: form.imageQuickActions } },
    bubble: { keepLatest: form.bubbleKeepLatest, maxStack: Number(form.bubbleMaxStack), position: form.bubblePosition, display: form.bubbleDisplay },
    ui: { avatarColor: form.avatarColor.toLowerCase() === DEFAULT_AVATAR_COLOR ? null : form.avatarColor, avatarPath: form.avatarPath, theme: form.uiTheme, font: form.uiFont, language: form.language, thoughtBubble: form.thoughtBubble },
    app: { checkForUpdates: form.checkForUpdates, launchAtLogin: form.launchAtLogin },
  };
}

function toBasePatch(form: FormState): ConfigPatch {
  return { watch: { fullscreen: form.watchFullscreen, apps: form.watchApps, sendIntervalMs: Number(form.sendIntervalMs), sendDebounceMs: Number(form.sendDebounceMs), framesPerSend: Number(form.framesPerSend), downscaleWidth: Number(form.downscaleWidth), triggers: { typingPauseMs: Number(form.typingPauseMs), activeThresholdMs: Number(form.activeThresholdMs), appSwitch: form.appSwitch, appSwitchSettleMs: Number(form.appSwitchSettleMs), maxIntervalMs: Number(form.maxIntervalMs), minSpacingMs: Number(form.minSpacingMs), pollMs: Number(form.pollMs) }, battery: { enabled: form.batteryEnabled, multiplier: Number(form.batteryMultiplier) }, ocrGate: { enabled: form.ocrGateEnabled, level: form.ocrGateLevel, timeoutMs: Number(form.ocrGateTimeoutMs), executable: form.ocrGateExecutable === "" ? null : form.ocrGateExecutable } }, observer: { provider: form.providerObserver, model: form.observerModel, effort: form.observerEffort, executable: form.observerExecutable === "" ? null : form.observerExecutable, timeoutMs: Number(form.observerTimeoutMs), dailyCallLimit: Number(form.observerLimit), textExcerptMaxChars: Number(form.observerTextExcerptMaxChars), textExcerptMaxCount: Number(form.observerTextExcerptMaxCount), textTotalMaxChars: Number(form.observerTextTotalMaxChars), changesMaxCount: Number(form.observerChangesMaxCount) }, companion: { provider: form.providerCompanion, model: form.companionModel, effort: form.companionEffort, executable: form.companionExecutable === "" ? null : form.companionExecutable, assertiveness: form.assertiveness, timeoutMs: Number(form.companionTimeoutMs), dailyProactiveLimit: form.companionLimit === "" ? null : Number(form.companionLimit), wakeCoalesceMax: Number(form.companionWakeCoalesceMax), sessionMaxCalls: Number(form.companionSessionMaxCalls), stuckAfterMs: Number(form.companionStuckAfterMs), pendingDeliveryLimit: Number(form.pendingDeliveryLimit), pendingDeliveryMaxBytes: Number(form.pendingDeliveryMaxBytes) }, notification: { mode: form.notificationMode, minPriority: form.minPriority, bubbleDurationMs: Number(form.bubbleDurationMs), showPriority: form.showPriority }, retention: { observationDays: Number(form.observationDays), conversationDays: Number(form.conversationDays) }, debug: { enabled: form.debugEnabled } };
}

export function defaultTuningForm(): Partial<FormState> { return { ...tuningDefaults }; }

export function tuningIsDefault(form: Pick<FormState, keyof typeof tuningDefaults>): boolean {
  return Object.entries(tuningDefaults).every(([key, value]) => form[key as keyof typeof tuningDefaults] === value);
}

export function appearancePreview(form: FormState): SettingsAppearancePreview {
  return {
    theme: form.uiTheme,
    font: form.uiFont,
    avatarColor: form.avatarColor,
    bubblePosition: form.bubblePosition,
    bubbleDisplay: form.bubbleDisplay,
  };
}

export function permissionLabel(value: string, locale: Locale = "ja"): string {
  const key = ({ granted: "view.permissionGranted", denied: "view.permissionDenied", restricted: "view.permissionRestricted", unavailable: "common.unavailable", "not-determined": "view.permissionUndetermined", "not-granted": "view.permissionNotGranted", unknown: "common.unknown" } as Record<string, TranslationKey>)[value];
  return key === undefined ? value : t(locale, key);
}

export function audioPhaseLabel(value: string, locale: Locale = "ja"): string {
  const key = ({ off: "view.audioOff", starting: "view.starting", listening: "view.listening", stopping: "view.stopped", error: "view.error" } as Record<string, TranslationKey>)[value];
  return key === undefined ? value : t(locale, key);
}

export function configIssueLabel(path: string, locale: Locale = "ja"): string {
  if (path.startsWith("companion.")) return t(locale, "settings.configIssue.companion");
  if (path.startsWith("observer.")) return t(locale, "settings.configIssue.observer");
  if (path.startsWith("watch.")) return t(locale, "settings.configIssue.watch");
  if (path.startsWith("audio.")) return t(locale, "settings.configIssue.audio");
  if (path.startsWith("memory.")) return t(locale, "settings.configIssue.memory");
  if (path.startsWith("voiceOutput.")) return t(locale, "settings.configIssue.voiceOutput");
  if (path.startsWith("speech.")) return t(locale, "settings.configIssue.speech");
  if (path.startsWith("keymap.")) return t(locale, "settings.configIssue.keymap");
  if (path.startsWith("popup.")) return t(locale, "settings.configIssue.popup");
  return t(locale, "settings.configIssue.default");
}

export function fontPreset(value: string): "system" | "rounded" | "serif" | "mono" | "custom" {
  return ["system", "rounded", "serif", "mono"].includes(value)
    ? value as "system" | "rounded" | "serif" | "mono"
    : "custom";
}

export function inputId(path: string): string { return `setting-${path.replaceAll(".", "-")}`; }
