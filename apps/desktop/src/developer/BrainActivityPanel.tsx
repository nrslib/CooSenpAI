import { useEffect, useMemo, useRef, useState, type ReactElement } from "react";
import type { BrainActivityObservation, BrainActivityModule, RateActivity } from "../brain-activity.js";
import { activityChangeScale, activityFrameChange, decodeActivityFrame, type ActivityDisplayMode } from "../brain-activity.js";
import { useI18n } from "../i18n/index.js";
import { BrainActivityDetails } from "./BrainActivityDetails.js";
export { BrainActivityPanel } from "./BrainActivityHistory.js";
import { createBrainRenderer, type BrainRenderer } from "./brain-activity-renderer.js";

export function unavailableKey(module: Pick<BrainActivityModule, "status" | "unavailableReason">): "brain.invalid" | "brain.failed" | "brain.cached" | "brain.replayed" | "brain.positionsUnavailable" | "brain.notProvided" {
  if (module.status === "invalid") return "brain.invalid";
  if (module.status === "failed") return "brain.failed";
  switch (module.unavailableReason) {
    case "image-cache-hit": return "brain.cached";
    case "observation-replayed": return "brain.replayed";
    case "positions-unavailable": return "brain.positionsUnavailable";
    default: return "brain.notProvided";
  }
}

export function ActivityObservation({ observation, mode, onModeChange }: { readonly observation: BrainActivityObservation; readonly mode: ActivityDisplayMode; readonly onModeChange: (mode: ActivityDisplayMode) => void }): ReactElement {
  const { t } = useI18n();
  const [moduleIndex, setModuleIndex] = useState(observation.modules[0]!.moduleIndex);
  const module = observation.modules.find((module) => module.moduleIndex === moduleIndex)!;
  return <>
    <label className="brain-module">{t("brain.module")}
      <select value={moduleIndex} onChange={(event) => setModuleIndex(Number(event.target.value))}>
        {observation.modules.map((module) => <option key={module.moduleIndex} value={module.moduleIndex}>{t("brain.moduleNumber", { number: module.moduleIndex + 1 })}</option>)}
      </select>
    </label>
    <BrainActivityDetails observation={observation} module={module} />
    {module.activity === null ? <p role="status">{t(unavailableKey(module))}</p> : <ActivityPlayer key={moduleIndex} activity={module.activity} mode={mode} onModeChange={onModeChange} />}
  </>;
}

