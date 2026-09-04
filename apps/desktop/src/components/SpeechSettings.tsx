import type { ReactElement } from "react";

import type { FormState } from "../settings-form.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { BooleanInput } from "./SettingsControls.js";

export function SpeechSettings({ form, snapshot, update }: SettingsCategoryProps): ReactElement {
  return <fieldset id="settings-speech"><legend>音声入力</legend>
    <label><span>入力デバイス</span><select id="setting-speech-inputDevice" value={form.speechInputDevice} onChange={(event) => update("speechInputDevice", event.target.value)}><option value="default">システム既定</option>{snapshot.speech.inputDevices.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}</select></label>
    <label><span>マイクショートカットの操作（既定: 押すたび）</span><select id="setting-speech-mode" value={form.speechMode} onChange={(event) => update("speechMode", event.target.value as FormState["speechMode"])}><option value="toggle">押すたびに開始 / 停止</option><option value="pushToTalk">押している間だけ録音（主キーの解放を検知）</option></select></label>
    <BooleanInput label="送信前に文字起こしを確認する" path="speech.confirmBeforeSend" value={form.speechConfirmBeforeSend} update={(value) => update("speechConfirmBeforeSend", value)} />
    <p className="field-help">音声は端末内で文字にし、生音声は保存しません。入力欄のマイクは文字を挿入するだけで、自動送信しません。</p>
  </fieldset>;
}
