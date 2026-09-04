import type { ReactElement } from "react";

import type { IpcResult, ProviderApiKeyStatus, ProviderModelOptions, ProviderName } from "../types.js";
import { tutorialSettingsHighlight } from "../tutorial-ui.js";
import type { SettingsCategoryProps } from "./SettingsCategoryProps.js";
import { ProviderApiKeyFields } from "./ProviderApiKeyFields.js";
import { ModelInput, NumberInput, ProviderInput, TextInput } from "./SettingsControls.js";

interface Props extends SettingsCategoryProps {
  readonly providerModels: readonly ProviderModelOptions[];
  readonly providerApiKeys?: ProviderApiKeyStatus;
  readonly providerApiKeysError?: string;
  readonly onChangeProvider: (target: "observer" | "companion", provider: ProviderName) => void;
  readonly onSaveProviderApiKey: (provider: ProviderName, apiKey: string) => Promise<IpcResult<ProviderApiKeyStatus>>;
  readonly onDeleteProviderApiKey: (provider: ProviderName) => Promise<IpcResult<ProviderApiKeyStatus>>;
}

export function ProviderSettings({ form, snapshot, advanced, saving, update, errorFor, providerModels, providerApiKeys, providerApiKeysError, onChangeProvider, onSaveProviderApiKey, onDeleteProviderApiKey }: Props): ReactElement {
  return <fieldset id="settings-ai" className={tutorialSettingsHighlight(snapshot.onboarding, "provider") ? "tutorial-highlight" : undefined}>
    <legend>プロバイダとモデル</legend>
    <h3>Vision AI</h3>
    <ProviderInput path="observer.provider" value={form.providerObserver} update={(value) => onChangeProvider("observer", value)} />
    <ModelInput path="observer.model" value={form.observerModel} options={providerModels.find((option) => option.provider === form.providerObserver)?.candidates ?? []} update={(value) => update("observerModel", value)} />
    <TextInput label="推論強度" path="observer.effort" value={form.observerEffort} update={(value) => update("observerEffort", value)} />
    {advanced ? <>
      <TextInput label="実行ファイル（任意）" path="observer.executable" value={form.observerExecutable} update={(value) => update("observerExecutable", value)} />
      <NumberInput label="タイムアウト" path="observer.timeoutMs" value={form.observerTimeoutMs} update={(value) => update("observerTimeoutMs", value)} errorFor={errorFor} />
      <NumberInput label="1 日の呼び出し上限" path="observer.dailyCallLimit" value={form.observerLimit} update={(value) => update("observerLimit", value)} errorFor={errorFor} />
    </> : null}
    <h3>{snapshot.companionDisplayName}</h3>
    <ProviderInput path="companion.provider" value={form.providerCompanion} update={(value) => onChangeProvider("companion", value)} />
    <ModelInput path="companion.model" value={form.companionModel} options={providerModels.find((option) => option.provider === form.providerCompanion)?.candidates ?? []} update={(value) => update("companionModel", value)} />
    <TextInput label="推論強度" path="companion.effort" value={form.companionEffort} update={(value) => update("companionEffort", value)} />
    {advanced ? <>
      <TextInput label="実行ファイル（任意）" path="companion.executable" value={form.companionExecutable} update={(value) => update("companionExecutable", value)} />
      <NumberInput label="タイムアウト" path="companion.timeoutMs" value={form.companionTimeoutMs} update={(value) => update("companionTimeoutMs", value)} errorFor={errorFor} />
      <NumberInput label="自発呼び出し上限" path="companion.dailyProactiveLimit" value={form.companionLimit} update={(value) => update("companionLimit", value)} errorFor={errorFor} />
    </> : null}
    {advanced ? <ProviderApiKeyFields status={providerApiKeys} error={providerApiKeysError} disabled={saving} onSave={onSaveProviderApiKey} onDelete={onDeleteProviderApiKey} /> : null}
  </fieldset>;
}
