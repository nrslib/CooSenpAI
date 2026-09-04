import type { AppSnapshot, ConversationEntry, DebugCatalog, DebugDetail } from "./types.js";

export function effectiveAssertiveness(
  snapshot: AppSnapshot,
  now = Date.now(),
): "low" | "normal" | "high" {
  const temporary = snapshot.temporaryAssertiveness;
  return temporary !== undefined && new Date(temporary.expiresAt).getTime() > now
    ? temporary.value
    : snapshot.config.companion.assertiveness;
}

export type PresenceMode = "resting" | "watching" | "thinking" | "attention" | "switching";

export interface PresenceView {
  readonly mode: PresenceMode;
  readonly text: string;
}

export function avatarColor(configured?: string | null): string {
  if (configured !== undefined && configured !== null) return configured;
  return "var(--logo-body)";
}

export function companionThought(snapshot: AppSnapshot): string {
  return snapshot.latestCompanionThought ?? "まだありません";
}

export function presenceView(snapshot: AppSnapshot, watchChanging: boolean): PresenceView {
  if (snapshot.onboarding.setupRequired) return { mode: "resting", text: "セットアップ待ち" };
  if (watchChanging) return { mode: "switching", text: "切り替えています…" };
  const observerError = observerErrorMessage(snapshot);
  if (observerError !== undefined) return { mode: "attention", text: `見守りエラー: ${observerError}` };
  if (companionResponseInProgress(snapshot)) return { mode: "thinking", text: "考えています…" };
  if (snapshot.lastError !== undefined || snapshot.deliveryOutboxBlocked) {
    return { mode: "attention", text: "確認が必要です" };
  }
  return snapshot.observerRunning
    ? { mode: "watching", text: "見ています" }
    : { mode: "resting", text: "休憩中" };
}

export interface StablePresence {
  readonly view: PresenceView;
  readonly candidate?: {
    readonly view: PresenceView;
    readonly since: number;
  };
}

export function stabilizePresence(
  current: StablePresence,
  next: PresenceView,
  now: number,
  minimumDurationMs = 2_000,
): StablePresence {
  if (current.view.mode === next.mode) {
    return current.candidate === undefined ? current : { view: current.view };
  }
  if (current.candidate?.view.mode !== next.mode) {
    return { view: current.view, candidate: { view: next, since: now } };
  }
  if (now - current.candidate.since < minimumDurationMs) {
    return { view: current.view, candidate: { view: next, since: current.candidate.since } };
  }
  return { view: next };
}

export type NowLineView = {
  readonly mode: "error";
  readonly errorMessage: string;
} | {
  readonly mode: "resting" | "watching" | "application";
  readonly frontApp?: string;
  readonly lastCapturedAt?: string;
};

