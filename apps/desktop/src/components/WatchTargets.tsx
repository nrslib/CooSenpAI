import { useEffect, useMemo, useState, type ReactElement } from "react";

import { desktopApi } from "../ipc.js";
import type { RunningApplication, WatchAppConfig } from "../types.js";

interface Props {
  readonly fullscreen: boolean;
  readonly apps: readonly WatchAppConfig[];
  readonly highlight: boolean;
  readonly updateFullscreen: (enabled: boolean) => void;
  readonly updateApps: (apps: readonly WatchAppConfig[]) => void;
}

export function WatchTargets({ fullscreen, apps, highlight, updateFullscreen, updateApps }: Props): ReactElement {
  const [applications, setApplications] = useState<readonly RunningApplication[]>([]);
  const [adding, setAdding] = useState(false);
  const [query, setQuery] = useState("");
  useEffect(() => {
    void desktopApi.listRunningApps().then((result) => { if (result.ok) setApplications(result.value); });
  }, []);
  const candidates = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase("ja-JP");
    return applications.filter((application) =>
      !apps.some((target) => target.bundleId === application.bundleId)
      && (normalized === ""
        || application.name.toLocaleLowerCase("ja-JP").includes(normalized)
        || application.bundleId.toLocaleLowerCase("ja-JP").includes(normalized)),
    );
  }, [applications, apps, query]);

  const openPicker = async (): Promise<void> => {
    const result = await desktopApi.listRunningApps();
    if (result.ok) {
      setApplications(result.value);
      setAdding(true);
    }
  };

  return <fieldset id="settings-watch-targets" className={highlight ? "tutorial-highlight" : undefined}>
    <legend>見ていいもの</legend>
    <p className="field-help">「見ています」の間、有効な対象だけを撮影します。見守りの ON/OFF は上部スイッチで変更し、次回起動時にも引き継ぎます。</p>
    <label className="boolean-field"><span>フルスクリーン</span><input id="setting-watch-fullscreen" type="checkbox" checked={fullscreen} onChange={(event) => updateFullscreen(event.target.checked)} /></label>
    <div className="watch-target-list">
      {apps.map((application) => <div className="watch-target" key={application.bundleId}>
        <ApplicationIcon application={applications.find((candidate) => candidate.bundleId === application.bundleId)} fallback={application.name} />
        <span><strong>{application.name}</strong><small>{application.bundleId}</small></span>
        <input aria-label={`${application.name}を見る`} type="checkbox" checked={application.enabled} onChange={(event) => updateApps(apps.map((target) => target.bundleId === application.bundleId ? { ...target, enabled: event.target.checked } : target))} />
        <button type="button" onClick={() => updateApps(apps.filter((target) => target.bundleId !== application.bundleId))}>削除</button>
      </div>)}
      {apps.length === 0 ? <p className="muted">アプリは追加されていません。</p> : null}
    </div>
    <button id="watch-target-add" type="button" onClick={() => void openPicker()}>起動中のアプリから追加</button>
    {adding ? <div className="application-picker">
      <div className="application-picker-heading"><strong>アプリを追加</strong><button type="button" onClick={() => setAdding(false)}>閉じる</button></div>
      <input autoFocus placeholder="アプリを検索" value={query} onChange={(event) => setQuery(event.target.value)} />
      <div className="application-picker-list">{candidates.map((application) => <button type="button" key={application.bundleId} onClick={() => {
        updateApps([...apps, { bundleId: application.bundleId, name: application.name, enabled: true }]);
        setAdding(false);
      }}><ApplicationIcon application={application} fallback={application.name} /><span>{application.name}<small>{application.bundleId}</small></span></button>)}</div>
    </div> : null}
    <div className="coming-soon"><span>マイク</span><span>近日対応</span></div>
    <div className="coming-soon"><span>スピーカー</span><span>近日対応</span></div>
  </fieldset>;
}

function ApplicationIcon({ application, fallback }: { readonly application?: RunningApplication; readonly fallback: string }): ReactElement {
  if (application === undefined || application.iconPng.length === 0) {
    return <span className="watch-target-icon" aria-hidden="true">{fallback.slice(0, 1)}</span>;
  }
  let binary = "";
  for (let index = 0; index < application.iconPng.length; index += 1) {
    binary += String.fromCharCode(application.iconPng[index] ?? 0);
  }
  return <img className="watch-target-icon" src={`data:image/png;base64,${btoa(binary)}`} alt="" />;
}
