import type { ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import { effortCandidatesForModel } from "../settings-model.js";
import type { IpcResult, ProviderApiKeyStatus, ProviderModelOptions, ProviderName } from "../types.js";
import { tutorialSettingsHighlight } from "../tutorial-ui.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { ProviderApiKeyFields, type ProviderApiKeyDrafts } from "./ProviderApiKeyFields.js";
import { EffortInput, ModelInput, NumberInput, ProviderInput } from "./SettingsControls.js";

interface Props extends SettingsCategoryProps {
  readonly providerModels: readonly ProviderModelOptions[];
  readonly providerApiKeys?: ProviderApiKeyStatus;
  readonly providerApiKeysError?: string;
  readonly providerApiKeyDrafts: ProviderApiKeyDrafts;
  readonly onChangeProvider: (target: "vision" | "hearing" | "companion", provider: ProviderName) => void;
  readonly onProviderApiKeyDraftChange: (provider: ProviderName, value: string) => void;
  readonly onSaveProviderApiKey: (provider: ProviderName, apiKey: string) => Promise<IpcResult<ProviderApiKeyStatus>>;
  readonly onDeleteProviderApiKey: (provider: ProviderName) => Promise<IpcResult<ProviderApiKeyStatus>>;
}

export function ProviderSettings({ form, snapshot, saving, update, errorFor, providerModels, providerApiKeys, providerApiKeysError, providerApiKeyDrafts, onChangeProvider, onProviderApiKeyDraftChange, onSaveProviderApiKey, onDeleteProviderApiKey }: Props): ReactElement {
  const { t } = useI18n();
  const visionModels = providerModels.find((option) => option.provider === form.providerObserver);
  const hearingModels = providerModels.find((option) => option.provider === form.providerHearing);
  const companionModels = providerModels.find((option) => option.provider === form.providerCompanion);
  return <>
    <fieldset id="settings-ai" className={tutorialSettingsHighlight(snapshot.onboarding, "provider") ? "tutorial-highlight" : undefined}>
      <legend>{t("settings.providers.heading")}</legend>
      <h3>{snapshot.companionDisplayName}</h3>
      <ProviderInput path="companion.provider" value={form.providerCompanion} update={(value) => onChangeProvider("companion", value)} />
      <ModelInput path="companion.model" value={form.companionModel} options={companionModels?.candidates ?? []} label={t("settings.providers.replyModel")} description={t("settings.providers.replyDescription")} update={(value) => update("companionModel", value)} />
      <EffortInput path="companion.effort" value={form.companionEffort} options={companionModels === undefined ? [] : effortCandidatesForModel(companionModels, form.companionModel)} label={t("settings.providers.replyEffort")} description={t("settings.providers.replyDescription")} update={(value) => update("companionEffort", value)} />
      <ModelInput path="companion.proactiveModel" value={form.proactiveModel} options={companionModels?.candidates ?? []} label={t("settings.providers.proactiveModel")} description={t("settings.providers.proactiveDescription")} placeholder={t("settings.providers.sameAsReply")} update={(value) => update("proactiveModel", value)} />
      <EffortInput path="companion.proactiveEffort" value={form.proactiveEffort} options={companionModels === undefined ? [] : effortCandidatesForModel(companionModels, form.proactiveModel === "" ? form.companionModel : form.proactiveModel)} label={t("settings.providers.proactiveEffort")} description={t("settings.providers.proactiveDescription")} placeholder={t("settings.providers.sameAsReply")} update={(value) => update("proactiveEffort", value)} />
      <NumberInput label={t("settings.providers.companionLimit")} path="companion.dailyProactiveLimit" value={form.companionLimit} update={(value) => update("companionLimit", value)} errorFor={errorFor} />
      <h3>{t("settings.providers.vision")}</h3>
      <ProviderInput path="observer.vision.provider" value={form.providerObserver} update={(value) => onChangeProvider("vision", value)} />
      <ModelInput path="observer.vision.model" value={form.observerModel} options={visionModels?.candidates ?? []} update={(value) => update("observerModel", value)} />
      <EffortInput path="observer.vision.effort" value={form.observerEffort} options={visionModels === undefined ? [] : effortCandidatesForModel(visionModels, form.observerModel)} update={(value) => update("observerEffort", value)} />
      <NumberInput label={t("settings.providers.observerLimit")} path="observer.vision.dailyCallLimit" value={form.observerLimit} update={(value) => update("observerLimit", value)} errorFor={errorFor} />
      <h3>{t("settings.providers.hearing")}</h3>
      <ProviderInput path="observer.hearing.provider" value={form.providerHearing} update={(value) => onChangeProvider("hearing", value)} />
      <ModelInput path="observer.hearing.model" value={form.hearingModel} options={hearingModels?.candidates ?? []} update={(value) => update("hearingModel", value)} />
      <EffortInput path="observer.hearing.effort" value={form.hearingEffort} options={hearingModels === undefined ? [] : effortCandidatesForModel(hearingModels, form.hearingModel)} update={(value) => update("hearingEffort", value)} />
      <NumberInput label={t("settings.providers.observerLimit")} path="observer.hearing.dailyCallLimit" value={form.hearingLimit} update={(value) => update("hearingLimit", value)} errorFor={errorFor} />
    </fieldset>
    <fieldset id="settings-provider-keys">
      <ProviderApiKeyFields status={providerApiKeys} error={providerApiKeysError} disabled={saving} drafts={providerApiKeyDrafts} onDraftChange={onProviderApiKeyDraftChange} onSave={onSaveProviderApiKey} onDelete={onDeleteProviderApiKey} />
    </fieldset>
  </>;
}