export function nowLineView(snapshot: AppSnapshot): NowLineView {
  const observerError = observerErrorMessage(snapshot);
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

export function nowLine(snapshot: AppSnapshot, now = Date.now()): string {
  const view = nowLineView(snapshot);
  if (view.mode === "error") return `いま: 見守りエラー ・ ${view.errorMessage}`;
  const app = view.mode === "resting"
    ? "休憩中"
    : view.mode === "watching"
      ? "見ています"
      : `${view.frontApp ?? ""} を見ている`;
  return `いま: ${app} ・ ${relativeCaptureTime(view.lastCapturedAt, now)}`;
}

export function companionResponseInProgress(snapshot: AppSnapshot): boolean {
  const activeUserMessageId = snapshot.activeUserMessageId;
  return activeUserMessageId !== undefined
    && snapshot.lastError?.attachmentOcr?.inputId !== activeUserMessageId;
}

function observerErrorMessage(snapshot: AppSnapshot): string | undefined {
  if (snapshot.observer.errorMessage !== undefined) return snapshot.observer.errorMessage;
  return snapshot.observer.phase === "error" ? "見守りでエラーが発生しました。" : undefined;
}

export function lastVisualActivity(snapshot: AppSnapshot): string {
  return snapshot.observer.lastVisualObservation?.activity ?? "視覚の記録はまだありません";
}

export function audioStatus(snapshot: AppSnapshot): string {
  const audio = snapshot.audio;
  if (audio === undefined) return "設定なし";
  const sourceCount = Number(snapshot.config.audio.speaker);
  if (!snapshot.config.audio.enabled) return "停止中";
  if (sourceCount === 0) return "入力源未選択";
  switch (audio.phase) {
    case "starting": return "開始中";
    case "listening": return "聞いています";
    case "stopping": return "停止中";
    case "error": return audio.message ?? "エラー";
    case "off": return "待機中";
}
}

export function thoughtBubbleText(snapshot: AppSnapshot): string | undefined {
  if (!companionResponseInProgress(snapshot)) return undefined;
  const raw = snapshot.companionDraft ?? "返事を考え中…";
  const lines = raw.split(/\r?\n/u).filter((line) => line.trim() !== "");
  const tail = (lines.length === 0 ? [raw] : lines).slice(-3).join("\n");
  return tail.length <= 240 ? tail : `…${tail.slice(-239)}`;
}

function relativeCaptureTime(value: string | undefined, now: number): string {
  if (value === undefined) return "まだ撮影していない";
  const capturedAt = new Date(value).getTime();
  if (Number.isNaN(capturedAt)) return "撮影時刻は不明";
  const seconds = Math.max(0, Math.floor((now - capturedAt) / 1_000));
  if (seconds < 60) return "たった今撮影";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} 分前に撮影`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 時間前に撮影`;
  return `${Math.floor(hours / 24)} 日前に撮影`;
}

export type StatusBannerAction = "open-settings" | "open-speech-settings" | "relaunch" | "open-app-settings";

export interface AttachmentTextView {
  readonly preview: string;
  readonly previewTruncated: boolean;
  readonly truncationNotice?: string;
}

export function attachmentTextView(value: string): AttachmentTextView {
  const match = value.match(/\n\n(末尾を切りました（\d+ 文字）)$/u);
  const body = match === null ? value : value.slice(0, match.index);
  return {
    preview: body.slice(0, 2_000),
    previewTruncated: body.length > 2_000,
    truncationNotice: match?.[1],
  };
}

export interface StatusBannerView {
  readonly tone: "warning" | "error" | "info";
  readonly message: string;
  readonly action?: StatusBannerAction;
  readonly actionLabel?: string;
}

