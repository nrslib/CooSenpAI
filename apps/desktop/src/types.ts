export type ProviderName = "codex" | "claude" | "opencode";
export type NotificationPriority = "none" | "info" | "warning" | "critical";
export interface PersonaOption { readonly id: string; readonly displayName: string; readonly builtin: boolean }
export interface ProviderModelOptions { readonly provider: ProviderName; readonly defaultModel: string; readonly candidates: readonly string[] }
export interface ModelCatalogProvider {
  readonly provider: ProviderName;
  readonly defaultModel: string;
  readonly candidates: readonly string[];
  readonly history: readonly string[];
  readonly efforts: readonly string[];
  readonly modelEfforts: Readonly<Record<string, readonly string[]>>;
}
export interface CompanionModelCatalog {
  readonly providers: readonly ModelCatalogProvider[];
  readonly opencodeError?: string;
}
export interface ProviderApiKeyStatus { readonly codex: boolean; readonly claude: boolean; readonly opencode: boolean }

export interface CooSenpaiConfig {
  readonly configVersion: number;
  readonly revision: number;
  readonly app: { readonly launchAtLogin: boolean; readonly checkForUpdates: boolean };
  readonly watch: {
    readonly enabled: boolean;
    readonly fullscreen: boolean;
    readonly apps: readonly WatchAppConfig[];
    readonly sendIntervalMs: number;
    readonly sendDebounceMs: number;
    readonly framesPerSend: number;
    readonly downscaleWidth: number;
    readonly triggers: {
      readonly typingPauseMs: number;
      readonly activeThresholdMs: number;
      readonly appSwitch: boolean;
      readonly appSwitchSettleMs: number;
      readonly maxIntervalMs: number;
      readonly minSpacingMs: number;
      readonly pollMs: number;
    };
    readonly battery: { readonly enabled: boolean; readonly multiplier: number };
    readonly ocrGate: { readonly enabled: boolean; readonly level: "fast" | "accurate"; readonly timeoutMs: number; readonly executable?: string | null };
  };
  readonly audio: { readonly enabled: boolean; readonly mic: boolean; readonly speaker: boolean };
  readonly speech: {
    readonly locale: string;
    readonly mode: "pushToTalk" | "toggle";
    readonly confirmBeforeSend: boolean;
    readonly inputDevice: string;
  };
  readonly ui: {
    readonly avatarColor?: string | null;
    readonly avatarPath?: string | null;
    readonly theme: "system" | "light" | "dark";
    readonly font: string;
    readonly thoughtBubble: boolean;
  };
  readonly keymap: {
    readonly captureRegion?: string | null;
    readonly microphone?: string | null;
    readonly togglePanel?: string | null;
    readonly toggleWatch?: string | null;
    readonly sendText?: string | null;
    readonly copyLastReply?: string | null;
    readonly sendKey: "enter" | "cmdEnter";
  };
  readonly popup: {
    readonly quickActions: {
      readonly text: readonly PopupQuickAction[];
      readonly image: readonly PopupQuickAction[];
    };
  };
  readonly bubble: {
    readonly alwaysShow: boolean;
    readonly keepLatest: boolean;
    readonly maxStack: number;
    readonly position: "bottom-right" | "top-right" | "bottom-left" | "top-left";
    readonly display: "main" | "cursor" | "front";
  };
  readonly observer: AgentConfig;
  readonly companion: CompanionConfig;
  readonly chat: { readonly whileThinking: "queue" | "append" };
  readonly memory: MemoryConfig;
  readonly debug: { readonly enabled: boolean };
  readonly notification: {
    readonly mode: "bubble" | "os" | "both";
    readonly minPriority: "info" | "warning" | "critical";
    readonly bubbleDurationMs: number;
    readonly showPriority: boolean;
  };
  readonly retention: { readonly observationDays: number; readonly conversationDays: number };
}

export interface WatchAppConfig {
  readonly bundleId: string;
  readonly name: string;
  readonly enabled: boolean;
}

export interface RunningApplication {
  readonly bundleId: string;
  readonly name: string;
  readonly iconPng: readonly number[];
}

export interface AgentConfig {
  readonly provider: ProviderName;
  readonly model: string;
  readonly effort: string;
  readonly executable?: string | null;
  readonly timeoutMs: number;
  readonly dailyCallLimit: number;
  readonly textExcerptMaxChars: number;
  readonly textExcerptMaxCount: number;
  readonly textTotalMaxChars: number;
  readonly changesMaxCount: number;
}

