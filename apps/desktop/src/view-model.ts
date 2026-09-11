import type { AppSnapshot, ConversationEntry, DebugCatalog, DebugDetail } from "./types.js";
import { t, type Locale } from "./i18n/index.js";

export function effectiveAssertiveness(
  snapshot: AppSnapshot,
  now = Date.now(),
): "low" | "normal" | "high" {
  const temporary = snapshot.temporaryAssertiveness;
  return temporary !== undefined && new Date(temporary.expiresAt).getTime() > now
    ? temporary.value
    : snapshot.config.companion.assertiveness;
}

export function avatarColor(configured?: string | null): string {
  if (configured !== undefined && configured !== null) return configured;
  return "var(--logo-body)";
}

export function companionThought(snapshot: AppSnapshot, locale: Locale = "ja"): string {
  return snapshot.latestCompanionThought ?? t(locale, "view.noThought");
}

export type NowLineView = {
  readonly mode: "error";
  readonly errorMessage: string;
} | {
  readonly mode: "resting" | "watching" | "application";
  readonly frontApp?: string;
  readonly lastCapturedAt?: string;
};

export function nowLineView(snapshot: AppSnapshot, locale: Locale = "ja"): NowLineView {
  const observerError = observerErrorMessage(snapshot, locale);
  if (observerError !== undefined) return { mode: "error", errorMessage: observerError };
  return {
    mode: !snapshot.observerRunning
      ? "resting"
      : snapshot.observer.frontApp === undefined
        ? "watching"
        : "application",
    frontApp: snapshot.observer.frontApp,
    lastCapturedAt: snapshot.observer.lastCapturedAt,
  };
}

export function nowLine(snapshot: AppSnapshot, now = Date.now(), locale: Locale = "ja"): string {
  const view = nowLineView(snapshot, locale);
  if (view.mode === "error") return t(locale, "view.nowError", { message: view.errorMessage });
  const app = view.mode === "resting"
    ? t(locale, "view.resting")
    : view.mode === "watching"
      ? t(locale, "view.watching")
      : t(locale, "view.watchingApp", { app: view.frontApp ?? "" });
  return t(locale, "view.now", { app, time: relativeCaptureTime(view.lastCapturedAt, now, locale) });
}

function observerErrorMessage(snapshot: AppSnapshot, _locale: Locale): string | undefined {
  if (snapshot.observer.errorMessage !== undefined) return snapshot.observer.errorMessage;
  return snapshot.observer.phase === "error" ? t(_locale, "view.observerErrorDefault") : undefined;
}

export function lastVisualActivity(snapshot: AppSnapshot, locale: Locale = "ja"): string {
  return snapshot.observer.lastVisualObservation?.activity ?? t(locale, "view.noVisualRecord");
}

export function audioStatus(snapshot: AppSnapshot, locale: Locale = "ja"): string {
  const audio = snapshot.audio;
  if (audio === undefined) return t(locale, "view.noSetting");
  const sourceCount = Number(snapshot.config.audio.mic) + Number(snapshot.config.audio.speaker);
  if (!snapshot.config.audio.enabled) return t(locale, "view.stopped");
  if (sourceCount === 0) return t(locale, "view.noInputSource");
  switch (audio.phase) {
    case "starting": return t(locale, "view.starting");
    case "listening": return t(locale, "view.listening");
    case "stopping": return t(locale, "view.stopped");
    case "error": return audio.message ?? t(locale, "view.error");
    case "off": return t(locale, "view.waiting");
}
}

