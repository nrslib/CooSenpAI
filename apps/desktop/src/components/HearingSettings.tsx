import type { ReactElement } from "react";

import { audioPhaseLabel, permissionLabel } from "../settings-form.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { BooleanInput } from "./SettingsControls.js";

interface Props extends SettingsCategoryProps {
  readonly onOpenSpeechSettings: (kind: "microphone" | "recognition") => void;
}

export function HearingSettings({ form, snapshot, update, onOpenSpeechSettings }: Props): ReactElement {
  return <>
    <fieldset id="settings-audio"><legend>音源</legend>
      <BooleanInput label="音を聞く" path="audio.enabled" value={form.audioEnabled} update={(value) => update("audioEnabled", value)} />
      <BooleanInput label="スピーカーの音（会議や動画）を聞く" path="audio.speaker" value={form.audioSpeaker} update={(value) => update("audioSpeaker", value)} />
      <p className="field-help">スピーカーの音（会議や動画）を聞きます。</p>
      <p className="field-help">状態: {audioPhaseLabel(snapshot.audio.phase)} ・ 画面収録 {permissionLabel(snapshot.audio.screenCapturePermission)}</p>
    </fieldset>
    <fieldset id="settings-hearing-permissions">
      <legend>文字起こし</legend>
      <p className="field-help">マイク: {permissionLabel(snapshot.speech.microphonePermission)} <button type="button" onClick={() => onOpenSpeechSettings("microphone")}>設定を開く</button></p>
      <p className="field-help">音声認識: {permissionLabel(snapshot.speech.recognitionPermission)} <button type="button" onClick={() => onOpenSpeechSettings("recognition")}>設定を開く</button></p>
    </fieldset>
  </>;
}
