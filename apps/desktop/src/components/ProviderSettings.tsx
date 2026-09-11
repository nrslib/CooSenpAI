import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import { tuningIsDefault } from "../settings-form.js";
import type { IpcResult, ProviderApiKeyStatus, ProviderModelOptions, ProviderName } from "../types.js";
import { tutorialSettingsHighlight } from "../tutorial-ui.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { ProviderApiKeyFields, type ProviderApiKeyDrafts } from "./ProviderApiKeyFields.js";
import { ModelInput, NumberInput, ProviderInput, TextInput } from "./SettingsControls.js";

interface Props extends SettingsCategoryProps {
  readonly onResetTuning: () => void;
  readonly providerModels: readonly ProviderModelOptions[];
  readonly providerApiKeys?: ProviderApiKeyStatus;
  readonly providerApiKeysError?: string;
  readonly providerApiKeyDrafts: ProviderApiKeyDrafts;
  readonly onChangeProvider: (target: "vision" | "hearing" | "companion", provider: ProviderName) => void;
  readonly onProviderApiKeyDraftChange: (provider: ProviderName, value: string) => void;
  readonly onSaveProviderApiKey: (provider: ProviderName, apiKey: string) => Promise<IpcResult<ProviderApiKeyStatus>>;
  readonly onDeleteProviderApiKey: (provider: ProviderName) => Promise<IpcResult<ProviderApiKeyStatus>>;
}

export function ProviderSettings({ form, snapshot, saving, update, errorFor, providerModels, providerApiKeys, providerApiKeysError, providerApiKeyDrafts, onChangeProvider, onProviderApiKeyDraftChange, onSaveProviderApiKey, onDeleteProviderApiKey, onResetTuning }: Props): ReactElement {
  const { t } = useI18n();
  return <>
    <fieldset id="settings-ai" className={tutorialSettingsHighlight(snapshot.onboarding, "provider") ? "tutorial-highlight" : undefined}>
      <legend>{t("settings.providers.heading")}</legend>
      <h3>{t("settings.providers.vision")}</h3>
      <ProviderInput path="observer.vision.provider" value={form.providerObserver} update={(value) => onChangeProvider("vision", value)} />
      <ModelInput path="observer.vision.model" value={form.observerModel} options={providerModels.find((option) => option.provider === form.providerObserver)?.candidates ?? []} update={(value) => update("observerModel", value)} />
      <TextInput label={t("settings.providers.effort")} path="observer.vision.effort" value={form.observerEffort} update={(value) => update("observerEffort", value)} />
      <h3>{t("settings.providers.hearing")}</h3>
      <ProviderInput path="observer.hearing.provider" value={form.providerHearing} update={(value) => onChangeProvider("hearing", value)} />
      <ModelInput path="observer.hearing.model" value={form.hearingModel} options={providerModels.find((option) => option.provider === form.providerHearing)?.candidates ?? []} update={(value) => update("hearingModel", value)} />
      <TextInput label={t("settings.providers.effort")} path="observer.hearing.effort" value={form.hearingEffort} update={(value) => update("hearingEffort", value)} />
      <h3>{snapshot.companionDisplayName}</h3>
      <ProviderInput path="companion.provider" value={form.providerCompanion} update={(value) => onChangeProvider("companion", value)} />
      <ModelInput path="companion.model" value={form.companionModel} options={providerModels.find((option) => option.provider === form.providerCompanion)?.candidates ?? []} update={(value) => update("companionModel", value)} />
      <TextInput label={t("settings.providers.effort")} path="companion.effort" value={form.companionEffort} update={(value) => update("companionEffort", value)} />
    </fieldset>
    <fieldset id="settings-provider-detail"><legend>{t("settings.providers.detail")}</legend>
      <h3>{t("settings.providers.vision")}</h3>
      <TextInput label={t("settings.providers.executable")} path="observer.vision.executable" value={form.observerExecutable} update={(value) => update("observerExecutable", value)} />
      <NumberInput label={t("settings.providers.interval")} path="observer.vision.intervalMs" value={form.observerIntervalMs} update={(value) => update("observerIntervalMs", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.providers.timeout")} path="observer.vision.timeoutMs" value={form.observerTimeoutMs} update={(value) => update("observerTimeoutMs", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.providers.observerLimit")} path="observer.vision.dailyCallLimit" value={form.observerLimit} update={(value) => update("observerLimit", value)} errorFor={errorFor} />
      <h3>{t("settings.providers.hearing")}</h3>
      <TextInput label={t("settings.providers.executable")} path="observer.hearing.executable" value={form.hearingExecutable} update={(value) => update("hearingExecutable", value)} />
      <NumberInput label={t("settings.providers.interval")} path="observer.hearing.intervalMs" value={form.hearingIntervalMs} update={(value) => update("hearingIntervalMs", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.providers.timeout")} path="observer.hearing.timeoutMs" value={form.hearingTimeoutMs} update={(value) => update("hearingTimeoutMs", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.providers.observerLimit")} path="observer.hearing.dailyCallLimit" value={form.hearingLimit} update={(value) => update("hearingLimit", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.vision.textExcerptMaxChars")} path="observer.hearing.textExcerptMaxChars" value={form.hearingTextExcerptMaxChars} update={(value) => update("hearingTextExcerptMaxChars", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.vision.textExcerptMaxCount")} path="observer.hearing.textExcerptMaxCount" value={form.hearingTextExcerptMaxCount} update={(value) => update("hearingTextExcerptMaxCount", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.vision.textTotalMaxChars")} path="observer.hearing.textTotalMaxChars" value={form.hearingTextTotalMaxChars} update={(value) => update("hearingTextTotalMaxChars", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.vision.changesMaxCount")} path="observer.hearing.changesMaxCount" value={form.hearingChangesMaxCount} update={(value) => update("hearingChangesMaxCount", value)} errorFor={errorFor} />
      {tuningIsDefault(form) ? null : <button type="button" disabled={saving} onClick={onResetTuning}>{t("settings.resetTuning")}</button>}
      <h3>{snapshot.companionDisplayName}</h3>
      <TextInput label={t("settings.providers.executable")} path="companion.executable" value={form.companionExecutable} update={(value) => update("companionExecutable", value)} />
      <NumberInput label={t("settings.providers.timeout")} path="companion.timeoutMs" value={form.companionTimeoutMs} update={(value) => update("companionTimeoutMs", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.providers.companionLimit")} path="companion.dailyProactiveLimit" value={form.companionLimit} update={(value) => update("companionLimit", value)} errorFor={errorFor} />
      <ProviderApiKeyFields status={providerApiKeys} error={providerApiKeysError} disabled={saving} drafts={providerApiKeyDrafts} onDraftChange={onProviderApiKeyDraftChange} onSave={onSaveProviderApiKey} onDelete={onDeleteProviderApiKey} />
    </fieldset>
  </>;
}
