import type { CompanionReminder, ConfigPatch, CooSenpaiConfig, PopupQuickAction, ProviderName, WatchAppConfig, WorkAllowedRoot } from "./types.js";
import { t, type Locale, type TranslationKey } from "./i18n/index.js";
import type { SettingsCategory } from "./settings-categories.js";

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
  watchEnabled: boolean;
  watchFullscreen: boolean;
  watchFocusElement: boolean;
  watchApps: readonly WatchAppConfig[];
  voiceOutputEnabled: boolean;
  voiceOutputProvider: CooSenpaiConfig["voiceOutput"]["provider"];
  voiceOutputRate: string;
  voicevoxStyleId: number | null;
  audioEnabled: boolean;
  audioMic: boolean;
  audioSpeaker: boolean;
  audioSpeakerIdentificationEnabled: boolean;
  audioDebugDumpDir: string;
  providerObserver: ProviderName;
  providerCompanion: ProviderName;
  observerModel: string;
  companionModel: string;
  observerEffort: string;
  companionEffort: string;
  proactiveModel: string;
  proactiveEffort: string;
  proactiveIdleMs: string;
  observerExecutable: string;
  observerIntervalMs: string;
  companionExecutable: string;
  providerHearing: ProviderName;
  hearingModel: string;
  hearingEffort: string;
  hearingExecutable: string;
  hearingIntervalMs: string;
  hearingStallTimeoutMs: string;
  hearingTimeoutMs: string;
  hearingLimit: string;
  hearingTextTotalMaxChars: string;
  hearingChangesMaxCount: string;
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
  sendDebounceMs: string;
  framesPerSend: string;
  appWindowLimit: string;
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
  observerStallTimeoutMs: string;
  companionTimeoutMs: string;
  companionStallTimeoutMs: string;
  observerLimit: string;
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
  bubbleEdgeRecall: boolean;
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
  observationDays: string;
  conversationDays: string;
  debugEnabled: boolean;
  judgeFollow: boolean;
  checkForUpdates: boolean;
  launchAtLogin: boolean;
}

export type SettingsUpdate = <K extends keyof FormState>(key: K, value: FormState[K]) => void;
export type SettingsErrorFor = (path: string) => string | undefined;

export type ConfigFormKey = keyof Omit<FormState, "avatarImage" | "avatarFileName" | "avatarImageLoadFailed">;

const pageFields: Readonly<Record<SettingsCategory, readonly ConfigFormKey[]>> = {
  work: ["workApprovalMode", "workAllowedRoots"],
  general: [
    "displayName", "avatarColor", "avatarPath", "persona", "assertiveness", "emotionsEnabled", "uiTheme", "uiFont", "language",
    "memoryEnabled", "memoryProviderConsent", "checkForUpdates", "launchAtLogin", "reviewTime", "reminders", "whileThinking",
    "memoryDailyRetentionDays", "memoryWeeklyRetentionWeeks", "factPromptDailyLimit", "observationDays", "conversationDays",
  ],
  vision: ["watchFullscreen", "watchApps", "watchFocusElement"],
  hearing: ["audioMic", "audioSpeaker", "audioSpeakerIdentificationEnabled"],
  speech: ["speechLocale", "speechInputDevice", "speechMode", "speechConfirmBeforeSend", "voiceOutputEnabled", "voiceOutputProvider", "voiceOutputRate", "voicevoxStyleId"],
  notifications: ["notificationMode", "bubblePosition", "bubbleDisplay", "bubbleEdgeRecall", "thoughtBubble", "bubbleKeepLatest", "textQuickActions", "imageQuickActions", "minPriority", "bubbleDurationMs"],
  providers: [
    "providerObserver", "observerModel", "observerEffort", "observerLimit", "providerHearing", "hearingModel", "hearingEffort", "hearingLimit", "providerCompanion", "companionModel",
    "companionEffort", "proactiveModel", "proactiveEffort", "companionLimit",
  ],
  shortcuts: ["captureShortcut", "microphoneShortcut", "togglePanelShortcut", "toggleAvatarShortcut", "toggleWatchShortcut", "sendTextShortcut", "copyLastReplyShortcut", "sendKey"],
  setup: [],
  developer: ["ocrGateExecutable", "observerExecutable", "hearingExecutable", "companionExecutable", "debugEnabled", "audioDebugDumpDir", "judgeFollow"],
};

const globalSettingsFields = ["watchEnabled", "audioEnabled"] as const satisfies readonly ConfigFormKey[];
const allSettingsFields = [
  ...globalSettingsFields,
  ...pageFields.work,
  ...pageFields.general,
  ...pageFields.vision,
  ...pageFields.hearing,
  ...pageFields.speech,
  ...pageFields.notifications,
  ...pageFields.providers,
  ...pageFields.shortcuts,
  ...pageFields.setup,
  ...pageFields.developer,
] as const satisfies readonly ConfigFormKey[];

