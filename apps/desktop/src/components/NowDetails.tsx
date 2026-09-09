import type { ReactElement } from "react";

import type { AppSnapshot, CompanionEmotions, DebugDetail } from "../types.js";
import { audioStatus, companionCallSummary, companionStatus, companionThought, findDebugDetail, lastVisualActivity, observerStatus, triggerLabel, triggerText } from "../view-model.js";
import { useI18n } from "../i18n/index.js";

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
  const { locale, t } = useI18n();
  const gate = snapshot.debugCatalog.latestGate;
  const showObservationDetail = createLastVisualDetailAction(snapshot, onShowDebug);
  return <div className="now-drawer" id="now-details">
    <dl>
      <div><dt>{t("now.visualState")}</dt><dd>{observerStatus(snapshot, Date.now(), locale)}</dd></div>
      <div><dt>{t("now.audioState")}</dt><dd>{audioStatus(snapshot, locale)}{snapshot.audio?.latestObservation === undefined ? "" : ` ・ ${snapshot.audio.latestObservation.source === "speaker" ? t("now.speaker") : t("now.microphone")}: ${snapshot.audio.latestObservation.text}`}</dd></div>
      <div><dt>{t("now.companionState")}</dt><dd>{companionStatus(snapshot, locale)}</dd></div>
      <div><dt>{t("now.emotions")}</dt><dd>
        <span>{snapshot.config.companion.emotionsEnabled
          ? Object.values(snapshot.companionEmotions).every((value) => value === 0) ? t("now.emotionsNeutral") : t("now.emotionsEnabled")
          : t("now.emotionsDisabled")}</span>
        <ul className="now-emotions">{Object.entries({
          joy: t("now.emotionJoy"),
          embarrassment: t("now.emotionEmbarrassment"),
          concern: t("now.emotionConcern"),
          surprise: t("now.emotionSurprise"),
          curiosity: t("now.emotionCuriosity"),
          frustration: t("now.emotionFrustration"),
        }).map(([key, label]) => <li key={key}>{label} {snapshot.companionEmotions[key as keyof CompanionEmotions]} / 100</li>)}</ul>
      </dd></div>
      <div><dt>{t("now.latestTrigger")}</dt><dd>{triggerText(snapshot, locale).replace(t("view.latestTriggerPrefix"), "")}</dd></div>
      <div><dt>{t("now.visualActivity")}</dt><dd>{lastVisualActivity(snapshot, locale)}</dd></div>
      <div><dt>{t("now.thought")}</dt><dd>{companionThought(snapshot, locale)}</dd></div>
      <div><dt>{t("now.ocrGate")}</dt><dd>{snapshot.observer.ocrGateEnabled ? t("common.enabled") : t("common.disabled")}</dd></div>
      <div><dt>{t("now.activitySignals")}</dt><dd>{snapshot.observer.activitySignalsEnabled ? t("common.enabled") : t("common.disabled")}</dd></div>
      <div><dt>{t("now.callsToday")}</dt><dd>{t("now.callsSummary", { visual: snapshot.observer.aiCallsToday, companion: companionCallSummary(snapshot, locale) })}</dd></div>
      {snapshot.observer.targets.map((target) => <div key={target.target}><dt>{target.name}</dt><dd>{target.enabled ? target.foreground ? t("now.foreground") : t("common.enabled") : t("common.disabled")} ・ {t("now.lastCapture", { value: target.lastCapturedAt === undefined ? t("common.none") : new Date(target.lastCapturedAt).toLocaleTimeString(locale === "ja" ? "ja-JP" : "en-US", { hour: "2-digit", minute: "2-digit" }) })} ・ {t("now.trigger", { value: target.lastTrigger === undefined ? t("common.none") : triggerLabel(target.lastTrigger, locale) })}</dd></div>)}
    </dl>
    {snapshot.config.debug.enabled ? <div className="now-debug-actions">
      {gate === undefined ? null : <button type="button" onClick={() => onShowDebug({ sourceIds: [gate.id], imageFiles: gate.imageFile === undefined ? [] : [gate.imageFile], ocrPreview: gate.ocrPreview })}>{t("now.captureDetails")}</button>}
      {showObservationDetail === undefined ? null : <button type="button" onClick={showObservationDetail}>{t("now.visualDetails")}</button>}
    </div> : null}
  </div>;
}
