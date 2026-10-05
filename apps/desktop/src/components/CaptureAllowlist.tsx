import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import { SettingsSearchItem } from "../settings-search.js";
import type { CaptureAllowedApp } from "../types.js";
import type { AppPickerView } from "../useSettingsPresenter.js";
import { ApplicationIcon } from "./WatchTargets.js";

interface Props {
  readonly enabled: boolean;
  readonly apps: readonly CaptureAllowedApp[];
  readonly osMajorVersion: number;
  readonly updateEnabled: (enabled: boolean) => void;
  readonly updateApps: (apps: readonly CaptureAllowedApp[]) => void;
  readonly picker: AppPickerView;
  readonly onPickerAction: (name: string, value: unknown) => void;
}

export function CaptureAllowlist({ enabled, apps, osMajorVersion, updateEnabled, updateApps, picker, onPickerAction }: Props): ReactElement {
  const { t } = useI18n();

  return <fieldset id="settings-capture-allowlist">
    <legend>{t("settings.captureAllowlist.heading")}</legend>
    <p className="field-help">{t("settings.captureAllowlist.help")}</p>
    <SettingsSearchItem label={t("settings.captureAllowlist.enabled")} path="capture.allowlist.enabled">
      <label className="boolean-field"><span>{t("settings.captureAllowlist.enabled")}</span><input id="setting-capture-allowlist-enabled" type="checkbox" checked={enabled} disabled={osMajorVersion < 14 && !enabled} onChange={(event) => updateEnabled(event.target.checked)} /></label>
    </SettingsSearchItem>
    {osMajorVersion < 14 ? <p className="field-help">{t("settings.captureAllowlist.requiresMacOS14")}</p> : null}
    <div className="watch-target-list">
      {apps.map((app, index) => <SettingsSearchItem key={app.bundleId} label={app.name} path={`capture.allowlist.apps[${index}]`} description={app.bundleId}>
        <div className="watch-target"><ApplicationIcon application={picker.applications.find((candidate) => candidate.bundleId === app.bundleId)} fallback={app.name} /><span><strong>{app.name}</strong><small>{app.bundleId}</small></span><button type="button" onClick={() => updateApps(apps.filter((item) => item.bundleId !== app.bundleId))}>{t("common.delete")}</button></div>
      </SettingsSearchItem>)}
      {apps.length === 0 ? <p className="muted">{t("settings.captureAllowlist.empty")}</p> : null}
    </div>
    <button id="capture-allowlist-add" type="button" onClick={() => onPickerAction("appPickerOpen", "allowlist")}>{t("settings.watchTargets.addRunning")}</button>
    {picker.target === "allowlist" && picker.error !== null ? <p role="alert">{picker.error}</p> : null}
    {picker.target === "allowlist" && !picker.loading && picker.error === null ? <div className="application-picker">
      <div className="application-picker-heading"><strong>{t("settings.watchTargets.addHeading")}</strong><button type="button" onClick={() => onPickerAction("appPickerClose", null)}>{t("settings.watchTargets.close")}</button></div>
      <input autoFocus placeholder={t("settings.watchTargets.search")} value={picker.query} onChange={(event) => onPickerAction("appPickerQuery", event.target.value)} />
      <div className="application-picker-list">{picker.applications.filter((candidate) => picker.candidateBundleIds.includes(candidate.bundleId)).map((candidate) => <button type="button" key={candidate.bundleId} onClick={() => {
        updateApps([...apps, { bundleId: candidate.bundleId, name: candidate.name }]);
        onPickerAction("appPickerClose", null);
      }}><ApplicationIcon application={candidate} fallback={candidate.name} /><span>{candidate.name}<small>{candidate.bundleId}</small></span></button>)}</div>
    </div> : null}
  </fieldset>;
}
