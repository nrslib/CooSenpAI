import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import type { IpcResult, ProviderApiKeyStatus, ProviderModelOptions, ProviderName } from "../types.js";
import { tutorialSettingsHighlight } from "../tutorial-ui.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { ProviderApiKeyFields, type ProviderApiKeyDrafts } from "./ProviderApiKeyFields.js";
import { ModelInput, NumberInput, ProviderInput, TextInput } from "./SettingsControls.js";

interface Props extends SettingsCategoryProps {
  readonly providerModels: readonly ProviderModelOptions[];
  readonly providerApiKeys?: ProviderApiKeyStatus;
  readonly providerApiKeysError?: string;
  readonly providerApiKeyDrafts: ProviderApiKeyDrafts;
  readonly onChangeProvider: (target: "observer" | "companion", provider: ProviderName) => void;
  readonly onProviderApiKeyDraftChange: (provider: ProviderName, value: string) => void;
  readonly onSaveProviderApiKey: (provider: ProviderName, apiKey: string) => Promise<IpcResult<ProviderApiKeyStatus>>;
  readonly onDeleteProviderApiKey: (provider: ProviderName) => Promise<IpcResult<ProviderApiKeyStatus>>;
}

export function ProviderSettings({ form, snapshot, saving, update, errorFor, providerModels, providerApiKeys, providerApiKeysError, providerApiKeyDrafts, onChangeProvider, onProviderApiKeyDraftChange, onSaveProviderApiKey, onDeleteProviderApiKey }: Props): ReactElement {
  const { t } = useI18n();
  return <>
    <fieldset id="settings-ai" className={tutorialSettingsHighlight(snapshot.onboarding, "provider") ? "tutorial-highlight" : undefined}>
      <legend>{t("settings.providers.heading")}</legend>
      <h3>{t("settings.providers.vision")}</h3>
      <ProviderInput path="observer.provider" value={form.providerObserver} update={(value) => onChangeProvider("observer", value)} />
      <ModelInput path="observer.model" value={form.observerModel} options={providerModels.find((option) => option.provider === form.providerObserver)?.candidates ?? []} update={(value) => update("observerModel", value)} />
      <TextInput label={t("settings.providers.effort")} path="observer.effort" value={form.observerEffort} update={(value) => update("observerEffort", value)} />
      <h3>{snapshot.companionDisplayName}</h3>
      <ProviderInput path="companion.provider" value={form.providerCompanion} update={(value) => onChangeProvider("companion", value)} />
      <ModelInput path="companion.model" value={form.companionModel} options={providerModels.find((option) => option.provider === form.providerCompanion)?.candidates ?? []} update={(value) => update("companionModel", value)} />
      <TextInput label={t("settings.providers.effort")} path="companion.effort" value={form.companionEffort} update={(value) => update("companionEffort", value)} />
    </fieldset>
    <fieldset id="settings-provider-detail"><legend>{t("settings.providers.detail")}</legend>
      <h3>{t("settings.providers.vision")}</h3>
      <TextInput label={t("settings.providers.executable")} path="observer.executable" value={form.observerExecutable} update={(value) => update("observerExecutable", value)} />
      <NumberInput label={t("settings.providers.timeout")} path="observer.timeoutMs" value={form.observerTimeoutMs} update={(value) => update("observerTimeoutMs", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.providers.observerLimit")} path="observer.dailyCallLimit" value={form.observerLimit} update={(value) => update("observerLimit", value)} errorFor={errorFor} />
      <h3>{snapshot.companionDisplayName}</h3>
      <TextInput label={t("settings.providers.executable")} path="companion.executable" value={form.companionExecutable} update={(value) => update("companionExecutable", value)} />
      <NumberInput label={t("settings.providers.timeout")} path="companion.timeoutMs" value={form.companionTimeoutMs} update={(value) => update("companionTimeoutMs", value)} errorFor={errorFor} />
      <NumberInput label={t("settings.providers.companionLimit")} path="companion.dailyProactiveLimit" value={form.companionLimit} update={(value) => update("companionLimit", value)} errorFor={errorFor} />
      <ProviderApiKeyFields status={providerApiKeys} error={providerApiKeysError} disabled={saving} drafts={providerApiKeyDrafts} onDraftChange={onProviderApiKeyDraftChange} onSave={onSaveProviderApiKey} onDelete={onDeleteProviderApiKey} />
    </fieldset>
  </>;
}
