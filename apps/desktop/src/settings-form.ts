import type { CompanionReminder, ConfigPatch, CooSenpaiConfig, PopupQuickAction, ProviderName, WatchAppConfig } from "./types.js";
import { changedConfigPatch } from "./settings-save.js";

export const DEFAULT_AVATAR_COLOR = "#efead8";

export interface SettingsAppearancePreview {
  readonly theme: CooSenpaiConfig["ui"]["theme"];
  readonly font: string;
  readonly avatarColor: string;
  readonly bubblePosition: CooSenpaiConfig["bubble"]["position"];
  readonly bubbleDisplay: CooSenpaiConfig["bubble"]["display"];
}

export interface FormState {
  displayName: string;
  avatarColor: string;
  avatarPath: string | null;
  avatarImage?: readonly number[];
  avatarFileName?: string;
  avatarImageLoadFailed: boolean;
  persona: string;
  watchFullscreen: boolean;
  watchApps: readonly WatchAppConfig[];
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

export const tuningHelp: Readonly<Record<string, { readonly defaultValue: string; readonly description: string }>> = {
  "watch.sendIntervalMs": { defaultValue: "60000", description: "フレームを送る最長の間隔" },
  "watch.sendDebounceMs": { defaultValue: "2000", description: "最後のフレームから送信するまでの待ち時間" },
  "watch.framesPerSend": { defaultValue: "4", description: "1回の送信にまとめる最大枚数" },
  "watch.downscaleWidth": { defaultValue: "1280", description: "AIに送る画像の縮小幅" },
  "watch.triggers.typingPauseMs": { defaultValue: "2000", description: "入力が止まったと判断するまでの時間" },
  "watch.triggers.activeThresholdMs": { defaultValue: "1000", description: "この時間未満のidleを入力中とみなす" },
  "watch.triggers.appSwitchSettleMs": { defaultValue: "1500", description: "切り替えが落ち着くまでの待ち時間" },
  "watch.triggers.maxIntervalMs": { defaultValue: "60000", description: "静かな画面でも撮影する上限間隔" },
  "watch.triggers.minSpacingMs": { defaultValue: "5000", description: "短時間の連続撮影を抑える間隔" },
  "watch.triggers.pollMs": { defaultValue: "1000", description: "入力とアプリ切り替えを確認する間隔" },
  "watch.triggers.appSwitch": { defaultValue: "有効", description: "前面アプリの切り替えを撮影のきっかけにする" },
  "watch.battery.enabled": { defaultValue: "有効", description: "バッテリー動作時の最大撮影間隔だけを延ばす" },
  "watch.battery.multiplier": { defaultValue: "2", description: "バッテリー時の最大撮影間隔への倍率" },
  "watch.ocrGate.enabled": { defaultValue: "有効", description: "画面の文字の変化を送信判断に使う" },
  "watch.ocrGate.level": { defaultValue: "accurate", description: "OCR の認識速度と精度" },
  "watch.ocrGate.timeoutMs": { defaultValue: "3000", description: "OCR helperを待つ最大時間" },
  "observer.textExcerptMaxChars": { defaultValue: "600", description: "視覚の記録に含める各テキストの上限" },
  "observer.textExcerptMaxCount": { defaultValue: "6", description: "視覚の記録に含めるテキスト箇所の数" },
  "observer.textTotalMaxChars": { defaultValue: "2000", description: "視覚の記録に含める文字全体の上限" },
  "observer.changesMaxCount": { defaultValue: "8", description: "視覚の記録に含める画面変化の数" },
  "companion.dailyProactiveLimit": { defaultValue: "無制限", description: "空欄で無制限。数値を入れると自発呼び出しの一日上限になります" },
  "companion.wakeCoalesceMax": { defaultValue: "5", description: "Coo へ一度に渡す視覚の記録の最大数" },
  "companion.sessionMaxCalls": { defaultValue: "60", description: "companion session を作り直す目安" },
  "companion.stuckAfterMs": { defaultValue: "900000", description: "変化がない状態を companion へ渡す目安" },
  "companion.pendingDeliveryLimit": { defaultValue: "20", description: "未配信 payload を保持する最大件数" },
  "companion.pendingDeliveryMaxBytes": { defaultValue: "21053440", description: "未配信 payload 全体の最大容量" },
  "companion.assertiveness": { defaultValue: "normal", description: "自分から声をかける積極性" },
};

export function toForm(config: CooSenpaiConfig, avatarImageLoadFailed: boolean): FormState {
  return {
    displayName: config.companion.displayName,
    avatarColor: config.ui.avatarColor ?? DEFAULT_AVATAR_COLOR,
    avatarPath: config.ui.avatarPath ?? null,
    avatarImageLoadFailed,
    persona: config.companion.persona,
    watchFullscreen: config.watch.fullscreen,
    watchApps: config.watch.apps.map((app) => ({ ...app })),
    audioEnabled: config.audio.enabled,
    audioMic: false,
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

function toBaseForm(source: CooSenpaiConfig): Omit<FormState, "displayName" | "avatarColor" | "avatarPath" | "avatarImage" | "avatarFileName" | "avatarImageLoadFailed" | "persona" | "watchFullscreen" | "watchApps" | "audioEnabled" | "audioMic" | "audioSpeaker" | "contextRefreshCalls" | "memoryEnabled" | "memoryProviderConsent" | "memoryGraceMinutes" | "memoryDailyRetentionDays" | "memoryWeeklyRetentionWeeks" | "sendKey" | "whileThinking" | "speechLocale" | "speechMode" | "speechConfirmBeforeSend" | "speechInputDevice" | "sendTextShortcut" | "copyLastReplyShortcut" | "textQuickActions" | "imageQuickActions" | "bubbleMaxStack" | "bubbleKeepLatest" | "bubblePosition" | "bubbleDisplay" | "uiTheme" | "uiFont" | "thoughtBubble" | "reviewTime" | "reminders" | "factPromptDailyLimit" | "launchAtLogin"> {
  const config = source;
  return { providerObserver: config.observer.provider, providerCompanion: config.companion.provider, observerModel: config.observer.model, companionModel: config.companion.model, observerEffort: config.observer.effort, companionEffort: config.companion.effort, observerExecutable: config.observer.executable ?? "", companionExecutable: config.companion.executable ?? "", captureShortcut: config.keymap.captureRegion ?? "", microphoneShortcut: config.keymap.microphone ?? "", togglePanelShortcut: config.keymap.togglePanel ?? "", toggleWatchShortcut: config.keymap.toggleWatch ?? "", assertiveness: config.companion.assertiveness, sendIntervalMs: String(config.watch.sendIntervalMs), sendDebounceMs: String(config.watch.sendDebounceMs), framesPerSend: String(config.watch.framesPerSend), downscaleWidth: String(config.watch.downscaleWidth), typingPauseMs: String(config.watch.triggers.typingPauseMs), activeThresholdMs: String(config.watch.triggers.activeThresholdMs), appSwitch: config.watch.triggers.appSwitch, appSwitchSettleMs: String(config.watch.triggers.appSwitchSettleMs), maxIntervalMs: String(config.watch.triggers.maxIntervalMs), minSpacingMs: String(config.watch.triggers.minSpacingMs), pollMs: String(config.watch.triggers.pollMs), batteryEnabled: config.watch.battery.enabled, batteryMultiplier: String(config.watch.battery.multiplier), ocrGateEnabled: config.watch.ocrGate.enabled, ocrGateLevel: config.watch.ocrGate.level, ocrGateTimeoutMs: String(config.watch.ocrGate.timeoutMs), ocrGateExecutable: config.watch.ocrGate.executable ?? "", observerTimeoutMs: String(config.observer.timeoutMs), companionTimeoutMs: String(config.companion.timeoutMs), observerLimit: String(config.observer.dailyCallLimit), observerTextExcerptMaxChars: String(config.observer.textExcerptMaxChars), observerTextExcerptMaxCount: String(config.observer.textExcerptMaxCount), observerTextTotalMaxChars: String(config.observer.textTotalMaxChars), observerChangesMaxCount: String(config.observer.changesMaxCount), companionLimit: config.companion.dailyProactiveLimit === null ? "" : String(config.companion.dailyProactiveLimit), proactiveQuietMinutes: String(config.companion.proactiveQuietMinutes), companionWakeCoalesceMax: String(config.companion.wakeCoalesceMax), companionSessionMaxCalls: String(config.companion.sessionMaxCalls), companionStuckAfterMs: String(config.companion.stuckAfterMs), pendingDeliveryLimit: String(config.companion.pendingDeliveryLimit), pendingDeliveryMaxBytes: String(config.companion.pendingDeliveryMaxBytes), notificationMode: config.notification.mode, minPriority: config.notification.minPriority, bubbleDurationMs: String(config.notification.bubbleDurationMs), showPriority: config.notification.showPriority, observationDays: String(config.retention.observationDays), conversationDays: String(config.retention.conversationDays), debugEnabled: config.debug.enabled, checkForUpdates: config.app.checkForUpdates };
}

export function toPatch(form: FormState): ConfigPatch {
  const base = toBasePatch(form);
  return {
    ...base,
    audio: { enabled: form.audioEnabled, mic: false, speaker: form.audioSpeaker },
    companion: {
      ...(base.companion as Record<string, unknown>),
      displayName: form.displayName.trim() || "Coo",
      persona: form.persona,
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
    keymap: { captureRegion: form.captureShortcut || null, microphone: form.microphoneShortcut || null, togglePanel: form.togglePanelShortcut || null, toggleWatch: form.toggleWatchShortcut || null, sendText: form.sendTextShortcut || null, copyLastReply: form.copyLastReplyShortcut || null, sendKey: form.sendKey },
    popup: { quickActions: { text: form.textQuickActions, image: form.imageQuickActions } },
    bubble: { keepLatest: form.bubbleKeepLatest, maxStack: Number(form.bubbleMaxStack), position: form.bubblePosition, display: form.bubbleDisplay },
    ui: { avatarColor: form.avatarColor.toLowerCase() === DEFAULT_AVATAR_COLOR ? null : form.avatarColor, avatarPath: form.avatarPath, theme: form.uiTheme, font: form.uiFont, thoughtBubble: form.thoughtBubble },
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

export function hasDraftChanges(config: CooSenpaiConfig, form: FormState): boolean {
  return form.avatarImage !== undefined || Object.keys(changedConfigPatch(config, toPatch(form))).length > 0;
}

export function permissionLabel(value: string): string {
  return ({ granted: "許可済み", denied: "拒否", restricted: "制限あり", unavailable: "利用不可", "not-determined": "未確認", "not-granted": "未許可", unknown: "不明" } as Record<string, string>)[value] ?? value;
}

export function audioPhaseLabel(value: string): string {
  return ({ off: "停止中", starting: "開始中", listening: "聞いています", stopping: "停止中", error: "エラー" } as Record<string, string>)[value] ?? value;
}

export function configIssueLabel(path: string): string {
  if (path.startsWith("companion.")) return "会話 AI";
  if (path.startsWith("observer.")) return "目";
  if (path.startsWith("watch.")) return "見るタイミング";
  if (path.startsWith("audio.")) return "耳 — 音を聞く";
  if (path.startsWith("memory.")) return "記憶";
  if (path.startsWith("speech.")) return "音声入力";
  if (path.startsWith("keymap.")) return "キーボード";
  if (path.startsWith("popup.")) return "送信ポップアップ";
  return "設定";
}

export function fontPreset(value: string): "system" | "rounded" | "serif" | "mono" | "custom" {
  return ["system", "rounded", "serif", "mono"].includes(value)
    ? value as "system" | "rounded" | "serif" | "mono"
    : "custom";
}

export function inputId(path: string): string { return `setting-${path.replaceAll(".", "-")}`; }
