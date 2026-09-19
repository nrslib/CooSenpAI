import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { BooleanInput, TextInput } from "./SettingsControls.js";
import { SettingsSearchItem } from "../settings-search.js";

export function DeveloperSettings({ form, saving, update, errorFor }: SettingsCategoryProps): ReactElement {
  const { t } = useI18n();
  return <>
    <fieldset id="settings-developer">
      <legend>{t("settings.developer.heading")}</legend>
      <p className="field-help">{t("settings.developer.description")}</p>
      <TextInput label={t("settings.developer.ocrExecutable")} path="watch.ocrGate.executable" value={form.ocrGateExecutable} update={(value) => update("ocrGateExecutable", value)} />
      <TextInput label={t("settings.developer.observerExecutable")} path="observer.vision.executable" value={form.observerExecutable} update={(value) => update("observerExecutable", value)} />
      <TextInput label={t("settings.developer.hearingExecutable")} path="observer.hearing.executable" value={form.hearingExecutable} update={(value) => update("hearingExecutable", value)} />
      <TextInput label={t("settings.developer.companionExecutable")} path="companion.executable" value={form.companionExecutable} update={(value) => update("companionExecutable", value)} />
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
    </fieldset>
  </>;
}