export type SettingsResetRequest =
  | { readonly scope: "page"; readonly category: SettingsCategory }
  | { readonly scope: "all" };

export const tuningHelp: Readonly<Record<string, { readonly defaultValue: string; readonly defaultValueKey?: TranslationKey; readonly descriptionKey: TranslationKey }>> = {
  "watch.focusElement": { defaultValue: "disabled", defaultValueKey: "common.disabled", descriptionKey: "settings.tuning.focusElement" },
  "companion.dailyProactiveLimit": { defaultValue: "unlimited", defaultValueKey: "settings.tuning.unlimited", descriptionKey: "settings.tuning.dailyProactiveLimit" },
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
    watchEnabled: config.watch.enabled,
    watchFullscreen: config.watch.fullscreen,
    watchFocusElement: config.watch.focusElement,
    watchApps: config.watch.apps.map((app) => ({ ...app })),
    voiceOutputEnabled: config.voiceOutput.enabled,
    voiceOutputProvider: config.voiceOutput.provider,
    voiceOutputRate: String(config.voiceOutput.rate),
    voicevoxStyleId: config.voiceOutput.voicevoxStyleId,
    audioEnabled: config.audio.enabled,
    audioMic: config.audio.mic,
    audioSpeaker: config.audio.speaker,
    audioSpeakerIdentificationEnabled: config.audio.speakerIdentification.enabled,
    audioDebugDumpDir: config.audio.debugDumpDir ?? "",
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
    bubbleEdgeRecall: config.bubble.edgeRecall,
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
    observationDays: String(config.retention.observationDays),
    conversationDays: String(config.retention.conversationDays),
    debugEnabled: config.debug.enabled,
    judgeFollow: config.judge?.follow ?? false,
    checkForUpdates: config.app.checkForUpdates,
    launchAtLogin: config.app.launchAtLogin,
  };
}

function toBaseForm(source: CooSenpaiConfig) {
  const vision = source.observer.vision;
  const hearing = source.observer.hearing;
  return {
    providerObserver: vision.provider,
    providerCompanion: source.companion.provider,
    observerModel: vision.model,
    companionModel: source.companion.model,
    observerEffort: vision.effort,
    companionEffort: source.companion.effort,
    proactiveModel: source.companion.proactiveModel,
    proactiveEffort: source.companion.proactiveEffort,
    proactiveIdleMs: String(source.companion.proactiveIdleMs),
    observerExecutable: vision.executable ?? "",
    observerIntervalMs: String(vision.intervalMs),
    companionExecutable: source.companion.executable ?? "",
    providerHearing: hearing.provider,
    hearingModel: hearing.model,
    hearingEffort: hearing.effort,
    hearingExecutable: hearing.executable ?? "",
    hearingIntervalMs: String(hearing.intervalMs),
    hearingStallTimeoutMs: String(hearing.stallTimeoutMs),
    hearingTimeoutMs: String(hearing.timeoutMs),
    hearingLimit: String(hearing.dailyCallLimit),
    hearingTextTotalMaxChars: String(hearing.textTotalMaxChars),
    hearingChangesMaxCount: String(hearing.changesMaxCount),
    captureShortcut: source.keymap.captureRegion ?? "",
    microphoneShortcut: source.keymap.microphone ?? "",
    togglePanelShortcut: source.keymap.togglePanel ?? "",
    toggleAvatarShortcut: source.keymap.toggleAvatar ?? "",
    toggleWatchShortcut: source.keymap.toggleWatch ?? "",
    assertiveness: source.companion.assertiveness,
    sendDebounceMs: String(source.watch.sendDebounceMs),
    framesPerSend: String(source.watch.framesPerSend),
    appWindowLimit: String(source.watch.appWindowLimit),
    downscaleWidth: String(source.watch.downscaleWidth),
    typingPauseMs: String(source.watch.triggers.typingPauseMs),
    activeThresholdMs: String(source.watch.triggers.activeThresholdMs),
    appSwitch: source.watch.triggers.appSwitch,
    appSwitchSettleMs: String(source.watch.triggers.appSwitchSettleMs),
    maxIntervalMs: String(source.watch.triggers.maxIntervalMs),
    minSpacingMs: String(source.watch.triggers.minSpacingMs),
    pollMs: String(source.watch.triggers.pollMs),
    batteryEnabled: source.watch.battery.enabled,
    batteryMultiplier: String(source.watch.battery.multiplier),
    ocrGateEnabled: source.watch.ocrGate.enabled,
    ocrGateLevel: source.watch.ocrGate.level,
    ocrGateTimeoutMs: String(source.watch.ocrGate.timeoutMs),
    ocrGateExecutable: source.watch.ocrGate.executable ?? "",
    observerTimeoutMs: String(vision.timeoutMs),
    observerStallTimeoutMs: String(vision.stallTimeoutMs),
    companionTimeoutMs: String(source.companion.timeoutMs),
    companionStallTimeoutMs: String(source.companion.stallTimeoutMs),
    observerLimit: String(vision.dailyCallLimit),
    observerTextTotalMaxChars: String(vision.textTotalMaxChars),
    observerChangesMaxCount: String(vision.changesMaxCount),
    companionLimit: source.companion.dailyProactiveLimit === null ? "" : String(source.companion.dailyProactiveLimit),
    proactiveQuietMinutes: String(source.companion.proactiveQuietMinutes),
    companionWakeCoalesceMax: String(source.companion.wakeCoalesceMax),
    companionSessionMaxCalls: String(source.companion.sessionMaxCalls),
    companionStuckAfterMs: String(source.companion.stuckAfterMs),
    pendingDeliveryLimit: String(source.companion.pendingDeliveryLimit),
    pendingDeliveryMaxBytes: String(source.companion.pendingDeliveryMaxBytes),
    notificationMode: source.notification.mode,
    minPriority: source.notification.minPriority,
    bubbleDurationMs: String(source.notification.bubbleDurationMs),
    observationDays: String(source.retention.observationDays),
    conversationDays: String(source.retention.conversationDays),
    debugEnabled: source.debug.enabled,
    checkForUpdates: source.app.checkForUpdates,
  };
}