export interface CompanionConfig {
  readonly provider: ProviderName;
  readonly model: string;
  readonly effort: string;
  readonly executable?: string | null;
  readonly persona: string;
  readonly displayName: string;
  readonly assertiveness: "low" | "normal" | "high";
  readonly timeoutMs: number;
  readonly dailyProactiveLimit: number | null;
  readonly wakeCoalesceMax: number;
  readonly sessionMaxCalls: number;
  readonly stuckAfterMs: number;
  readonly pendingDeliveryLimit: number;
  readonly pendingDeliveryMaxBytes: number;
  readonly contextRefreshCalls: number;
  readonly reviewTime: string;
  readonly reminders: readonly CompanionReminder[];
  readonly proactiveQuietMinutes: number;
}

export interface CompanionReminder { readonly id: string; readonly time: string; readonly theme: string }

export interface MemoryConfig {
  readonly enabled: boolean;
  readonly providerConsent: boolean;
  readonly graceMinutes: number;
  readonly dailyRetentionDays: number;
  readonly weeklyRetentionWeeks: number;
  readonly jobRetentionDays: number;
  readonly sourceMaxBytes: number;
  readonly promptMaxBytes: number;
  readonly factLimit: number;
  readonly factMaxBytes: number;
  readonly candidateLimit: number;
  readonly candidateMaxBytes: number;
  readonly storageMaxBytes: number;
  readonly factPromptDailyLimit: number;
}

export interface ConversationEntry {
  readonly schemaVersion: number;
  readonly id: string;
  readonly createdAt: string;
  readonly role: "user" | "companion";
  readonly message: string;
  readonly attachmentPath?: string;
  readonly attachmentText?: string;
  readonly tutorialResponseKey?: string;
  readonly causedByIds?: readonly string[];
  readonly notificationPriority: NotificationPriority;
}

export interface ObservationSummary {
  readonly kind: "visual" | "no-change" | "audio";
  readonly id: string;
  readonly createdAt: string;
  readonly activity?: string;
  readonly changes?: readonly string[];
  readonly wakeCompanion?: boolean;
  readonly source?: "microphone" | "speaker";
  readonly text?: string;
}

export interface VisualObservationSummary extends ObservationSummary {
  readonly kind: "visual";
  readonly activity: string;
}

export interface RuntimeLastError {
  readonly kind: string;
  readonly occurredAt: string;
  readonly message?: string;
  readonly issues?: readonly ConfigIssue[];
  readonly attachmentOcr?: {
    readonly inputId: string;
    readonly reason: "capability" | "helper-unavailable" | "recognition" | "no-text";
    readonly attempts: number;
    readonly retryable: boolean;
  };
}