export function ActivityPlayer({ activity, mode, onModeChange }: { readonly activity: RateActivity; readonly mode: ActivityDisplayMode; readonly onModeChange: (mode: ActivityDisplayMode) => void }): ReactElement {
  const { t } = useI18n();
  const changeScale = useMemo(() => activityChangeScale(activity), [activity]);
  const host = useRef<HTMLDivElement>(null);
  const renderer = useRef<BrainRenderer | null>(null);
  const [frame, setFrame] = useState(0);
  const [playing, setPlaying] = useState(() => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches !== true);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (host.current === null) return;
    try {
      const view = createBrainRenderer(host.current, activity, () => { setFailed(true); setPlaying(false); });
      renderer.current = view;
      view.setFrame(0, mode);
      return () => { renderer.current = null; view.dispose(); };
    } catch {
      // WebGL failure affects only the optional display; the read-only activity remains usable.
      setFailed(true); setPlaying(false);
    }
  }, [activity]);
  useEffect(() => { renderer.current?.setFrame(frame, mode); }, [frame, mode]);
  useEffect(() => {
    if (!playing) return;
    if (frame === activity.frames.length - 1) { setPlaying(false); return; }
    const timer = window.setTimeout(() => setFrame(frame + 1), 400);
    return () => window.clearTimeout(timer);
  }, [playing, frame, activity.frames.length]);
  const values = decodeActivityFrame(activity, frame);
  let peak = 0;
  let active = 0;
  for (const value of values) { peak = Math.max(peak, value); if (value > 0) active += 1; }
  const differences = activityFrameChange(activity, frame);
  let increase = 0, decrease = 0, maxChange = 0;
  if (differences !== null) for (const value of differences) { if (value > 0) increase += 1; if (value < 0) decrease += 1; maxChange = Math.max(maxChange, Math.abs(value)); }
  const ended = frame === activity.frames.length - 1;
  return <>
    <p role="status" data-playback-state={playing ? "playing" : ended ? "finished" : "paused"}>{t(playing ? "brain.playing" : ended ? "brain.finished" : "brain.paused")}</p>
    <p>{t("brain.stepMeaning", { total: activity.total_steps })}</p>
    <p>{t("brain.scope", { sample: activity.neurons.length, positioned: activity.positioned_neurons, total: activity.total_neurons, missing: activity.total_neurons - activity.positioned_neurons })}</p>
    <label className="brain-mode">{t("brain.displayMode")}
      <select aria-label={t("brain.displayMode")} value={mode} onChange={(event) => onModeChange(event.target.value as ActivityDisplayMode)}>
        <option value="activity">{t("brain.activityMode")}</option><option value="change">{t("brain.changeMode")}</option>
      </select>
    </label>
    <div ref={host} className="brain-canvas" role="img" aria-label={t("brain.canvas")} />
    {failed ? <p role="alert">{t("brain.webglFailed")}</p> : null}
    <div className="brain-controls">
      <button type="button" onClick={() => { if (frame === activity.frames.length - 1) setFrame(0); setPlaying(!playing); }}>{playing ? t("brain.pause") : ended ? t("brain.replay") : t("brain.play")}</button>
      <label>{t(frame === 0 ? "brain.initialStep" : ended ? "brain.finalStep" : "brain.step", { step: activity.frames[frame]!.step, total: activity.total_steps })}
        <input type="range" min={0} max={activity.frames.length - 1} value={frame} aria-label={t("brain.seek")} onChange={(event) => { setPlaying(false); setFrame(Number(event.target.value)); }} />
      </label>
    </div>
    <div className="brain-camera-controls">
      <button type="button" disabled={failed} onClick={() => renderer.current?.rotate(-Math.PI / 12, 0)}>{t("brain.left")}</button>
      <button type="button" disabled={failed} onClick={() => renderer.current?.rotate(Math.PI / 12, 0)}>{t("brain.right")}</button>
      <button type="button" disabled={failed} onClick={() => renderer.current?.rotate(0, Math.PI / 12)}>{t("brain.up")}</button>
      <button type="button" disabled={failed} onClick={() => renderer.current?.rotate(0, -Math.PI / 12)}>{t("brain.down")}</button>
      <button type="button" disabled={failed} onClick={() => renderer.current?.zoom(0.8)}>{t("brain.zoomIn")}</button>
      <button type="button" disabled={failed} onClick={() => renderer.current?.zoom(1.25)}>{t("brain.zoomOut")}</button>
      <button type="button" disabled={failed} onClick={() => renderer.current?.reset()}>{t("brain.reset")}</button>
    </div>
    <p className={mode === "change" ? "brain-legend brain-change-legend" : "brain-legend"}><span aria-hidden="true" />{mode === "activity" ? t("brain.legend", { hmax: activity.hmax }) : t("brain.changeLegend", { scale: changeScale.toExponential(2) })}</p>
    {mode === "change" ? <p className="brain-note">{t("brain.changeScaleNote")}</p> : null}
    <p className="brain-change-statistics">{differences === null ? t("brain.noComparison") : t("brain.changeStatistics", { previous: activity.frames[frame - 1]!.step, current: activity.frames[frame]!.step, increase, decrease, peak: maxChange.toExponential(2) })}</p>
    <p className="brain-statistics">{t("brain.statistics", { active, peak: peak.toExponential(2) })}</p>
    <p className="brain-note">{t("brain.note")}</p>
    <p className="brain-source">{t("brain.source")}</p>
  </>;
}
