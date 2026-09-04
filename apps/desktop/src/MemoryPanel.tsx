import { useEffect, useState, type ReactElement } from "react";

import { desktopApi } from "./ipc.js";
import type { IpcResult, MemoryCatalog, MemoryStatus } from "./types.js";

export function MemoryPanel({ status }: { readonly status: MemoryStatus }): ReactElement {
  const [catalog, setCatalog] = useState<MemoryCatalog>();
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
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
    <legend>覚えていること</legend>
    <p className="field-help">確認した事実と、日次・週次の要約です。候補は確認するまで事実として保存されません。</p>
    {status.enabled && !status.providerConsent ? <p className="error-text">記憶を AI provider へ送る同意がないため、生成と添付を停止しています。</p> : null}
    {status.delayedJobs > 0 ? <p>未処理の要約: {status.delayedJobs} 件</p> : null}
    {status.stale ? <p>元の記録が更新された要約を作り直します。</p> : null}
    {status.suggestConsolidation && !status.capacityBlocked ? <p>記憶の容量が上限に近づいています。「昨日を整理する」で内容を確認してください。</p> : null}
    {status.capacityBlocked ? <p className="error-text">記憶の容量上限に達しています。整理してください。</p> : null}
    {status.lastErrorKind === undefined ? null : <p className="error-text">記憶の処理に失敗しました（{status.lastErrorKind}）。{status.retryAt === undefined ? "" : `次回: ${status.retryAt}`}</p>}
    {catalog?.candidates.map((candidate) => <div key={candidate.id} className="memory-item"><p>{candidate.text}</p><div className="button-row"><button type="button" onClick={() => void run(desktopApi.confirmMemory(candidate.id, crypto.randomUUID()))}>確認して覚える</button><button type="button" onClick={() => void run(desktopApi.rejectMemory(candidate.id))}>却下</button></div></div>)}
    {catalog?.updates.map((update) => <div key={update.id} className="memory-item"><p>{update.reason}</p>{update.replacement === undefined ? null : <p>{update.replacement}</p>}<div className="button-row"><button type="button" onClick={() => void run(desktopApi.confirmMemoryUpdate(update.id, crypto.randomUUID()))}>整理案を反映</button><button type="button" onClick={() => void run(desktopApi.rejectMemoryUpdate(update.id))}>却下</button></div></div>)}
    <h3>確認済みの事実</h3>
    {catalog?.facts.length === 0 ? <p className="muted">まだありません。</p> : catalog?.facts.map((fact) => <div key={fact.id} className="memory-item"><span>{fact.text}</span><button type="button" onClick={() => void run(desktopApi.deleteMemory(fact.id, crypto.randomUUID()))}>削除</button></div>)}
    <h3>日次 / 週次</h3>
    {[...(catalog?.daily ?? []), ...(catalog?.weekly ?? [])].map((summary) => <details key={summary.localDate ?? summary.period}><summary>{summary.localDate ?? summary.period}{summary.state === "stale" ? "（更新待ち）" : ""}</summary><p>{summary.text}</p></details>)}
    <button type="button" onClick={() => void run(desktopApi.consolidateMemory(yesterday()))}>昨日を整理する</button>
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
