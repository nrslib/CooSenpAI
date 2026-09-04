import type { ReactElement } from "react";

import { formatShortcutLabel } from "../shortcut-label.js";
import type { AppSnapshot, DebugDetail } from "../types.js";
import { audioStatus, companionCallSummary, companionStatus, companionThought, findDebugDetail, lastVisualActivity, observerStatus, triggerLabel, triggerText } from "../view-model.js";

export function createLastVisualDetailAction(
  snapshot: AppSnapshot,
  onShowDebug: (detail: DebugDetail) => void,
): (() => void) | undefined {
  const observation = snapshot.observer.lastVisualObservation;
  if (observation === undefined) return undefined;
  const detail = findDebugDetail(snapshot.debugCatalog, [observation.id]);
  return detail === undefined ? undefined : () => onShowDebug(detail);
}

export function NowDetails({ snapshot, onShowDebug }: { readonly snapshot: AppSnapshot; readonly onShowDebug: (detail: DebugDetail) => void }): ReactElement {
  const gate = snapshot.debugCatalog.latestGate;
  const showObservationDetail = createLastVisualDetailAction(snapshot, onShowDebug);
  return <div className="now-drawer" id="now-details">
    <dl>
      <div><dt>視覚の状態</dt><dd>{observerStatus(snapshot)}</dd></div>
      <div><dt>聴覚の状態</dt><dd>{audioStatus(snapshot)}{snapshot.audio?.latestObservation === undefined ? "" : ` ・ ${snapshot.audio.latestObservation.source === "speaker" ? "スピーカー" : "マイク"}: ${snapshot.audio.latestObservation.text}`}</dd></div>
      <div><dt>Cooの状態</dt><dd>{companionStatus(snapshot)}</dd></div>
      <div><dt>直近のきっかけ</dt><dd>{triggerText(snapshot).replace("直近のきっかけ: ", "")}</dd></div>
      <div><dt>視覚が見たこと</dt><dd>{lastVisualActivity(snapshot)}</dd></div>
      <div><dt>いま思っていること</dt><dd>{companionThought(snapshot)}</dd></div>
      <div><dt>OCR ゲート</dt><dd>{snapshot.observer.ocrGateEnabled ? "有効" : "無効"}</dd></div>
      <div><dt>区切り検知</dt><dd>{snapshot.observer.activitySignalsEnabled ? "有効" : "無効"}</dd></div>
      <div><dt>今日の呼び出し</dt><dd>視覚 {snapshot.observer.aiCallsToday} 回 ・ {companionCallSummary(snapshot)}</dd></div>
      {snapshot.observer.targets.map((target) => <div key={target.target}><dt>{target.name}</dt><dd>{target.enabled ? target.foreground ? "前面" : "有効" : "無効"} ・ 最後の撮影: {target.lastCapturedAt === undefined ? "なし" : new Date(target.lastCapturedAt).toLocaleTimeString("ja-JP", { hour: "2-digit", minute: "2-digit" })} ・ きっかけ: {target.lastTrigger === undefined ? "なし" : triggerLabel(target.lastTrigger)}</dd></div>)}
    </dl>
    <div className="shortcut-hints"><strong>ショートカット</strong><span>文章 {formatShortcutLabel(snapshot.config.keymap.sendText ?? "未設定")}</span><span>画面 {formatShortcutLabel(snapshot.config.keymap.captureRegion ?? "未設定")}</span><span>声 {formatShortcutLabel(snapshot.config.keymap.microphone ?? "未設定")}</span><span>パネル {formatShortcutLabel(snapshot.config.keymap.togglePanel ?? "未設定")}</span></div>
    {snapshot.config.debug.enabled ? <div className="now-debug-actions">
      {gate === undefined ? null : <button type="button" onClick={() => onShowDebug({ sourceIds: [gate.id], imageFiles: gate.imageFile === undefined ? [] : [gate.imageFile], ocrPreview: gate.ocrPreview })}>撮影判断の詳細</button>}
      {showObservationDetail === undefined ? null : <button type="button" onClick={showObservationDetail}>視覚の詳細</button>}
    </div> : null}
  </div>;
}