function relativeCaptureTime(value: string | undefined, now: number, locale: Locale): string {
  if (value === undefined) return t(locale, "view.noCapture");
  const capturedAt = new Date(value).getTime();
  if (Number.isNaN(capturedAt)) return t(locale, "view.unknownCaptureTime");
  const seconds = Math.max(0, Math.floor((now - capturedAt) / 1_000));
  if (seconds < 60) return t(locale, "view.capturedJustNow");
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return t(locale, "view.capturedMinutes", { value: minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return t(locale, "view.capturedHours", { value: hours });
  return t(locale, "view.capturedDays", { value: Math.floor(hours / 24) });
}

export interface AttachmentTextView {
  readonly preview: string;
  readonly previewTruncated: boolean;
  readonly truncationNotice?: string;
}

export function attachmentTextView(value: string, locale: Locale = "ja"): AttachmentTextView {
  const marker = new RegExp(`\\n\\n(${truncationMarkerPattern("ja")}|${truncationMarkerPattern("en")})$`, "u");
  const match = value.match(marker);
  const body = match === null ? value : value.slice(0, match.index);
  const count = match?.[1]?.match(/\d+/u)?.[0];
  return {
    preview: body.slice(0, 2_000),
    previewTruncated: body.length > 2_000,
    truncationNotice: count === undefined ? undefined : t(locale, "capture.textTruncated", { count }),
  };
}

function truncationMarkerPattern(locale: Locale): string {
  return escapeRegExp(t(locale, "capture.textTruncated", { count: "__COUNT__" })).replace("__COUNT__", "\\d+");
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

export function unreadBoundaryIndex(
  conversation: readonly ConversationEntry[],
  unreadCount: number,
): number | undefined {
  if (unreadCount <= 0 || conversation.length === 0) return undefined;
  return Math.max(0, conversation.length - Math.min(unreadCount, conversation.length));
}

export function conversationDate(value: string, locale: Locale = "ja"): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return t(locale, "view.unknownDate");
  return date.toLocaleDateString(locale === "ja" ? "ja-JP" : "en-US", { month: "long", day: "numeric", weekday: "short" });
}

export function observerStatus(snapshot: AppSnapshot, now = Date.now(), locale: Locale = "ja"): string {
  const observer = snapshot.observer;
  const observerError = observerErrorMessage(snapshot, locale);
  if (observerError !== undefined) return t(locale, "view.observerErrorStatus", { message: observerError });
  if (observer.pendingFrameCount > 0 && observer.nextSendAt !== undefined && observer.phase !== "thinking") {
    const seconds = Math.max(0, Math.ceil((new Date(observer.nextSendAt).getTime() - now) / 1_000));
    return t(locale, "view.nextSend", { seconds, frames: observer.pendingFrameCount });
  }
  switch (observer.phase) {
    case "stopped": return t(locale, "view.stopped");
    case "idle": return t(locale, "view.observing");
    case "capturing": return t(locale, "view.capturing");
    case "thinking": return t(locale, "view.visualChecking");
    case "suspended": return t(locale, "view.suspended");
    case "error": return t(locale, "view.error");
  }
}

export function companionStatus(snapshot: AppSnapshot, locale: Locale = "ja"): string {
  const name = snapshot.companionDisplayName;
  if (snapshot.lastError?.userResponse !== undefined) return t(locale, "view.userResponseStopped");
  if (snapshot.lastError?.attachmentOcr !== undefined) {
    return attachmentOcrFailureMessage(snapshot.lastError.attachmentOcr.reason, locale);
  }
  if (snapshot.lastError !== undefined && snapshot.companionRetryInSeconds !== undefined) {
    return t(locale, "view.companionRetry", { name, kind: snapshot.lastError.kind, seconds: snapshot.companionRetryInSeconds });
  }
  if (snapshot.deliveryOutboxBlocked) return t(locale, "view.outboxBlocked", { count: snapshot.pendingDeliveries });
  if (snapshot.companion.phase === "thinking") return t(locale, "view.companionThinking", { name });
  if (snapshot.companion.phase === "error") return t(locale, "view.companionError", { name });
  return t(locale, "view.companionReady", { name });
}

export function companionCallSummary(snapshot: AppSnapshot, locale: Locale = "ja"): string {
  const limit = snapshot.companion.proactiveLimitReached ? t(locale, "view.limit") : "";
  return t(locale, "view.calls", { name: snapshot.companionDisplayName, count: snapshot.companion.totalCallsToday, limit });
}

export function attachmentOcrFailureMessage(
  reason: NonNullable<NonNullable<AppSnapshot["lastError"]>["attachmentOcr"]>["reason"],
  locale: Locale = "ja",
): string {
  switch (reason) {
    case "capability": return t(locale, "view.attachmentCapability");
    case "helper-unavailable": return t(locale, "view.attachmentHelper");
    case "recognition": return t(locale, "view.attachmentRecognition");
    case "no-text": return t(locale, "view.attachmentNoText");
  }
}

export function attachmentFailureState(
  failure: NonNullable<NonNullable<AppSnapshot["lastError"]>["attachmentOcr"]>,
  locale: Locale = "ja",
): { readonly terminal: boolean; readonly message: string } {
  if (!failure.retryable) {
    return { terminal: true, message: t(locale, "view.attachmentUnavailable") };
  }
  return {
    terminal: false,
    message: t(locale, "view.attachmentRetry", { attempts: Math.min(failure.attempts, 3) }),
  };
}

export function triggerText(snapshot: AppSnapshot, locale: Locale = "ja"): string {
  const trigger = snapshot.observer.lastTrigger;
  if (trigger === undefined) return t(locale, "view.latestTrigger");
  const label = triggerLabel(trigger, locale);
  const disposition = snapshot.observer.lastCaptureDisposition ?? t(locale, "view.captureDisposition");
  return t(locale, "view.latestTriggerWithDisposition", { trigger: label, disposition });
}

export function triggerLabel(trigger: string, locale: Locale = "ja"): string {
  if (trigger === "typing-paused") return t(locale, "view.triggerTyping");
  if (trigger === "app-switched") return t(locale, "view.triggerAppSwitch");
  if (trigger === "timer") return t(locale, "view.triggerTimer");
  return trigger;
}

export type ComposerKeyAction = "send" | "newline" | "ignore";

export function composerKeyAction(
  key: string,
  metaKey: boolean,
  shiftKey: boolean,
  composing: boolean,
  keyCode: number,
  sendKey: "enter" | "cmdEnter",
): ComposerKeyAction {
  if (key !== "Enter") return "ignore";
  if (composing || keyCode === 229) return "ignore";
  if (sendKey === "cmdEnter") return metaKey && !shiftKey ? "send" : "newline";
  return shiftKey ? "newline" : "send";
}

export function findDebugDetail(catalog: DebugCatalog, sourceIds: readonly string[]): DebugDetail | undefined {
  const matches = catalog.details.filter((detail) => detail.sourceIds.some((id) => sourceIds.includes(id)));
  if (matches.length === 0) return undefined;
  return {
    sourceIds: [...new Set(matches.flatMap((detail) => detail.sourceIds))],
    imageFiles: [...new Set(matches.flatMap((detail) => detail.imageFiles))],
    ocrPreview: matches.find((detail) => detail.ocrPreview !== undefined)?.ocrPreview,
    observerResponse: matches.find((detail) => detail.observerResponse !== undefined)?.observerResponse,
    companionContext: [...matches].reverse().find((detail) => detail.companionContext !== undefined)?.companionContext,
  };
}

export function formatTime(value: string | undefined, locale: Locale = "ja"): string {
  if (value === undefined) return t(locale, "common.none");
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? t(locale, "common.none") : date.toLocaleTimeString(locale === "ja" ? "ja-JP" : "en-US", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

export function acceptSnapshot(current: AppSnapshot | undefined, event: SnapshotEventLike): AppSnapshot | undefined {
  return current !== undefined && event.revision <= current.revision ? undefined : event.snapshot;
}

interface SnapshotEventLike { readonly revision: number; readonly snapshot: AppSnapshot }
