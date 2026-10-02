import type { ReactElement } from "react";

import { DeveloperBrainActivity } from "../developer/DeveloperBrainActivity.js";
import { desktopApi } from "../ipc.js";
import { useI18n } from "../i18n/index.js";
import type { AppSnapshot } from "../types.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { BooleanInput, TextInput } from "./SettingsControls.js";
import { SettingsSearchItem } from "../settings-search.js";
import type { DebugWakeView } from "../useSettingsPresenter.js";

const MAX_DEBUG_WAKE_CONTEXT_BYTES = 32 * 1024;

interface DeveloperSettingsProps extends SettingsCategoryProps {
  readonly connectomeStatus?: AppSnapshot["connectomeStatus"];
  readonly connectomeDownload: AppSnapshot["connectomeDownload"];
  readonly debugWake: DebugWakeView;
  readonly onDebugWakeImage: (file: File | undefined) => void;
  readonly onDebugWakeContext: (value: string) => void;
  readonly onDebugWakeSend: () => void;
}

export function DeveloperSettings({ form, saving, update, errorFor, connectomeStatus, connectomeDownload, debugWake, onDebugWakeImage, onDebugWakeContext, onDebugWakeSend }: DeveloperSettingsProps): ReactElement {
  const { t } = useI18n();
  const busy = debugWake.phase === "loading" || debugWake.phase === "processing";
  const busyText = debugWake.phase === "loading" ? t("settings.developer.debugWakeReading") : t("settings.developer.debugWakeProcessing");
  return <>
    <fieldset id="settings-developer">
      <legend>{t("settings.developer.heading")}</legend>
      <p className="field-help">{t("settings.developer.description")}</p>
      <TextInput label={t("settings.developer.ocrExecutable")} path="watch.ocrGate.executable" value={form.ocrGateExecutable} update={(value) => update("ocrGateExecutable", value)} />
      <TextInput label={t("settings.developer.observerExecutable")} path="observer.vision.executable" value={form.observerExecutable} update={(value) => update("observerExecutable", value)} />
      <TextInput label={t("settings.developer.hearingExecutable")} path="observer.hearing.executable" value={form.hearingExecutable} update={(value) => update("hearingExecutable", value)} />
      <TextInput label={t("settings.developer.companionExecutable")} path="companion.executable" value={form.companionExecutable} update={(value) => update("companionExecutable", value)} />
      <BooleanInput label={t("settings.developer.feedbackEnabled")} path="debug.feedbackEnabled" value={form.feedbackEnabled} update={(value) => update("feedbackEnabled", value)} />
      <BooleanInput label={t("settings.developer.debugEnabled")} path="debug.enabled" value={form.debugEnabled} update={(value) => update("debugEnabled", value)} />
      <TextInput label={t("settings.developer.audioDebugDumpDir")} path="audio.debugDumpDir" value={form.audioDebugDumpDir} update={(value) => update("audioDebugDumpDir", value)} />
      <SettingsSearchItem label={t("settings.developer.bundledConnectome")} path="judge.bundledConnectome" description={t("settings.developer.bundledConnectomeHelp")}>
        <label className="boolean-field">
          <span>{t("settings.developer.bundledConnectome")}</span>
          <input id="setting-judge-bundled-connectome" type="checkbox" checked={form.judgeBundledConnectome} disabled={saving} onChange={(event) => update("judgeBundledConnectome", event.target.checked)} />
          {errorFor("judge.bundledConnectome") === undefined ? null : <span className="field-error">{errorFor("judge.bundledConnectome")}</span>}
        </label>
        <p className="field-help">{t("settings.developer.bundledConnectomeHelp")}</p>
        {form.judgeBundledConnectome && connectomeStatus?.state === "manual" ? <p className="field-help">{t("settings.developer.bundledConnectomeManual")}</p> : null}
        {form.judgeBundledConnectome && connectomeStatus && connectomeStatus.state !== "manual" && connectomeStatus.state !== "off" ? <button type="button" disabled={connectomeStatus.state === "checking" || connectomeDownload.phase === "downloading" || connectomeDownload.phase === "verifying"} onClick={() => { void desktopApi.recheckConnectomePack().then((result) => { if (!result.ok) window.alert(result.error.message); }); }}>{t("settings.developer.bundledConnectomeRecheck")}</button> : null}
        {form.judgeBundledConnectome && connectomeStatus?.state === "checking" ? <p role="status">{t("settings.developer.bundledConnectomeChecking")}: <code>{connectomeStatus.reason}</code></p> : null}
        {form.judgeBundledConnectome && connectomeStatus?.state === "unavailable" ? <div className="field-help" role="status">
          <p>{t("settings.developer.bundledConnectomeUnavailable")}: <code>{connectomeStatus.reason}</code></p>
          {connectomeStatus.manifestSha256 ? <p>{t("settings.developer.bundledConnectomeManifest")}: <code>{connectomeStatus.manifestSha256}</code></p> : null}
          <p>{t("settings.developer.bundledConnectomePackPath")}: <code>{connectomeStatus.packPath}</code></p>
          <div className="button-row">
            {connectomeStatus.reason === "pack-missing" ? <button type="button" disabled={connectomeDownload.phase === "downloading" || connectomeDownload.phase === "verifying"} onClick={() => { void desktopApi.downloadConnectomePack().then((result) => { if (!result.ok) window.alert(result.error.message); }); }}>{t("settings.developer.bundledConnectomeDownload")}</button> : null}
            <button type="button" onClick={() => { void desktopApi.openConnectomePackDirectory().then((result) => { if (!result.ok) window.alert(result.error.message); }); }}>{t("settings.developer.bundledConnectomeOpenPath")}</button>
            <button type="button" onClick={() => { void desktopApi.copyConnectomePackPath().then((result) => { if (!result.ok) window.alert(result.error.message); }); }}>{t("settings.developer.bundledConnectomeCopyPath")}</button>
          </div>
        </div> : null}
        {connectomeDownload.phase === "downloading" ? <div role="status">
          <p>{t("settings.developer.bundledConnectomeProgress")}: {connectomeDownload.receivedBytes.toLocaleString()} / {connectomeDownload.totalBytes.toLocaleString()} bytes</p>
          <button type="button" onClick={() => { void desktopApi.cancelConnectomePackDownload(); }}>{t("settings.developer.bundledConnectomeCancel")}</button>
        </div> : null}
        {connectomeDownload.phase === "verifying" ? <div role="status"><p>{t("settings.developer.bundledConnectomeChecking")}: <code>{connectomeDownload.reason}</code></p><button type="button" onClick={() => { void desktopApi.cancelConnectomePackDownload(); }}>{t("settings.developer.bundledConnectomeCancel")}</button></div> : null}
        {connectomeDownload.phase === "failed" ? <p className="field-error" role="alert">{t("settings.developer.bundledConnectomeDownloadFailed")}: <code>{connectomeDownload.reason}</code></p> : null}
      </SettingsSearchItem>
      <SettingsSearchItem label={t("settings.developer.judgeFollow")} path="judge.follow" description={t("settings.developer.judgeFollowHelp")}>
        <label className="boolean-field">
          <span>{t("settings.developer.judgeFollow")}</span>
          <input id="setting-judge-follow" type="checkbox" checked={form.judgeFollow} disabled={saving} onChange={(event) => update("judgeFollow", event.target.checked)} />
          {errorFor("judge.follow") === undefined ? null : <span className="field-error">{errorFor("judge.follow")}</span>}
        </label>
        <p className="field-help">{t("settings.developer.judgeFollowHelp")}</p>
      </SettingsSearchItem>
      <SettingsSearchItem label={t("brain.title")} description={t("brain.developerDescription")}>
        <DeveloperBrainActivity />
      </SettingsSearchItem>
      <SettingsSearchItem label={t("settings.developer.debugWakeHeading")} path="debug.wake" description={t("settings.developer.debugWakeDescription")}>
        <div className="debug-wake-panel">
          <p className="field-help">{t("settings.developer.debugWakeDescription")}</p>
          <label>
            <span>{t("settings.developer.debugWakeImage")}</span>
            <input id="debug-wake-image" type="file" accept="image/png,image/jpeg" disabled={saving || busy} onClick={(event) => { event.currentTarget.value = ""; }} onChange={(event) => onDebugWakeImage(event.currentTarget.files?.[0])} />
          </label>
          {debugWake.selectedName === null ? <small>{t("settings.developer.debugWakeNoImage")}</small> : <small>{t("settings.developer.debugWakeSelected", { name: debugWake.selectedName })}</small>}
          <label>
            <span>{t("settings.developer.debugWakeContext")}</span>
            <textarea id="debug-wake-context" value={debugWake.context} maxLength={MAX_DEBUG_WAKE_CONTEXT_BYTES} disabled={saving || busy} placeholder={t("settings.developer.debugWakeContextPlaceholder")} onChange={(event) => onDebugWakeContext(event.target.value)} />
          </label>
          <button id="debug-wake-send" type="button" disabled={saving || debugWake.selectedName === null || busy} onClick={onDebugWakeSend}>
            {busy ? busyText : t("settings.developer.debugWakeSend")}
          </button>
          {debugWake.error === null ? null : <p className="field-error" role="alert">{debugWake.error}</p>}
          {busy ? <p role="status">{busyText}</p> : null}
          {debugWake.result === null ? null : <dl>
            <div><dt>{t("settings.developer.debugWakeStatus")}</dt><dd>{debugWake.result.status === "emitted" ? t("settings.developer.debugWakeSpeech") : debugWake.result.status === "silent" ? t("settings.developer.debugWakeSilence") : t("settings.developer.debugWakeDeferred")}</dd></div>
            <div><dt>{t("settings.developer.debugWakeSourceId")}</dt><dd>{debugWake.result.sourceId}</dd></div>
            <div><dt>{t("settings.developer.debugWakeCallId")}</dt><dd>{debugWake.result.callId ?? t("settings.developer.debugWakeNoValue")}</dd></div>
            <div><dt>{t("settings.developer.debugWakeEmit")}</dt><dd>{debugWake.result.status === "deferred" ? t("settings.developer.debugWakeDeferred") : debugWake.result.emit ? t("settings.developer.debugWakeSpeech") : t("settings.developer.debugWakeSilence")}</dd></div>
            <div><dt>{t("settings.developer.debugWakeThought")}</dt><dd>{debugWake.result.thought ?? t("settings.developer.debugWakeNoValue")}</dd></div>
            <div><dt>{t("settings.developer.debugWakeMessage")}</dt><dd>{debugWake.result.message ?? t("settings.developer.debugWakeNoValue")}</dd></div>
          </dl>}
        </div>
      </SettingsSearchItem>
    </fieldset>
  </>;
}