export interface AppSnapshot {
  readonly revision: number;
  readonly configRevision: number;
  readonly config: CooSenpaiConfig;
  readonly observerRunning: boolean;
  readonly watchIntentActive: boolean;
  readonly observer: {
    readonly phase: "stopped" | "idle" | "capturing" | "thinking" | "suspended" | "error";
    readonly aiCallsToday: number;
    readonly lastCapturedAt?: string;
    readonly lastTrigger?: "typing-paused" | "app-switched" | "timer";
    readonly lastCaptureDisposition?: string;
    readonly frontApp?: string;
    readonly pendingFrameCount: number;
    readonly nextSendAt?: string;
    readonly lastObservation?: ObservationSummary;
    readonly lastVisualObservation?: VisualObservationSummary;
    readonly ocrGateEnabled: boolean;
    readonly activitySignalsEnabled: boolean;
    readonly batteryMultiplier: number;
    readonly errorMessage?: string;
    readonly targets: readonly WatchTargetView[];
  };
  readonly companion: { readonly phase: "idle" | "thinking" | "error"; readonly ready: boolean; readonly totalCallsToday: number; readonly proactiveLimitReached: boolean };
  readonly notify: { readonly mode: string; readonly minimumPriority: string };
  readonly observerProviderLabel: string;
  readonly companionProviderLabel: string;
  readonly companionDisplayName: string;
  readonly temporaryAssertiveness?: {
    readonly value: "low" | "normal" | "high";
    readonly expiresAt: string;
  };
  readonly conversation: readonly ConversationEntry[];
  readonly unreadCount: number;
  readonly screenRecordingStatus: "granted" | "not-granted" | "unknown";
  readonly screenRecordingMessage?: string;
  readonly screenRecordingRestartRequired: boolean;
  readonly signedBuild: boolean;
  readonly lastError?: RuntimeLastError;
  readonly companionRetryInSeconds?: number;
  readonly pendingDeliveries: number;
  readonly deliveryOutboxBlocked: boolean;
  readonly memoryStatus: MemoryStatus;
  readonly debugCatalog: DebugCatalog;
  readonly captureShortcutError?: string;
  readonly activeUserMessageId?: string;
  readonly cancelledUserMessageIds: readonly string[];
  readonly companionDraft?: string;
  readonly latestCompanionThought?: string;
  readonly avatarImagePng?: readonly number[];
  readonly avatarImageLoadFailed: boolean;
  readonly providerUsage: {
    readonly callId?: string;
    readonly provider?: "codex" | "claude" | "opencode";
    readonly model?: string;
    readonly inputTokens?: number;
    readonly cachedInputTokens?: number;
    readonly outputTokens?: number;
    readonly totalTokens?: number;
  };
  readonly speech: SpeechView;
  readonly audio: AudioView;
  readonly onboarding: {
    readonly setupRequired: boolean;
    readonly tutorialActive: boolean;
    readonly finishPending: boolean;
    readonly resumePending: boolean;
    readonly chatInputEnabled: boolean;
    readonly currentStep?: "chat" | "text" | "image" | "voice" | "persona" | "watch";
    readonly skipHint?: string;
    readonly settingsHighlight?: "persona" | "watch";
  };
}

export interface PersonaDocument {
  readonly id: string;
  readonly body: string;
  readonly builtin: boolean;
  readonly versions: readonly { readonly id: string; readonly createdAt: string }[];
}

export interface SpeechView {
  readonly generation: number;
  readonly phase: "idle" | "starting" | "recording" | "finalizing" | "confirming" | "sending" | "cancelling";
  readonly partial: string;
  readonly microphonePermission: "not-determined" | "granted" | "denied" | "restricted" | "unavailable";
  readonly recognitionPermission: "not-determined" | "granted" | "denied" | "restricted" | "unavailable";
  readonly inputDevices: readonly { readonly id: string; readonly name: string }[];
  readonly warningKind?: string;
  readonly message?: string;
  readonly source?: "shortcut" | "composer";
}

export interface AudioView {
  readonly generation: number;
  readonly phase: "off" | "starting" | "listening" | "stopping" | "error";
  readonly microphonePermission: "not-determined" | "granted" | "denied" | "restricted" | "unavailable";
  readonly recognitionPermission: "not-determined" | "granted" | "denied" | "restricted" | "unavailable";
  readonly screenCapturePermission: "granted" | "not-granted" | "unknown";
  readonly warningKind?: string;
  readonly message?: string;
  readonly latestObservation?: {
    readonly id: string;
    readonly createdAt: string;
    readonly source: "microphone" | "speaker";
    readonly text: string;
  };
}

export interface SpeechPopupSnapshot {
  readonly revision: number;
  readonly companionDisplayName: string;
  readonly speech: SpeechView;
  readonly theme: "system" | "light" | "dark";
  readonly font: string;
  readonly avatarColor?: string;
  readonly avatarImagePng?: readonly number[];
}

export interface WatchTargetView {
  readonly target: string;
  readonly name: string;
  readonly enabled: boolean;
  readonly foreground: boolean;
  readonly lastCapturedAt?: string;
  readonly lastTrigger?: string;
}

export interface CapturePopupSnapshot {
  readonly revision: number;
  readonly captureId: string;
  readonly attachmentKind: "image" | "text";
  readonly accessibilityPermissionRequired: boolean;
  readonly png?: readonly number[];
  readonly textPreview?: string;
  readonly textPreviewTruncated: boolean;
  readonly textTruncated: boolean;
  readonly textTruncatedCharacters: number;
  readonly quickActions: readonly PopupQuickAction[];
  readonly sendKey: "enter" | "cmdEnter";
  readonly companionDisplayName: string;
  readonly theme: "system" | "light" | "dark";
  readonly font: string;
  readonly avatarColor?: string;
  readonly avatarImagePng?: readonly number[];
}

export interface PopupQuickAction {
  readonly label: string;
  readonly message: string;
}

