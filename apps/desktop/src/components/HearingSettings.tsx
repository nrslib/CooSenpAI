import type { ReactElement } from "react";

import { useEffect, useRef, type FormEvent } from "react";

import { useI18n } from "../i18n/index.js";
import { audioPhaseLabel, permissionLabel } from "../settings-form.js";
import type { MergedSpeakers, SpeakerSummary } from "../types.js";
import { speakerDisplayName } from "../view-model.js";
import type { SpeakerManagementView } from "../useSettingsPresenter.js";
import { ConfirmationDialog } from "./ConfirmationDialog.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { BooleanInput } from "./SettingsControls.js";

interface Props extends SettingsCategoryProps {
  readonly onOpenSpeechSettings: (kind: "microphone" | "recognition") => void;
  readonly speaker?: SpeakerManagementView;
  readonly speakerConfirmation?: "speaker-delete" | "speaker-delete-all" | null;
  readonly onSpeakerAction?: (name: string, value?: unknown) => void;
}

function SpeakerSelect({ label, candidates, value, disabled, onChange }: {
  readonly label: string;
  readonly candidates: readonly SpeakerSummary[];
  readonly value: string;
  readonly disabled: boolean;
  readonly onChange: (id: string) => void;
}): ReactElement {
  const { t } = useI18n();
  return <label>{label}
    <select value={value} disabled={disabled} onChange={(event) => onChange(event.target.value)}>
      <option value="">{t("settings.hearing.speakerSelect")}</option>
      {candidates.map((candidate) => <option key={candidate.id} value={candidate.id}>{speakerDisplayName(candidate.id, candidate.name)}</option>)}
    </select>
  </label>;
}

function mergedPairLabel(pair: MergedSpeakers): string {
  return `${speakerDisplayName(pair.sourceId, pair.sourceName)} → ${speakerDisplayName(pair.targetId, pair.targetName)}`;
}

