import type { ReactElement } from "react";

import { useState, type FormEvent } from "react";

import { useI18n } from "../i18n/index.js";
import { audioPhaseLabel, permissionLabel } from "../settings-form.js";
import type { IpcResult, SpeakerManagementPayload } from "../types.js";
import { ConfirmationDialog } from "./ConfirmationDialog.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { BooleanInput } from "./SettingsControls.js";

interface Props extends SettingsCategoryProps {
  readonly onOpenSpeechSettings: (kind: "microphone" | "recognition") => void;
  readonly onSpeakerManagement?: (payload: SpeakerManagementPayload) => Promise<IpcResult<null>>;
}

export function HearingSettings({ form, snapshot, update, onOpenSpeechSettings, onSpeakerManagement }: Props): ReactElement {
  const { locale, t } = useI18n();
  const [mergeFrom, setMergeFrom] = useState("");
  const [mergeTo, setMergeTo] = useState("");
  const [reregisterID, setReregisterID] = useState("");
  const [deleteID, setDeleteID] = useState("");
  const [confirmation, setConfirmation] = useState<"delete" | "deleteAll">();
  const [busy, setBusy] = useState(false);
  const [managementMessage, setManagementMessage] = useState<string>();
  const runManagement = async (payload: SpeakerManagementPayload): Promise<void> => {
    if (onSpeakerManagement === undefined || busy) return;
    setBusy(true);
    try {
      const result = await onSpeakerManagement(payload);
      setManagementMessage(result.ok ? t("settings.hearing.speakerManagementSucceeded") : result.error.message);
    } catch (error: unknown) {
      setManagementMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };
  const merge = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    void runManagement({ operation: "merge", sourceId: mergeFrom.trim(), targetId: mergeTo.trim() });
  };
  const undoMerge = (): void => {
    void runManagement({ operation: "undoMerge", sourceId: mergeFrom.trim() });
  };
  const reregister = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    void runManagement({ operation: "reregister", speakerId: reregisterID.trim() });
  };
  const requestDelete = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    setConfirmation("delete");
  };
  const confirmManagement = (): void => {
    const operation = confirmation;
    setConfirmation(undefined);
    if (operation === "delete") {
      void runManagement({ operation, speakerId: deleteID.trim() });
    } else if (operation === "deleteAll") {
      void runManagement({ operation });
    }
  };
  return <>
    <fieldset id="settings-audio"><legend>{t("settings.hearing.source")}</legend>
      <BooleanInput label={t("settings.hearing.microphone")} path="audio.mic" value={form.audioMic} update={(value) => update("audioMic", value)} />
      <BooleanInput label={t("settings.hearing.speaker")} path="audio.speaker" value={form.audioSpeaker} update={(value) => update("audioSpeaker", value)} />
      <BooleanInput label={t("settings.hearing.speakerIdentification")} path="audio.speakerIdentification.enabled" value={form.audioSpeakerIdentificationEnabled} update={(value) => update("audioSpeakerIdentificationEnabled", value)} />
      <p className="field-help">{t("settings.hearing.help")}</p>
      <p className="field-help">{t("settings.hearing.speakerIdentificationHelp")}</p>
      {snapshot.audio.screenCapturePermission === "not-required" ? <>
        <p className="field-help">{t("settings.hearing.systemAudioStatus", { phase: audioPhaseLabel(snapshot.audio.phase, locale), microphone: permissionLabel(snapshot.audio.microphonePermission, locale) })}</p>
        <p className="field-help">{t("settings.hearing.systemAudioPermissionHelp")}</p>
        {snapshot.audio.warningKind?.startsWith("system-audio") && snapshot.audio.message && <p className="field-help" role="status">{snapshot.audio.message}</p>}
      </> : <p className="field-help">{t("settings.hearing.status", { phase: audioPhaseLabel(snapshot.audio.phase, locale), microphone: permissionLabel(snapshot.audio.microphonePermission, locale), screen: permissionLabel(snapshot.audio.screenCapturePermission, locale) })}</p>}
    </fieldset>
    <fieldset id="settings-hearing-permissions">
      <legend>{t("settings.hearing.transcription")}</legend>
      <p className="field-help">{t("settings.hearing.microphonePermission", { permission: permissionLabel(snapshot.audio.microphonePermission, locale) })} <button type="button" onClick={() => onOpenSpeechSettings("microphone")}>{t("common.openSettings")}</button></p>
      <p className="field-help">{t("settings.hearing.recognitionPermission", { permission: permissionLabel(snapshot.audio.recognitionPermission, locale) })} <button type="button" onClick={() => onOpenSpeechSettings("recognition")}>{t("common.openSettings")}</button></p>
    </fieldset>
    {onSpeakerManagement === undefined ? null : <fieldset id="settings-speaker-management">
      <legend>{t("settings.hearing.speakerManagement")}</legend>
      <p className="field-help">{t("settings.hearing.speakerManagementHelp")}</p>
      <form onSubmit={merge}>
        <label>{t("settings.hearing.mergeFrom")}<input value={mergeFrom} placeholder="speaker-1" onChange={(event) => setMergeFrom(event.target.value)} /></label>
        <label>{t("settings.hearing.mergeTo")}<input value={mergeTo} placeholder="speaker-2" onChange={(event) => setMergeTo(event.target.value)} /></label>
        <div className="button-row">
          <button type="submit" disabled={busy}>{t("settings.hearing.merge")}</button>
          <button type="button" disabled={busy} onClick={undoMerge}>{t("settings.hearing.undoMerge")}</button>
        </div>
      </form>
      <form onSubmit={reregister}>
        <label>{t("settings.hearing.reregisterID")}<input value={reregisterID} placeholder="speaker-1" onChange={(event) => setReregisterID(event.target.value)} /></label>
        <button type="submit" disabled={busy}>{t("settings.hearing.reregister")}</button>
      </form>
      <form onSubmit={requestDelete}>
        <label>{t("settings.hearing.deleteID")}<input value={deleteID} placeholder="speaker-1" onChange={(event) => setDeleteID(event.target.value)} /></label>
        <button type="submit" disabled={busy}>{t("settings.hearing.delete")}</button>
      </form>
      <button type="button" disabled={busy} onClick={() => setConfirmation("deleteAll")}>{t("settings.hearing.deleteAll")}</button>
      {managementMessage === undefined ? null : <p className="field-help" role="status">{managementMessage}</p>}
    </fieldset>}
    {confirmation === "delete" ? <ConfirmationDialog id="speaker-delete" title={t("settings.hearing.deleteTitle")} description={t("settings.hearing.deleteDescription", { id: deleteID.trim() })} cancelLabel={t("common.cancel")} confirmLabel={t("settings.hearing.delete")} onCancel={() => setConfirmation(undefined)} onConfirm={confirmManagement} /> : confirmation === "deleteAll" ? <ConfirmationDialog id="speaker-delete-all" title={t("settings.hearing.deleteAllTitle")} description={t("settings.hearing.deleteAllDescription")} cancelLabel={t("common.cancel")} confirmLabel={t("settings.hearing.deleteAll")} onCancel={() => setConfirmation(undefined)} onConfirm={confirmManagement} /> : null}
  </>;
}
