import { useEffect, useState, type ReactElement } from "react";

import { desktopApi } from "./ipc.js";
import type { IpcResult, MemoryCatalog, MemoryStatus } from "./types.js";
import { useI18n } from "./i18n/index.js";

export function MemoryPanel({ status }: { readonly status: MemoryStatus }): ReactElement {
  const [catalog, setCatalog] = useState<MemoryCatalog>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const { t } = useI18n();
  useEffect(() => {
    let active = true;
    void desktopApi.listMemory().then((result) => {
      if (!active) return;
      if (result.ok) setCatalog(result.value);
      else setError(result.error.message);
    });
    return () => { active = false; };
  }, [status.factCount, status.candidateCount, status.dailyCount, status.weeklyCount]);
  const run = async (operation: Promise<IpcResult<MemoryCatalog>>): Promise<void> => {
    setBusy(true);
    const result = await operation;
    if (result.ok) { setCatalog(result.value); setError(undefined); }
    else setError(result.error.message);
    setBusy(false);
  };
  return <fieldset disabled={busy}>
    <legend>{t("memory.title")}</legend>
    <p className="field-help">{t("memory.description")}</p>
    {status.enabled && !status.providerConsent ? <p className="error-text">{t("memory.consent")}</p> : null}
    {status.delayedJobs > 0 ? <p>{t("memory.delayed", { count: status.delayedJobs })}</p> : null}
    {status.stale ? <p>{t("memory.stale")}</p> : null}
    {status.suggestConsolidation && !status.capacityBlocked ? <p>{t("memory.suggest")}</p> : null}
    {status.capacityBlocked ? <p className="error-text">{t("memory.capacity")}</p> : null}
    {status.lastErrorKind === undefined ? null : <p className="error-text">{t("memory.failed", { kind: status.lastErrorKind, retry: status.retryAt === undefined ? "" : t("memory.next", { value: status.retryAt }) })}</p>}
    {catalog?.candidates.map((candidate) => <div key={candidate.id} className="memory-item"><p>{candidate.text}</p><div className="button-row"><button type="button" onClick={() => void run(desktopApi.confirmMemory(candidate.id, crypto.randomUUID()))}>{t("memory.confirm")}</button><button type="button" onClick={() => void run(desktopApi.rejectMemory(candidate.id))}>{t("memory.reject")}</button></div></div>)}
    {catalog?.updates.map((update) => <div key={update.id} className="memory-item"><p>{update.reason}</p>{update.replacement === undefined ? null : <p>{update.replacement}</p>}<div className="button-row"><button type="button" onClick={() => void run(desktopApi.confirmMemoryUpdate(update.id, crypto.randomUUID()))}>{t("memory.update")}</button><button type="button" onClick={() => void run(desktopApi.rejectMemoryUpdate(update.id))}>{t("memory.reject")}</button></div></div>)}
    <h3>{t("memory.confirmed")}</h3>
    {catalog?.facts.length === 0 ? <p className="muted">{t("memory.empty")}</p> : catalog?.facts.map((fact) => <div key={fact.id} className="memory-item"><span>{fact.text}</span><button type="button" onClick={() => void run(desktopApi.deleteMemory(fact.id, crypto.randomUUID()))}>{t("common.delete")}</button></div>)}
    <h3>{t("memory.dailyWeekly")}</h3>
    {[...(catalog?.daily ?? []), ...(catalog?.weekly ?? [])].map((summary) => <details key={summary.localDate ?? summary.period}><summary>{summary.localDate ?? summary.period}{summary.state === "stale" ? t("memory.staleSuffix") : ""}</summary><p>{summary.text}</p></details>)}
    <button type="button" onClick={() => void run(desktopApi.consolidateMemory(yesterday()))}>{t("memory.consolidate")}</button>
    {error === undefined ? null : <p className="error-text">{error}</p>}
  </fieldset>;
}

function yesterday(): string {
  const value = new Date();
  value.setDate(value.getDate() - 1);
  return [
    value.getFullYear(),
    String(value.getMonth() + 1).padStart(2, "0"),
    String(value.getDate()).padStart(2, "0"),
  ].join("-");
}