// 描画と編集値・操作イベントだけを持つ。操作の可否・busy・成否・確認の遷移は Settings Presenter が決める。
function SpeakerManagementSection({ speaker, confirmation, onSpeakerAction }: {
  readonly speaker: SpeakerManagementView;
  readonly confirmation: "speaker-delete" | "speaker-delete-all" | null;
  readonly onSpeakerAction: (name: string, value?: unknown) => void;
}): ReactElement {
  const { t } = useI18n();
  const candidates = speaker.speakers ?? [];
  const merged = speaker.merged ?? [];
  const busy = speaker.busy;
  const controls = speaker.controls;
  const { mergeFrom: source = "", mergeTo: target = "", undoSource: undo = "", renameID: rename = "", nameDraft = "", reregisterID: reregister = "", deleteID: deleteTarget = "" } = controls.draft;
  const actionRef = useRef(onSpeakerAction);
  actionRef.current = onSpeakerAction;
  useEffect(() => {
    actionRef.current("speakerViewMounted");
    return () => actionRef.current("speakerViewUnmounted");
  }, []);
  const field = (name: string, value: string): void => onSpeakerAction("speakerField", { field: name, value });
  const deleteTargetLabel = speaker.deleteTarget === null
    ? ""
    : speakerDisplayName(speaker.deleteTarget, candidates.find((candidate) => candidate.id === speaker.deleteTarget)?.name);

  const submitMerge = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    onSpeakerAction("speakerMerge", { sourceId: source, targetId: target });
  };
  const submitUndo = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    onSpeakerAction("speakerUndoMerge", undo);
  };
  const submitRename = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    onSpeakerAction("speakerRename", { speakerId: rename, name: nameDraft });
  };
  const submitReregister = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    onSpeakerAction("speakerReregister", reregister);
  };
  const submitDelete = (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    onSpeakerAction("speakerDelete", deleteTarget);
  };

  return <fieldset id="settings-speaker-management">
    <legend>{t("settings.hearing.speakerManagement")}</legend>
    <p className="field-help">{t("settings.hearing.speakerManagementHelp")}</p>
    {speaker.directoryError === null ? null : <p className="field-help" role="alert">{speaker.directoryError}</p>}
    {!controls.mergeAvailable
      ? <p className="field-help">{t("settings.hearing.mergeEmpty")}</p>
      : <form onSubmit={submitMerge}>
        <SpeakerSelect label={t("settings.hearing.mergeFrom")} candidates={candidates} value={source} disabled={busy} onChange={(id) => field("mergeFrom", id)} />
        <SpeakerSelect label={t("settings.hearing.mergeTo")} candidates={candidates} value={target} disabled={busy} onChange={(id) => field("mergeTo", id)} />
        <div className="button-row">
          <button type="submit" disabled={!controls.canMerge}>{t("settings.hearing.merge")}</button>
        </div>
      </form>}
    {!controls.undoAvailable
      ? <p className="field-help">{t("settings.hearing.undoMergeEmpty")}</p>
      : <form onSubmit={submitUndo}>
        <label>{t("settings.hearing.undoMergeLabel")}
          <select value={undo} disabled={busy} onChange={(event) => field("undoSource", event.target.value)}>
            <option value="">{t("settings.hearing.speakerSelect")}</option>
            {merged.map((pair) => <option key={pair.sourceId} value={pair.sourceId}>{mergedPairLabel(pair)}</option>)}
          </select>
        </label>
        <div className="button-row">
          <button type="submit" disabled={!controls.canUndo}>{t("settings.hearing.undoMerge")}</button>
        </div>
      </form>}
    <p className="field-help">{t("settings.hearing.speakerNameHelp")}</p>
    <form onSubmit={submitRename}>
      <SpeakerSelect label={t("settings.hearing.speakerNameTarget")} candidates={candidates} value={rename} disabled={busy} onChange={(id) => field("renameID", id)} />
      <label>{t("settings.hearing.speakerName")}<input value={nameDraft} maxLength={40} disabled={!controls.canEditName} onChange={(event) => field("nameDraft", event.target.value)} /></label>
      <div className="button-row">
        <button type="submit" disabled={!controls.canRename}>{t("settings.hearing.speakerNameSave")}</button>
        <button type="button" disabled={!controls.canClear} onClick={() => onSpeakerAction("speakerRename", { speakerId: rename, name: null })}>{t("settings.hearing.speakerNameClear")}</button>
      </div>
    </form>
    <form onSubmit={submitReregister}>
      <SpeakerSelect label={t("settings.hearing.reregisterID")} candidates={candidates} value={reregister} disabled={busy} onChange={(id) => field("reregisterID", id)} />
      <button type="submit" disabled={!controls.canReregister}>{t("settings.hearing.reregister")}</button>
    </form>
    <form onSubmit={submitDelete}>
      <SpeakerSelect label={t("settings.hearing.deleteID")} candidates={candidates} value={deleteTarget} disabled={busy} onChange={(id) => field("deleteID", id)} />
      <button type="submit" disabled={!controls.canDelete}>{t("settings.hearing.delete")}</button>
    </form>
    <button type="button" disabled={busy} onClick={() => onSpeakerAction("speakerDeleteAll")}>{t("settings.hearing.deleteAll")}</button>
    {speaker.error === null ? null : <p className="field-help" role="alert">{speaker.error}</p>}
    {!speaker.succeeded || busy ? null : <p className="field-help" role="status">{t("settings.hearing.speakerManagementSucceeded")}</p>}
    {confirmation === "speaker-delete" ? <ConfirmationDialog id="speaker-delete" title={t("settings.hearing.deleteTitle")} description={t("settings.hearing.deleteDescription", { id: deleteTargetLabel })} cancelLabel={t("common.cancel")} confirmLabel={t("settings.hearing.delete")} onCancel={() => onSpeakerAction("cancelConfirmation")} onConfirm={() => onSpeakerAction("acceptConfirmation")} /> : confirmation === "speaker-delete-all" ? <ConfirmationDialog id="speaker-delete-all" title={t("settings.hearing.deleteAllTitle")} description={t("settings.hearing.deleteAllDescription")} cancelLabel={t("common.cancel")} confirmLabel={t("settings.hearing.deleteAll")} onCancel={() => onSpeakerAction("cancelConfirmation")} onConfirm={() => onSpeakerAction("acceptConfirmation")} /> : null}
  </fieldset>;
}

export function HearingSettings({ form, snapshot, update, onOpenSpeechSettings, speaker, speakerConfirmation, onSpeakerAction }: Props): ReactElement {
  const { locale, t } = useI18n();
  return <>
    <fieldset id="settings-audio"><legend>{t("settings.hearing.source")}</legend>
      <BooleanInput label={t("settings.hearing.microphone")} path="audio.mic" value={form.audioMic} update={(value) => update("audioMic", value)} />
      <BooleanInput label={t("settings.hearing.microphoneCommands")} path="audio.microphoneCommandsEnabled" value={form.audioMicrophoneCommandsEnabled} update={(value) => update("audioMicrophoneCommandsEnabled", value)} />
      <p className="field-help">{t("settings.hearing.microphoneCommandsHelp")}</p>
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
    {speaker === undefined || onSpeakerAction === undefined ? null : <SpeakerManagementSection speaker={speaker} confirmation={speakerConfirmation ?? null} onSpeakerAction={onSpeakerAction} />}
  </>;
}
