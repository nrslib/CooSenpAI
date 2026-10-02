import { useEffect, useState, type ReactElement } from "react";
import type { BrainActivityHistory, BrainActivityRecord } from "../brain-activity.js";
import type { JudgeDecision } from "../types.js";
import { brainActivityApi } from "../ipc.js";
import { useI18n } from "../i18n/index.js";
import type { ActivityDisplayMode } from "../brain-activity.js";
import { ActivityObservation, unavailableKey } from "./BrainActivityPanel.js";

export function BrainActivityPanel({ decision, enabled, sourceKey = "" }: { readonly decision: JudgeDecision | undefined; readonly enabled: boolean; readonly sourceKey?: string }): ReactElement {
  const { t } = useI18n();
  const [loaded, setLoaded] = useState<{ sourceKey: string; history: BrainActivityHistory } | null>(null);
  const [record, setRecord] = useState<{ sourceKey: string; value: BrainActivityRecord } | null>(null);
  const [manual, setManual] = useState<{ generation: number; recordId: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [evicted, setEvicted] = useState(false);
  const [mode, setMode] = useState<ActivityDisplayMode>("activity");
  const history = enabled && loaded?.sourceKey === sourceKey ? loaded.history : null;
  const selected = history?.entries.find((entry) => manual?.generation === history.generation && entry.recordId === manual.recordId);
  const target = selected ?? history?.entries[0];
  const generation = history?.generation;
  const recordId = target?.recordId;
  const inputId = target?.observation.inputId;
  useEffect(() => {
    if (!enabled) return;
    let active = true;
    void brainActivityApi.getHistory().then((result) => {
      if (!active) return;
      if (!result.ok) { setError(result.error.message); return; }
      setError(null);
      setLoaded((previous) => previous?.sourceKey === sourceKey && previous.history.generation === result.value.generation && previous.history.revision > result.value.revision ? previous : { sourceKey, history: result.value });
    });
    return () => { active = false; };
  }, [enabled, sourceKey, decision?.inputId, decision?.sequence]);
  useEffect(() => {
    if (history === null) return;
    if (manual !== null && manual.generation !== history.generation) { setManual(null); setEvicted(false); }
    else if (manual !== null && !history.entries.some((entry) => entry.recordId === manual.recordId)) { setManual(null); setEvicted(true); }
  }, [history, manual]);
  useEffect(() => {
    setRecord((previous) => previous !== null && (previous.sourceKey !== sourceKey || previous.value.generation !== history?.generation || !history.entries.some((entry) => entry.recordId === previous.value.recordId)) ? null : previous);
  }, [history, sourceKey]);
  useEffect(() => {
    setManual(null);
    setEvicted(false);
    setError(null);
  }, [sourceKey, enabled]);
  useEffect(() => {
    if (!enabled || generation === undefined || recordId === undefined || inputId === undefined) return;
    let active = true;
    void brainActivityApi.getActivity({ generation, recordId, inputId }).then((result) => {
      if (!active) return;
      if (!result.ok) { setError(result.error.message); return; }
      const value = result.value;
      if (value === null) { setError(t("brain.expired")); return; }
      if (value.generation !== generation || value.recordId !== recordId || value.observation.inputId !== inputId) { setError(t("brain.expired")); return; }
      setRecord((previous) => previous?.sourceKey === sourceKey && previous.value.generation === generation && previous.value.recordId === recordId ? previous : { sourceKey, value });
      setError(null);
    });
    return () => { active = false; };
  }, [enabled, sourceKey, generation, recordId, inputId]);
  // A retained record stays mounted during metadata refresh and the next record's read.
  const displayed = record?.sourceKey === sourceKey && record.value.generation === generation && history?.entries.some((entry) => entry.recordId === record.value.recordId) ? record.value : null;
  const pending = target !== undefined && displayed?.recordId !== target.recordId;
  const latest = history?.latest;
  const latestCurrent = latest !== null && latest !== undefined && decision !== undefined && latest.inputId === decision.inputId && latest.decision?.sequence === decision.sequence;
  const reference = displayed !== null && (!latestCurrent || latest?.inputId !== displayed.observation.inputId || !latest?.modules.some((module) => module.status === "available") || selected !== undefined || pending);
  if (!enabled) return <p role="status">{t("brain.disabled")}</p>;
  return <div className="brain-panel">
    <section className="brain-latest" aria-label={t("brain.latest")}>
      <h2>{t("brain.latest")}</h2>
      {decision === undefined ? <p role="status">{t("brain.waiting")}</p> : <p className="brain-observation">{t("brain.observation", { id: decision.inputId, time: decision.occurredAt, mode: decision.mode })}</p>}
      {decision === undefined ? null : <p>{t("brain.readiness")}: {decision.readiness}</p>}
      {decision?.holdReason === undefined ? null : <p>{t("brain.reason")}: {decision.holdReason}</p>}
      {decision?.fallbackReason === undefined ? null : <p>{t("brain.reason")}: {decision.fallbackReason}</p>}
      {decision?.readiness === "composition-pending" ? <p role="status">{t("brain.preparing")}</p> : null}
      {!latestCurrent ? <p role="status">{t("common.loading")}</p> : latest.modules.length === 0 ? <p role="status">{t("brain.notProvided")}</p> : latest.modules.filter((module) => module.status !== "available").map((module) => <p role="status" key={module.moduleIndex}>{t("brain.moduleNumber", { number: module.moduleIndex + 1 })}: {t(unavailableKey(module))}</p>)}
    </section>
    {error === null ? null : <p role="alert">{error}</p>}
    <label className="brain-history">{t("brain.history")}
      <select aria-label={t("brain.history")} value={selected?.recordId ?? "latest"} onChange={(event) => { setEvicted(false); setManual(event.target.value === "latest" || generation === undefined ? null : { generation, recordId: Number(event.target.value) }); }}>
        <option value="latest">{t("brain.followLatest")}</option>
        {history?.entries.map((entry) => <option key={entry.recordId} value={entry.recordId}>{entry.observation.occurredAt} / {entry.observation.inputId}</option>)}
      </select>
    </label>
    {evicted ? <p role="status">{t("brain.evicted")}</p> : null}
    {pending ? <p role="status">{t("brain.loadingRecord")}</p> : null}
    {displayed === null ? <p role="status">{t(history === null ? "common.loading" : "brain.waiting")}</p> : <section className="brain-record" data-observation-id={displayed.observation.inputId}>
      <h2>{t("brain.displayed")}</h2>
      <p>{t("brain.observation", { id: displayed.observation.inputId, time: displayed.observation.occurredAt, mode: displayed.observation.mode ?? "—" })}</p>
      {reference ? <p className="brain-reference" role="status">{t("brain.reference")}</p> : null}
      <ActivityObservation key={`${generation}:${displayed.recordId}`} observation={displayed.observation} mode={mode} onModeChange={setMode} />
    </section>}
  </div>;
}
