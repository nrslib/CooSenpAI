import { useEffect, useState, type ChangeEvent, type KeyboardEvent, type ReactElement } from "react";

import { companionEffortPatch, companionProviderModelPatch, defaultModelForProvider, effortCandidates, modelCandidates, PROVIDER_OPTIONS } from "../model-catalog.js";
import type { AppSnapshot, CompanionModelCatalog, ConfigPatch, CooSenpaiConfig, IpcResult, ProviderName } from "../types.js";

interface CompanionModelControlsProps {
  readonly snapshot: AppSnapshot;
  readonly catalog?: CompanionModelCatalog;
  readonly disabled: boolean;
  readonly onSave: (patch: ConfigPatch) => Promise<IpcResult<CooSenpaiConfig>>;
  readonly onReloadOpencode: () => Promise<void>;
}

export function CompanionModelControls({ snapshot, catalog, disabled, onSave, onReloadOpencode }: CompanionModelControlsProps): ReactElement {
  const companion = snapshot.config.companion;
  const [provider, setProvider] = useState<ProviderName>(companion.provider);
  const [model, setModel] = useState(companion.model);
  const [effort, setEffort] = useState(companion.effort);
  const [saving, setSaving] = useState(false);
  const [reloading, setReloading] = useState(false);
  const [notice, setNotice] = useState<string>();

  useEffect(() => {
    setProvider(companion.provider);
    setModel(companion.model);
    setEffort(companion.effort);
  }, [companion.effort, companion.provider, companion.model]);

  const save = async (patch: ConfigPatch): Promise<IpcResult<CooSenpaiConfig>> => {
    setSaving(true);
    setNotice(undefined);
    try {
      return await onSave(patch);
    } finally {
      setSaving(false);
    }
  };

  const changeProvider = async (event: ChangeEvent<HTMLSelectElement>): Promise<void> => {
    const nextProvider = event.currentTarget.value as ProviderName;
    const nextModel = defaultModelForProvider(catalog, nextProvider);
    setProvider(nextProvider);
    setModel(nextModel);
    if (nextModel === "") {
      setNotice("opencode はモデル名を入力すると切り替わります。自由入力できます。");
      return;
    }
    const result = await save(companionProviderModelPatch(nextProvider, nextModel));
    if (!result.ok) {
      setProvider(companion.provider);
      setModel(companion.model);
    }
  };

  const commitModel = async (): Promise<void> => {
    const nextModel = model;
    if (nextModel === "") {
      setModel(companion.model);
      return;
    }
    if (provider === companion.provider && nextModel === companion.model) return;
    const result = await save(companionProviderModelPatch(provider, nextModel));
    if (!result.ok) {
      setProvider(companion.provider);
      setModel(companion.model);
    }
  };

  const commitEffort = async (): Promise<void> => {
    if (effort === companion.effort) return;
    const result = await save(companionEffortPatch(effort));
    if (!result.ok) setEffort(companion.effort);
  };

  const handleModelKeyDown = (event: KeyboardEvent<HTMLInputElement>): void => {
    if (event.key === "Enter") {
      event.preventDefault();
      void commitModel();
    }
  };

  const handleEffortKeyDown = (event: KeyboardEvent<HTMLInputElement>): void => {
    if (event.key === "Enter") {
      event.preventDefault();
      void commitEffort();
    }
  };

  const candidates = modelCandidates(catalog, provider, model);
  const efforts = effortCandidates(catalog, provider, model);
  const datalistId = "companion-model-candidates";

  return <section className="companion-model-controls" aria-label="Coo のモデル設定">
    <div className="companion-model-control">
      <label htmlFor="companion-provider">Provider</label>
      <select id="companion-provider" value={provider} onChange={(event) => { void changeProvider(event); }} disabled={disabled || saving}>
        {PROVIDER_OPTIONS.map((candidate) => <option key={candidate} value={candidate}>{candidate}</option>)}
      </select>
    </div>
    <div className="companion-model-control companion-model-control-model">
      <label htmlFor="companion-model">Model</label>
      <input
        id="companion-model"
        value={model}
        list={datalistId}
        onChange={(event) => setModel(event.currentTarget.value)}
        onBlur={() => { void commitModel(); }}
        onKeyDown={handleModelKeyDown}
        disabled={disabled || saving}
        autoComplete="off"
      />
      <datalist id={datalistId}>
        {candidates.map((candidate) => <option key={candidate} value={candidate} />)}
      </datalist>
    </div>
    <div className="companion-model-control companion-model-control-effort">
      <label htmlFor="companion-effort">Effort</label>
      <input
        id="companion-effort"
        value={effort}
        list="companion-effort-candidates"
        onChange={(event) => setEffort(event.currentTarget.value)}
        onBlur={() => { void commitEffort(); }}
        onKeyDown={handleEffortKeyDown}
        disabled={disabled || saving}
        autoComplete="off"
      />
      <datalist id="companion-effort-candidates">
        {efforts.map((candidate) => <option key={candidate} value={candidate} />)}
      </datalist>
    </div>
    {provider === "claude" ? <small className="companion-model-help">別名は常に最新モデルを指します。</small> : null}
    {provider === "opencode" ? <>
      <button className="companion-model-reload" type="button" onClick={() => {
        setReloading(true);
        void onReloadOpencode().finally(() => setReloading(false));
      }} disabled={disabled || reloading || saving}>
        {reloading ? "読込中…" : "再読込"}
      </button>
      {catalog?.opencodeError !== undefined ? <small className="companion-model-help" role="status">一覧を取得できませんでした。現在値・履歴と自由入力を使えます。</small> : null}
    </> : null}
    {notice !== undefined ? <small className="companion-model-help" role="status">{notice}</small> : null}
  </section>;
}
