import { useEffect, useMemo, useState, type ReactElement } from "react";

import { desktopApi } from "../ipc.js";
import { useI18n } from "../i18n/index.js";
import type { RunningApplication, WatchAppConfig } from "../types.js";
import { SettingsSearchItem } from "../settings-search.js";

interface Props {
  readonly fullscreen: boolean;
  readonly apps: readonly WatchAppConfig[];
  readonly highlight: boolean;
  readonly updateFullscreen: (enabled: boolean) => void;
  readonly updateApps: (apps: readonly WatchAppConfig[]) => void;
}

export function WatchTargets({ fullscreen, apps, highlight, updateFullscreen, updateApps }: Props): ReactElement {
  const { t } = useI18n();
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
    <legend>{t("settings.watchTargets.heading")}</legend>
    <p className="field-help">{t("settings.watchTargets.help")}</p>
    <SettingsSearchItem label={t("settings.watchTargets.fullscreen")} path="watch.fullscreen" description={t("settings.watchTargets.fullscreenDescription")}>
      <label className="boolean-field"><span>{t("settings.watchTargets.fullscreen")}</span><input id="setting-watch-fullscreen" type="checkbox" checked={fullscreen} onChange={(event) => updateFullscreen(event.target.checked)} /></label>
    </SettingsSearchItem>
    <div className="watch-target-list">
      {apps.map((application, index) => <SettingsSearchItem key={application.bundleId} label={application.name} path={`watch.apps[${index}]`} description={application.bundleId}>
        <div className="watch-target">
          <ApplicationIcon application={applications.find((candidate) => candidate.bundleId === application.bundleId)} fallback={application.name} />
          <span><strong>{application.name}</strong><small>{application.bundleId}</small></span>
          <input aria-label={t("settings.watchTargets.watching", { name: application.name })} type="checkbox" checked={application.enabled} onChange={(event) => updateApps(apps.map((target) => target.bundleId === application.bundleId ? { ...target, enabled: event.target.checked } : target))} />
          <button type="button" onClick={() => updateApps(apps.filter((target) => target.bundleId !== application.bundleId))}>{t("common.delete")}</button>
        </div>
      </SettingsSearchItem>)}
      {apps.length === 0 ? <p className="muted">{t("settings.watchTargets.noApps")}</p> : null}
    </div>
    <button id="watch-target-add" type="button" onClick={() => void openPicker()}>{t("settings.watchTargets.addRunning")}</button>
    {adding ? <div className="application-picker">
      <div className="application-picker-heading"><strong>{t("settings.watchTargets.addHeading")}</strong><button type="button" onClick={() => setAdding(false)}>{t("settings.watchTargets.close")}</button></div>
      <input autoFocus placeholder={t("settings.watchTargets.search")} value={query} onChange={(event) => setQuery(event.target.value)} />
      <div className="application-picker-list">{candidates.map((application) => <button type="button" key={application.bundleId} onClick={() => {
        updateApps([...apps, { bundleId: application.bundleId, name: application.name, enabled: true }]);
        setAdding(false);
      }}><ApplicationIcon application={application} fallback={application.name} /><span>{application.name}<small>{application.bundleId}</small></span></button>)}</div>
    </div> : null}
    <div className="coming-soon"><span>{t("settings.watchTargets.microphone")}</span><span>{t("settings.watchTargets.soon")}</span></div>
    <div className="coming-soon"><span>{t("settings.watchTargets.speaker")}</span><span>{t("settings.watchTargets.soon")}</span></div>
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
