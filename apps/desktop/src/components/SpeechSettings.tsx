import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import type { FormState } from "../settings-form.js";
import { SettingsSearchItem } from "../settings-search.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { BooleanInput } from "./SettingsControls.js";

export function SpeechSettings({ form, snapshot, update }: SettingsCategoryProps): ReactElement {
  const { t } = useI18n();
  return <fieldset id="settings-speech"><legend>{t("settings.speechSettings.heading")}</legend>
    <SettingsSearchItem label={t("settings.speechSettings.inputDevice")} path="speech.inputDevice">
      <label><span>{t("settings.speechSettings.inputDevice")}</span><select id="setting-speech-inputDevice" value={form.speechInputDevice} onChange={(event) => update("speechInputDevice", event.target.value)}><option value="default">{t("settings.speechSettings.systemDefault")}</option>{snapshot.speech.inputDevices.map((device) => <option key={device.id} value={device.id}>{device.name}</option>)}</select></label>
    </SettingsSearchItem>
    <SettingsSearchItem label={t("settings.speechSettings.mode")} path="speech.mode" description={t("settings.speechSettings.modeHelp")}>
      <label><span>{t("settings.speechSettings.mode")}</span><select id="setting-speech-mode" value={form.speechMode} onChange={(event) => update("speechMode", event.target.value as FormState["speechMode"])}><option value="toggle">{t("settings.speechSettings.toggle")}</option><option value="pushToTalk">{t("settings.speechSettings.pushToTalk")}</option></select></label>
    </SettingsSearchItem>
    <BooleanInput label={t("settings.speechSettings.confirmBeforeSend")} path="speech.confirmBeforeSend" value={form.speechConfirmBeforeSend} update={(value) => update("speechConfirmBeforeSend", value)} />
    <p className="field-help">{t("settings.speechSettings.help")}</p>
  </fieldset>;
}