export function statusBanner(snapshot: AppSnapshot): StatusBannerView | undefined {
  if (snapshot.onboarding.finishPending) {
    return {
      tone: "error",
      message: "終了処理をやり直してください",
    };
  }
  if (snapshot.captureShortcutError !== undefined) {
    return {
      tone: "warning",
      message: snapshot.captureShortcutError,
      action: "open-app-settings",
      actionLabel: "設定を開く",
    };
  }
  if (snapshot.speech?.message !== undefined) {
    const permission = snapshot.speech.microphonePermission !== "granted"
      || snapshot.speech.recognitionPermission !== "granted";
    return {
      tone: "warning",
      message: snapshot.speech.message,
      action: permission ? "open-speech-settings" : undefined,
      actionLabel: permission ? "システム設定を開く" : undefined,
    };
  }
  if (snapshot.audio?.message !== undefined && snapshot.config.audio.enabled) {
    const screenPermission = snapshot.config.audio.speaker
      && snapshot.audio.screenCapturePermission !== "granted";
    const recognitionPermission = snapshot.audio.recognitionPermission !== "granted";
    const openSpeechSettings = !screenPermission && recognitionPermission;
    return {
      tone: "warning",
      message: snapshot.audio.message,
      action: screenPermission ? "open-settings" : openSpeechSettings ? "open-speech-settings" : undefined,
      actionLabel: screenPermission || openSpeechSettings ? "システム設定を開く" : undefined,
    };
  }
  if (snapshot.lastError?.kind === "config") {
    return {
      tone: "error",
      message: snapshot.lastError.message ?? "設定を確認してください。",
      action: "open-app-settings",
      actionLabel: "設定を開く",
    };
  }
  const observerError = observerErrorMessage(snapshot);
  if (observerError !== undefined) {
    return {
      tone: "error",
      message: `見守りに失敗しました: ${observerError}`,
      action: "open-app-settings",
      actionLabel: "設定を開く",
    };
  }
  if (snapshot.lastError?.attachmentOcr !== undefined) {
    const retry = snapshot.lastError.attachmentOcr.retryable
      && snapshot.companionRetryInSeconds !== undefined
      ? ` ${snapshot.companionRetryInSeconds} 秒後に再試行します。`
      : "";
    return {
      tone: "error",
      message: `${attachmentOcrFailureMessage(snapshot.lastError.attachmentOcr.reason)}${retry}`,
    };
  }
  if (snapshot.watchIntentActive
      && snapshot.config !== undefined
      && !snapshot.config.watch.fullscreen
      && !snapshot.config.watch.apps.some((app) => app.enabled)) {
    return {
      tone: "info",
      message: "設定の「見ていいもの」で、画面全体かアプリを追加してください。",
      action: "open-app-settings",
      actionLabel: "設定を開く",
    };
  }
  if (snapshot.watchIntentActive && snapshot.screenRecordingStatus !== "granted") {
    const restart = snapshot.screenRecordingRestartRequired;
    return {
      tone: "warning",
      message: snapshot.screenRecordingMessage ?? "画面収録の権限が必要です。",
      action: restart ? "relaunch" : "open-settings",
      actionLabel: restart ? "再起動" : "システム設定を開く",
    };
  }
  if (snapshot.deliveryOutboxBlocked) {
    return {
      tone: "warning",
      message: `配信待ち ${snapshot.pendingDeliveries} 件。保存先へ書き込めません。`,
    };
  }
  if (snapshot.lastError !== undefined) {
    const retry = snapshot.companionRetryInSeconds === undefined
      ? ""
      : ` ${snapshot.companionRetryInSeconds} 秒後に再試行します。`;
    return {
      tone: "info",
      message: `${snapshot.companionDisplayName}の準備に失敗しました（${snapshot.lastError.kind}）。${retry}`.trim(),
    };
  }
  return undefined;
}

export function unreadBoundaryIndex(
  conversation: readonly ConversationEntry[],
  unreadCount: number,
): number | undefined {
  if (unreadCount <= 0 || conversation.length === 0) return undefined;
  return Math.max(0, conversation.length - Math.min(unreadCount, conversation.length));
}

export function conversationDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "日付不明";
  return date.toLocaleDateString("ja-JP", { month: "long", day: "numeric", weekday: "short" });
}

export function observerStatus(snapshot: AppSnapshot, now = Date.now()): string {
  const observer = snapshot.observer;
  const observerError = observerErrorMessage(snapshot);
  if (observerError !== undefined) return `エラー: ${observerError}`;
  if (observer.pendingFrameCount > 0 && observer.nextSendAt !== undefined && observer.phase !== "thinking") {
    const seconds = Math.max(0, Math.ceil((new Date(observer.nextSendAt).getTime() - now) / 1_000));
    return `次の送信まで ${seconds} 秒（フレーム ${observer.pendingFrameCount} 枚）`;
  }
  switch (observer.phase) {
    case "stopped": return "停止中";
    case "idle": return "見守り中";
    case "capturing": return "撮影中";
    case "thinking": return "視覚が確認中";
    case "suspended": return "一時停止中";
    case "error": return "エラー";
  }
}

export function companionStatus(snapshot: AppSnapshot): string {
  const name = snapshot.companionDisplayName;
  if (snapshot.lastError?.attachmentOcr !== undefined) {
    return attachmentOcrFailureMessage(snapshot.lastError.attachmentOcr.reason);
  }
  if (snapshot.lastError !== undefined && snapshot.companionRetryInSeconds !== undefined) {
    return `${name}の準備に失敗（${snapshot.lastError.kind}）: ${snapshot.companionRetryInSeconds} 秒後に再試行`;
  }
  if (snapshot.deliveryOutboxBlocked) return `配信待ち ${snapshot.pendingDeliveries} 件（outbox に書けません）`;
  if (snapshot.companion.phase === "thinking") return `${name}が考え中`;
  if (snapshot.companion.phase === "error") return `${name}: エラー`;
  return `${name}: 話せます`;
}