export interface DebugGateRecord {
  readonly id: string;
  readonly createdAt: string;
  readonly trigger: string;
  readonly sent: boolean;
  readonly reason: string;
  readonly imageFile?: string;
  readonly ocrPreview?: string;
}

export interface DebugDetail {
  readonly sourceIds: readonly string[];
  readonly imageFiles: readonly string[];
  readonly ocrPreview?: string;
  readonly observerResponse?: unknown;
  readonly companionContext?: string;
}

export interface DebugCatalog {
  readonly details: readonly DebugDetail[];
  readonly latestGate?: DebugGateRecord;
}

export interface MemoryStatus {
  readonly enabled: boolean;
  readonly providerConsent: boolean;
  readonly dailyCount: number;
  readonly weeklyCount: number;
  readonly factCount: number;
  readonly candidateCount: number;
  readonly delayedJobs: number;
  readonly stale: boolean;
  readonly lastErrorKind?: "provider" | "invalidOutput" | "persistence" | "consent" | "sourceChanged" | "capacity";
  readonly retryAt?: string;
  readonly suggestConsolidation: boolean;
  readonly capacityBlocked: boolean;
}

export interface MemoryFact {
  readonly id: string;
  readonly text?: string;
  readonly confirmedAt: string;
  readonly sourceUserMessageIds: readonly string[];
}

export interface MemoryCandidate {
  readonly id: string;
  readonly text: string;
  readonly createdAt: string;
  readonly sourceUserMessageIds: readonly string[];
}

export interface MemoryFactUpdate {
  readonly id: string;
  readonly operation: "expire" | "merge" | "rewrite";
  readonly factIds: readonly string[];
  readonly replacement?: string;
  readonly reason: string;
  readonly createdAt: string;
}

export interface MemorySummary {
  readonly localDate?: string;
  readonly period?: string;
  readonly generatedAt: string;
  readonly text: string;
  readonly state: "current" | "stale";
}

export interface MemoryCatalog {
  readonly facts: readonly MemoryFact[];
  readonly candidates: readonly MemoryCandidate[];
  readonly updates: readonly MemoryFactUpdate[];
  readonly daily: readonly MemorySummary[];
  readonly weekly: readonly MemorySummary[];
}

export interface BubbleRecord {
  readonly id: string;
  readonly createdAt: string;
  readonly message: string;
  readonly messageKind: "advice" | "encouragement" | "nudge" | "celebration" | "summary" | "chat" | "thought" | "tutorial" | "tutorial-typing" | "setup" | "notice" | "fact-confirmation";
  readonly notificationPriority: NotificationPriority;
  readonly causedBy?: string;
  readonly displayName: string;
  readonly persona: string;
  readonly avatarColor?: string;
  readonly conversationGeneration: number;
  readonly persistent: boolean;
  readonly openUrl?: string;
  readonly interaction?: BubbleInteraction;
}

export interface BubbleInteraction {
  readonly select?: {
    readonly options: readonly { readonly value: string; readonly label: string }[];
    readonly selected: string;
    readonly action: string;
    readonly confirmLabel: string;
  };
  readonly actions: readonly { readonly id: string; readonly label: string }[];
  readonly detail?: string;
  readonly technicalDetail?: string;
}

export interface BubbleSnapshot {
  readonly generation: number;
  readonly records: readonly BubbleRecord[];
  readonly theme: "system" | "light" | "dark";
  readonly font: string;
  readonly position: "bottom-right" | "top-right" | "bottom-left" | "top-left";
  readonly avatarColor?: string;
  readonly avatarImagePng?: readonly number[];
}

export interface SettingsAppearancePreviewPayload {
  readonly theme: "system" | "light" | "dark";
  readonly font: string;
  readonly avatarColor: string;
  readonly bubblePosition: "bottom-right" | "top-right" | "bottom-left" | "top-left";
  readonly bubbleDisplay: "main" | "cursor" | "front";
}

export interface SnapshotEvent { readonly revision: number; readonly snapshot: AppSnapshot }
export interface ConfigIssue { readonly path: string; readonly message: string }
export type IpcResult<T> =
  | { readonly ok: true; readonly value: T; readonly issues?: readonly ConfigIssue[] }
  | { readonly ok: false; readonly error: { readonly message: string; readonly issues?: readonly ConfigIssue[] } };
export type ConfigPatch = Record<string, unknown>;