export function toPatch(form: FormState): ConfigPatch {
  const base = toBasePatch(form);
  return {
    ...base,
    work: { approvalMode: form.workApprovalMode, allowedRoots: form.workAllowedRoots },
    voiceOutput: { enabled: form.voiceOutputEnabled, provider: form.voiceOutputProvider, rate: Number(form.voiceOutputRate), voicevoxStyleId: form.voicevoxStyleId },
    audio: {
      enabled: form.audioEnabled,
      mic: form.audioMic,
      speaker: form.audioSpeaker,
      speakerIdentification: { enabled: form.audioSpeakerIdentificationEnabled },
      debugDumpDir: form.audioDebugDumpDir === "" ? null : form.audioDebugDumpDir,
    },
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
    judge: { follow: form.judgeFollow ?? false },
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
    bubble: { keepLatest: form.bubbleKeepLatest, edgeRecall: form.bubbleEdgeRecall, maxStack: Number(form.bubbleMaxStack), position: form.bubblePosition, display: form.bubbleDisplay },
    ui: { avatarColor: form.avatarColor.toLowerCase() === DEFAULT_AVATAR_COLOR ? null : form.avatarColor, avatarPath: form.avatarPath, theme: form.uiTheme, font: form.uiFont, language: form.language, thoughtBubble: form.thoughtBubble },
    app: { checkForUpdates: form.checkForUpdates, launchAtLogin: form.launchAtLogin },
  };
}

