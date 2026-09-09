import type { ChangeEvent, RefObject, ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import { AppUpdatePanel } from "./AppUpdatePanel.js";
import type { PersonaOption } from "../types.js";
import { MemoryPanel } from "../MemoryPanel.js";
import { tutorialSettingsHighlight } from "../tutorial-ui.js";
import { fontPreset, inputId } from "../settings-form.js";
import type { FormState } from "../settings-form.js";
import { personaOptionLabel } from "./PersonaPicker.js";
import { SettingsSearchItem } from "../settings-search.js";
import { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { BooleanInput, NumberInput, ReminderEditor, SelectInput, TextInput } from "./SettingsControls.js";

interface Props extends SettingsCategoryProps {
  readonly personas: readonly PersonaOption[];
  readonly avatarInputRef: RefObject<HTMLInputElement | null>;
  readonly onSelectAvatar: (event: ChangeEvent<HTMLInputElement>) => void;
  readonly onResetAvatar: () => void;
  readonly onReloadPersona: () => void;
  readonly onEditPersona: () => void;
  readonly onOpenPersonaPicker: () => void;
}

export function GeneralSettings({ form, snapshot, saving, update, errorFor, personas, avatarInputRef, onSelectAvatar, onResetAvatar, onReloadPersona, onEditPersona, onOpenPersonaPicker }: Props): ReactElement {
  const { locale, t } = useI18n();
  const selectedPersona = personas.find((option) => option.id === form.persona);
  return <>
    <fieldset id="settings-companion" className={tutorialSettingsHighlight(snapshot.onboarding, "persona") ? "tutorial-highlight" : undefined}>
      <legend>{t("settings.companion.heading")}</legend>
      <p className="field-help">{t("settings.companion.description")}</p>
      <SettingsSearchItem label={t("settings.companion.displayName")} path="companion.displayName" description={t("settings.companion.description")}>
        <label><span>{t("settings.companion.displayName")}</span><input id="setting-companion-displayName" value={form.displayName} maxLength={20} disabled={saving} onChange={(event) => update("displayName", event.target.value)} />{errorFor("companion.displayName") === undefined ? null : <span className="field-error">{errorFor("companion.displayName")}</span>}</label>
      </SettingsSearchItem>
      <SettingsSearchItem label={t("settings.companion.avatarColor")} path="ui.avatarColor">
        <label><span>{t("settings.companion.avatarColor")}</span><input id="setting-ui-avatarColor" type="color" value={form.avatarColor} onChange={(event) => update("avatarColor", event.target.value)} /></label>
      </SettingsSearchItem>
      <SettingsSearchItem label={t("settings.companion.avatarImage")} path="ui.avatarPath" description={t("settings.companion.avatarImageDescription")}>
        <label><span>{t("settings.companion.avatarImage")}</span><input ref={avatarInputRef} id="setting-ui-avatarPath" type="file" accept="image/png,image/jpeg,.png,.jpg,.jpeg" disabled={saving} onChange={onSelectAvatar} /><small className={form.avatarImageLoadFailed ? "field-error" : undefined}>{form.avatarImageLoadFailed ? t("settings.companion.avatarImageLoadFailed") : form.avatarFileName === undefined ? form.avatarPath === null ? t("settings.companion.avatarUnset") : t("settings.companion.avatarConfigured") : t("settings.companion.avatarSelected", { file: form.avatarFileName })}</small></label>
      </SettingsSearchItem>
      <div className="button-row"><button type="button" disabled={saving || (form.avatarPath === null && form.avatarImage === undefined)} onClick={onResetAvatar}>{t("settings.companion.resetAvatar")}</button></div>
      <SettingsSearchItem label={t("settings.companion.persona")} path="companion.persona" description={t("settings.companion.personaDescription")}>
        <div className="persona-picker-trigger"><span>{t("settings.companion.persona")}</span><button id={inputId("companion.persona")} type="button" aria-haspopup="dialog" disabled={saving} onClick={onOpenPersonaPicker}><span>{selectedPersona === undefined ? form.persona : personaOptionLabel(selectedPersona, locale)}</span><span aria-hidden="true">{t("settings.companion.choosePersona")}</span></button>{errorFor("companion.persona") === undefined ? null : <span className="field-error">{errorFor("companion.persona")}</span>}</div>
      </SettingsSearchItem>
      <div className="button-row"><button type="button" disabled={saving} onClick={onReloadPersona}>{t("settings.companion.reloadPersona")}</button><button type="button" onClick={onEditPersona}>{t("settings.companion.editPersona")}</button></div>
      <SelectInput label={t("settings.companion.assertiveness")} path="companion.assertiveness" value={form.assertiveness} options={["low", "normal", "high"]} update={(value) => update("assertiveness", value as FormState["assertiveness"])} />
      <p className="field-help">{t("settings.companion.assertivenessHelp")}</p>
      <SettingsSearchItem label={t("settings.companion.emotionsEnabled")} path="companion.emotionsEnabled" description={t("settings.companion.emotionsDescription")}>
        <label><span>{t("settings.companion.emotionsEnabled")}</span><input id="setting-companion-emotionsEnabled" type="checkbox" checked={form.emotionsEnabled} disabled={saving} onChange={(event) => update("emotionsEnabled", event.target.checked)} />{errorFor("companion.emotionsEnabled") === undefined ? null : <span className="field-error">{errorFor("companion.emotionsEnabled")}</span>}</label>
        <p className="field-help">{t("settings.companion.emotionsHelp")}</p>
      </SettingsSearchItem>
    </fieldset>

    <fieldset id="settings-appearance"><legend>{t("settings.appearance.heading")}</legend>
      <SettingsSearchItem label={t("settings.appearance.theme")} path="ui.theme">
        <label><span>{t("settings.appearance.theme")}</span><select id="setting-ui-theme" value={form.uiTheme} onChange={(event) => update("uiTheme", event.target.value as FormState["uiTheme"])}><option value="system">{t("settings.appearance.system")}</option><option value="light">{t("settings.appearance.light")}</option><option value="dark">{t("settings.appearance.dark")}</option></select></label>
      </SettingsSearchItem>
      <SettingsSearchItem label={t("settings.appearance.font")} path="ui.font">
        <label><span>{t("settings.appearance.font")}</span><select id="setting-ui-font" value={fontPreset(form.uiFont)} onChange={(event) => { if (event.target.value !== "custom") update("uiFont", event.target.value); }}><option value="system">{t("settings.appearance.systemFont")}</option><option value="rounded">{t("settings.appearance.roundedFont")}</option><option value="serif">{t("settings.appearance.serifFont")}</option><option value="mono">{t("settings.appearance.monoFont")}</option><option value="custom">{t("settings.appearance.installedFont")}</option></select></label>
      </SettingsSearchItem>
      <SettingsSearchItem label={t("settings.appearance.freeFont")} path="ui.font" description={t("settings.appearance.freeFontDescription")}>
        <label><span>{t("settings.appearance.freeFont")}</span><input value={fontPreset(form.uiFont) === "custom" ? form.uiFont : ""} placeholder={t("settings.appearance.freeFontPlaceholder")} onChange={(event) => update("uiFont", event.target.value)} /></label>
      </SettingsSearchItem>
      <SettingsSearchItem label={t("language.label")} path="ui.language">
        <label><span>{t("language.label")}</span><select id="setting-ui-language" value={form.language} onChange={(event) => update("language", event.target.value as FormState["language"])}><option value="ja">{t("language.japanese")}</option><option value="en">{t("language.english")}</option></select></label>
      </SettingsSearchItem>
    </fieldset>

    <fieldset id="settings-memory"><legend>{t("settings.memorySettings.heading")}</legend>
      <BooleanInput label={t("settings.memorySettings.enabled")} path="memory.enabled" value={form.memoryEnabled} update={(value) => update("memoryEnabled", value)} />
      <BooleanInput label={t("settings.memorySettings.providerConsent")} path="memory.providerConsent" value={form.memoryProviderConsent} update={(value) => update("memoryProviderConsent", value)} />
      <p className="field-help">{t("settings.memorySettings.help")}</p>
    </fieldset>
    <MemoryPanel status={snapshot.memoryStatus} />

    <fieldset id="settings-app"><legend>{t("settings.appSettings.heading")}</legend><BooleanInput label={t("settings.appSettings.checkForUpdates")} path="app.checkForUpdates" value={form.checkForUpdates} update={(value) => update("checkForUpdates", value)} /><p className="field-help">{t("settings.appSettings.checkForUpdatesHelp")}</p><BooleanInput label={t("settings.appSettings.launchAtLogin")} path="app.launchAtLogin" value={form.launchAtLogin} update={(value) => update("launchAtLogin", value)} /></fieldset>

    <AppUpdatePanel enabled={snapshot.config.app.checkForUpdates} showCheck />

    <fieldset id="settings-general-detail"><legend>{t("settings.detail.heading")}</legend>
      <h3>{t("settings.detail.personality")}</h3>
      <SettingsSearchItem label={t("settings.detail.reviewTime")} path="companion.reviewTime" description={t("settings.detail.reviewTimeHelp")}>
        <label><span>{t("settings.detail.reviewTime")}</span><input id="setting-companion-reviewTime" type="time" value={form.reviewTime} onChange={(event) => update("reviewTime", event.target.value)} /><small>{t("settings.detail.reviewTimeHelp")}</small></label>
      </SettingsSearchItem>
      <ReminderEditor value={form.reminders} update={(value) => update("reminders", value)} />
      <SettingsSearchItem label={t("settings.detail.whileThinking")} path="chat.whileThinking" description={t("settings.detail.whileThinkingHelp")}>
        <label><span>{t("settings.detail.whileThinking")}</span><select id="setting-chat-whileThinking" value={form.whileThinking} onChange={(event) => update("whileThinking", event.target.value as FormState["whileThinking"])}><option value="queue">{t("settings.detail.queue")}</option><option value="append">{t("settings.detail.append")}</option></select><small>{t("settings.detail.whileThinkingHelp")}</small></label>
      </SettingsSearchItem>
      <h3>{t("settings.detail.language")}</h3>
      <TextInput label={t("settings.detail.recognitionLocale")} path="speech.locale" value={form.speechLocale} update={(value) => update("speechLocale", value)} />
      <h3>{t("settings.detail.memory")}</h3>
      <NumberInput label={t("settings.detail.graceMinutes")} path="memory.graceMinutes" value={form.memoryGraceMinutes} update={(value) => update("memoryGraceMinutes", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.detail.dailyRetentionDays")} path="memory.dailyRetentionDays" value={form.memoryDailyRetentionDays} update={(value) => update("memoryDailyRetentionDays", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.detail.weeklyRetentionWeeks")} path="memory.weeklyRetentionWeeks" value={form.memoryWeeklyRetentionWeeks} update={(value) => update("memoryWeeklyRetentionWeeks", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.detail.contextRefreshCalls")} path="companion.contextRefreshCalls" value={form.contextRefreshCalls} update={(value) => update("contextRefreshCalls", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.detail.factPromptDailyLimit")} path="memory.factPromptDailyLimit" value={form.factPromptDailyLimit} update={(value) => update("factPromptDailyLimit", value)} errorFor={errorFor} />
      <fieldset id="settings-debug"><legend>{t("settings.detail.debug")}</legend><BooleanInput label={t("settings.detail.debugEnabled")} path="debug.enabled" value={form.debugEnabled} update={(value) => update("debugEnabled", value)} /><p className="field-help">{t("settings.detail.debugHelp")}</p></fieldset>
    </fieldset>
  </>;
}