export function companionCallSummary(snapshot: AppSnapshot): string {
  const limit = snapshot.companion.proactiveLimitReached ? "・上限" : "";
  return `${snapshot.companionDisplayName} ${snapshot.companion.totalCallsToday} 回${limit}`;
}

export function attachmentOcrFailureMessage(
  reason: NonNullable<NonNullable<AppSnapshot["lastError"]>["attachmentOcr"]>["reason"],
): string {
  switch (reason) {
    case "capability": return "添付の画像対応を確認できませんでした";
    case "helper-unavailable": return "添付の文字起こしに失敗しました（OCR helper が見つかりません）";
    case "recognition": return "添付の文字起こしに失敗しました（画像を認識できません）";
    case "no-text": return "添付の文字起こしに失敗しました（文字が見つかりません）";
  }
}

export function attachmentFailureState(
  failure: NonNullable<NonNullable<AppSnapshot["lastError"]>["attachmentOcr"]>,
): { readonly terminal: boolean; readonly message: string } {
  if (!failure.retryable) {
    return { terminal: true, message: "添付を準備できませんでした" };
  }
  return {
    terminal: false,
    message: `再試行を待っています（${Math.min(failure.attempts, 3)}/3）`,
  };
}

export function triggerText(snapshot: AppSnapshot): string {
  const trigger = snapshot.observer.lastTrigger;
  if (trigger === undefined) return "直近のきっかけ: なし";
  const label = triggerLabel(trigger);
  const disposition = snapshot.observer.lastCaptureDisposition ?? "撮影";
  return `直近のきっかけ: ${label} → ${disposition}`;
}

export function triggerLabel(trigger: string): string {
  if (trigger === "typing-paused") return "入力が止まった";
  if (trigger === "app-switched") return "アプリが切り替わった";
  if (trigger === "timer") return "定期撮影";
  return trigger;
}

export function conversationThinking(snapshot: AppSnapshot, sending: boolean): boolean {
  return sending || companionResponseInProgress(snapshot);
}

export type ComposerKeyAction = "send" | "newline" | "ignore";
export type MicrophoneAction = "start" | "finish" | "cancel" | "ignore";

export function microphoneAction(
  mode: "pushToTalk" | "toggle",
  recording: boolean,
  gesture: "press" | "release" | "cancel" | "click",
): MicrophoneAction {
  if (mode === "toggle") {
    return gesture === "click" ? recording ? "finish" : "start" : "ignore";
  }
  if (gesture === "press") return "start";
  if (gesture === "release") return "finish";
  if (gesture === "cancel") return "cancel";
  return "ignore";
}

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

export type ConversationScrollCause = "panel-open" | "user-send" | "new-entry" | "selection" | "thinking" | "layout";

export function shouldFollowConversation(cause: ConversationScrollCause, userScrolledUp: boolean): boolean {
  return !userScrolledUp || (cause !== "layout" && cause !== "thinking");
}

export function shouldRevealNewestTutorialNotice(
  entry: Pick<ConversationEntry, "role" | "tutorialResponseKey"> | undefined,
): boolean {
  return entry?.role === "companion" && entry.tutorialResponseKey !== undefined;
}

export function didUserScrollUp(previousTop: number, currentTop: number, programmatic: boolean): boolean {
  return !programmatic && currentTop < previousTop;
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

export function formatTime(value: string | undefined): string {
  if (value === undefined) return "なし";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "なし" : date.toLocaleTimeString("ja-JP", { hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

export function acceptSnapshot(current: AppSnapshot | undefined, event: SnapshotEventLike): AppSnapshot | undefined {
  return current !== undefined && event.revision <= current.revision ? undefined : event.snapshot;
}

interface SnapshotEventLike { readonly revision: number; readonly snapshot: AppSnapshot }