function toBasePatch(form: FormState): ConfigPatch {
  return {
    watch: {
      enabled: form.watchEnabled,
      fullscreen: form.watchFullscreen,
      focusElement: form.watchFocusElement,
      apps: form.watchApps,
      sendDebounceMs: Number(form.sendDebounceMs),
      framesPerSend: Number(form.framesPerSend),
      appWindowLimit: Number(form.appWindowLimit),
      downscaleWidth: Number(form.downscaleWidth),
      triggers: {
        typingPauseMs: Number(form.typingPauseMs),
        activeThresholdMs: Number(form.activeThresholdMs),
        appSwitch: form.appSwitch,
        appSwitchSettleMs: Number(form.appSwitchSettleMs),
        maxIntervalMs: Number(form.maxIntervalMs),
        minSpacingMs: Number(form.minSpacingMs),
        pollMs: Number(form.pollMs),
      },
      battery: { enabled: form.batteryEnabled, multiplier: Number(form.batteryMultiplier) },
      ocrGate: {
        enabled: form.ocrGateEnabled,
        level: form.ocrGateLevel,
        timeoutMs: Number(form.ocrGateTimeoutMs),
        executable: form.ocrGateExecutable === "" ? null : form.ocrGateExecutable,
      },
    },
    observer: {
      vision: {
        provider: form.providerObserver,
        model: form.observerModel,
        effort: form.observerEffort,
        executable: form.observerExecutable === "" ? null : form.observerExecutable,
        intervalMs: Number(form.observerIntervalMs),
        stallTimeoutMs: Number(form.observerStallTimeoutMs),
        timeoutMs: Number(form.observerTimeoutMs),
        dailyCallLimit: Number(form.observerLimit),
        textTotalMaxChars: Number(form.observerTextTotalMaxChars),
        changesMaxCount: Number(form.observerChangesMaxCount),
      },
      hearing: {
        provider: form.providerHearing,
        model: form.hearingModel,
        effort: form.hearingEffort,
        executable: form.hearingExecutable === "" ? null : form.hearingExecutable,
        intervalMs: Number(form.hearingIntervalMs),
        stallTimeoutMs: Number(form.hearingStallTimeoutMs),
        timeoutMs: Number(form.hearingTimeoutMs),
        dailyCallLimit: Number(form.hearingLimit),
        textTotalMaxChars: Number(form.hearingTextTotalMaxChars),
        changesMaxCount: Number(form.hearingChangesMaxCount),
      },
    },
    companion: {
      provider: form.providerCompanion,
      model: form.companionModel,
      effort: form.companionEffort,
      proactiveModel: form.proactiveModel,
      proactiveEffort: form.proactiveEffort,
      proactiveIdleMs: Number(form.proactiveIdleMs),
      executable: form.companionExecutable === "" ? null : form.companionExecutable,
      assertiveness: form.assertiveness,
      stallTimeoutMs: Number(form.companionStallTimeoutMs),
      timeoutMs: Number(form.companionTimeoutMs),
      dailyProactiveLimit: form.companionLimit === "" ? null : Number(form.companionLimit),
      wakeCoalesceMax: Number(form.companionWakeCoalesceMax),
      sessionMaxCalls: Number(form.companionSessionMaxCalls),
      stuckAfterMs: Number(form.companionStuckAfterMs),
      pendingDeliveryLimit: Number(form.pendingDeliveryLimit),
      pendingDeliveryMaxBytes: Number(form.pendingDeliveryMaxBytes),
    },
    notification: { mode: form.notificationMode, minPriority: form.minPriority, bubbleDurationMs: Number(form.bubbleDurationMs) },
    retention: { observationDays: Number(form.observationDays), conversationDays: Number(form.conversationDays) },
    debug: { enabled: form.debugEnabled },
  };
}

const RESET_EXCLUDED_KEYS: ReadonlySet<ConfigFormKey> = new Set(["avatarPath", "persona"]);

function resettableFields(fields: readonly ConfigFormKey[]): readonly ConfigFormKey[] {
  return fields.filter((key) => !RESET_EXCLUDED_KEYS.has(key));
}

export function settingsResetFields(request: SettingsResetRequest): readonly ConfigFormKey[] {
  switch (request.scope) {
    case "page": return resettableFields(pageFields[request.category]);
    case "all": return resettableFields(allSettingsFields);
  }
}

export function resetSettingsForm(form: FormState, defaults: CooSenpaiConfig, request: SettingsResetRequest): FormState {
  const defaultForm = toForm(defaults, form.avatarImageLoadFailed);
  const keys = settingsResetFields(request);
  const next = { ...form };
  for (const key of keys) Object.assign(next, { [key]: defaultForm[key] });
  return next;
}

export type ResetFormValues = Partial<Pick<FormState, ConfigFormKey>>;

export function sameFormValue(left: unknown, right: unknown): boolean {
  return Object.is(left, right) || JSON.stringify(left) === JSON.stringify(right);
}

export function restoreSettingsForm(form: FormState, previous: ResetFormValues, defaults: CooSenpaiConfig, request: SettingsResetRequest, excludedKeys: ReadonlySet<ConfigFormKey> = new Set()): FormState {
  const resetForm = resetSettingsForm(form, defaults, request);
  const next = { ...form };
  for (const key of settingsResetFields(request)) {
    if (!excludedKeys.has(key) && Object.prototype.hasOwnProperty.call(previous, key) && sameFormValue(form[key], resetForm[key])) {
      Object.assign(next, { [key]: previous[key] });
    }
  }
  return next;
}

export function defaultValueForPath(config: CooSenpaiConfig | undefined, path: string, locale: Locale = "ja"): string | undefined {
  if (config === undefined) return undefined;
  let value: unknown = config;
  for (const part of path.split(".")) {
    if (typeof value !== "object" || value === null || !(part in value)) return undefined;
    value = (value as Record<string, unknown>)[part];
  }
  if (typeof value === "boolean") return t(locale, value ? "common.enabled" : "common.disabled");
  if (typeof value === "number" || typeof value === "string") return String(value);
  if (value === null && path === "companion.dailyProactiveLimit") return t(locale, "settings.tuning.unlimited");
  return undefined;
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
  const key = ({ granted: "view.permissionGranted", denied: "view.permissionDenied", restricted: "view.permissionRestricted", unavailable: "common.unavailable", "not-determined": "view.permissionUndetermined", "not-granted": "view.permissionNotGranted", "not-required": "view.permissionNotRequired", unknown: "common.unknown" } as Record<string, TranslationKey>)[value];
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
  if (path.startsWith("judge.")) return t(locale, "settings.configIssue.judge");
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
