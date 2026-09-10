import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import { audioPhaseLabel, permissionLabel } from "../settings-form.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { BooleanInput } from "./SettingsControls.js";

interface Props extends SettingsCategoryProps {
  readonly onOpenSpeechSettings: (kind: "microphone" | "recognition") => void;
}

export function HearingSettings({ form, snapshot, update, onOpenSpeechSettings }: Props): ReactElement {
  const { locale, t } = useI18n();
  return <>
    <fieldset id="settings-audio"><legend>{t("settings.hearing.source")}</legend>
      <BooleanInput label={t("settings.hearing.microphone")} path="audio.mic" value={form.audioMic} update={(value) => update("audioMic", value)} />
      <BooleanInput label={t("settings.hearing.speaker")} path="audio.speaker" value={form.audioSpeaker} update={(value) => update("audioSpeaker", value)} />
      <p className="field-help">{t("settings.hearing.help")}</p>
      {snapshot.audio.screenCapturePermission === "not-required" ? <>
        <p className="field-help">{t("settings.hearing.systemAudioStatus", { phase: audioPhaseLabel(snapshot.audio.phase, locale), microphone: permissionLabel(snapshot.audio.microphonePermission, locale) })}</p>
        <p className="field-help">{t("settings.hearing.systemAudioPermissionHelp")}</p>
        {snapshot.audio.warningKind?.startsWith("system-audio") && snapshot.audio.message && <p className="field-help" role="status">{snapshot.audio.message}</p>}
      </> : <p className="field-help">{t("settings.hearing.status", { phase: audioPhaseLabel(snapshot.audio.phase, locale), microphone: permissionLabel(snapshot.audio.microphonePermission, locale), screen: permissionLabel(snapshot.audio.screenCapturePermission, locale) })}</p>}
    </fieldset>
    <fieldset id="settings-hearing-permissions">
      <legend>{t("settings.hearing.transcription")}</legend>
      <p className="field-help">{t("settings.hearing.microphonePermission", { permission: permissionLabel(snapshot.speech.microphonePermission, locale) })} <button type="button" onClick={() => onOpenSpeechSettings("microphone")}>{t("common.openSettings")}</button></p>
      <p className="field-help">{t("settings.hearing.recognitionPermission", { permission: permissionLabel(snapshot.speech.recognitionPermission, locale) })} <button type="button" onClick={() => onOpenSpeechSettings("recognition")}>{t("common.openSettings")}</button></p>
    </fieldset>
  </>;
}
