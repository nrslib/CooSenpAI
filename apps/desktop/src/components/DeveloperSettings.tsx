import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { BooleanInput, TextInput } from "./SettingsControls.js";
import { SettingsSearchItem } from "../settings-search.js";
import type { DebugWakeView } from "../useSettingsPresenter.js";

const MAX_DEBUG_WAKE_CONTEXT_BYTES = 32 * 1024;

interface DeveloperSettingsProps extends SettingsCategoryProps {
  readonly debugWake: DebugWakeView;
  readonly onDebugWakeImage: (file: File | undefined) => void;
  readonly onDebugWakeContext: (value: string) => void;
  readonly onDebugWakeSend: () => void;
}

export function DeveloperSettings({ form, saving, update, errorFor, debugWake, onDebugWakeImage, onDebugWakeContext, onDebugWakeSend }: DeveloperSettingsProps): ReactElement {
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
      <SettingsSearchItem label={t("settings.developer.judgeFollow")} path="judge.follow" description={t("settings.developer.judgeFollowHelp")}>
        <label className="boolean-field">
          <span>{t("settings.developer.judgeFollow")}</span>
          <input id="setting-judge-follow" type="checkbox" checked={form.judgeFollow} disabled={saving} onChange={(event) => update("judgeFollow", event.target.checked)} />
          {errorFor("judge.follow") === undefined ? null : <span className="field-error">{errorFor("judge.follow")}</span>}
        </label>
        <p className="field-help">{t("settings.developer.judgeFollowHelp")}</p>
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
