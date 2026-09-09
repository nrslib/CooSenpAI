import { useEffect, useState, type ReactElement } from "react";

import { useI18n } from "../i18n/index.js";
import type { IpcResult, ProviderApiKeyStatus, ProviderName } from "../types.js";
import { SettingsSearchItem } from "../settings-search.js";
import { ConfirmationDialog } from "./ConfirmationDialog.js";

interface Props {
  readonly status?: ProviderApiKeyStatus;
  readonly error?: string;
  readonly disabled: boolean;
  readonly drafts: ProviderApiKeyDrafts;
  readonly onDraftChange: (provider: ProviderName, value: string) => void;
  readonly onSave: (provider: ProviderName, apiKey: string) => Promise<IpcResult<ProviderApiKeyStatus>>;
  readonly onDelete: (provider: ProviderName) => Promise<IpcResult<ProviderApiKeyStatus>>;
}

export type ProviderApiKeyDrafts = Record<ProviderName, string>;

export function ProviderApiKeyFields({ status, error, disabled, drafts, onDraftChange, onSave, onDelete }: Props): ReactElement {
  const { t } = useI18n();
  return <div id="settings-provider-api-keys" className="provider-api-key-section">
    <h3>{t("settings.apiKeys.heading")}</h3>
    <p className="field-help">{t("settings.apiKeys.help")}</p>
    {error === undefined ? null : <p className="field-error" role="alert">{error}</p>}
    {status === undefined ? <p className="field-help">{t("settings.apiKeys.checking")}</p> : <>
      <ProviderApiKeyField provider="claude" label="Claude Code" environment="ANTHROPIC_API_KEY" configured={status.claude} disabled={disabled} draft={drafts.claude} onDraftChange={(value) => onDraftChange("claude", value)} onSave={onSave} onDelete={onDelete} />
      <ProviderApiKeyField provider="codex" label="Codex" environment="OPENAI_API_KEY" configured={status.codex} disabled={disabled} draft={drafts.codex} onDraftChange={(value) => onDraftChange("codex", value)} onSave={onSave} onDelete={onDelete} />
      <ProviderApiKeyField provider="opencode" label="OpenCode" environment="OPENCODE_API_KEY / OPENCODE_ZEN_API_KEY" configured={status.opencode} disabled={disabled} draft={drafts.opencode} onDraftChange={(value) => onDraftChange("opencode", value)} onSave={onSave} onDelete={onDelete} />
    </>}
  </div>;
}

function ProviderApiKeyField({ provider, label, environment, configured, disabled, draft, onDraftChange, onSave, onDelete }: { readonly provider: ProviderName; readonly label: string; readonly environment: string; readonly configured: boolean; readonly disabled: boolean; readonly draft: string; readonly onDraftChange: (value: string) => void; readonly onSave: (provider: ProviderName, apiKey: string) => Promise<IpcResult<ProviderApiKeyStatus>>; readonly onDelete: (provider: ProviderName) => Promise<IpcResult<ProviderApiKeyStatus>> }): ReactElement {
  const { t } = useI18n();
  const [editing, setEditing] = useState(!configured || draft !== "");
  const [busy, setBusy] = useState(false);
  const [fieldError, setFieldError] = useState<string>();
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  useEffect(() => {
    if (!configured || draft === "") setEditing(!configured);
    setFieldError(undefined);
  }, [configured]);
  const save = async (): Promise<void> => {
    if (draft.trim() === "") {
      setFieldError(t("bubble.apiKeyRequired"));
      return;
    }
    setBusy(true);
    setFieldError(undefined);
    try {
      const result = await onSave(provider, draft);
      if (result.ok) {
        onDraftChange("");
        setEditing(false);
      } else {
        setFieldError(result.error.message);
      }
    } finally {
      setBusy(false);
    }
  };
  const remove = (): void => {
    setDeleteConfirmOpen(true);
  };
  const removeConfirmed = async (): Promise<void> => {
    setDeleteConfirmOpen(false);
    setBusy(true);
    setFieldError(undefined);
    try {
      const result = await onDelete(provider);
      if (result.ok) {
        onDraftChange("");
        setEditing(true);
      } else {
        setFieldError(result.error.message);
      }
    } finally {
      setBusy(false);
    }
  };
  const inputValue = editing ? draft : "••••••••";
  const inputId = `setting-provider-api-key-${provider}`;
  return <SettingsSearchItem label={t("settings.apiKeys.searchLabel", { label })} path={`provider.apiKey.${provider}`} description={t("settings.apiKeys.description", { environment })}>
    <div className="provider-api-key-field">
      <label htmlFor={inputId}><span>{label}</span><small>{environment}</small></label>
      <input id={inputId} type="password" autoComplete="new-password" spellCheck={false} value={inputValue} readOnly={!editing} disabled={disabled || busy} placeholder={editing ? t("settings.apiKeys.unset") : undefined} onFocus={() => { if (!editing) { onDraftChange(""); setEditing(true); } }} onChange={(event) => onDraftChange(event.target.value)} />
      <span className="provider-api-key-actions">
        {configured && !editing ? <small>{t("settings.apiKeys.savedMasked")}</small> : null}
        <button type="button" disabled={disabled || busy} onClick={() => { if (editing) void save(); else setEditing(true); }}>{editing ? t("settings.apiKeys.save") : t("settings.apiKeys.change")}</button>
        {configured ? <button type="button" disabled={disabled || busy} onClick={remove}>{t("settings.apiKeys.delete")}</button> : null}
      </span>
      {fieldError === undefined ? null : <span className="field-error" role="alert">{fieldError}</span>}
      {deleteConfirmOpen ? <ConfirmationDialog id={`provider-api-key-delete-${provider}`} title={t("settings.apiKeys.deleteTitle")} description={t("settings.apiKeys.deleteDescription", { label })} cancelLabel={t("common.cancel")} confirmLabel={t("settings.apiKeys.delete")} onCancel={() => setDeleteConfirmOpen(false)} onConfirm={() => { void removeConfirmed(); }} /> : null}
    </div>
  </SettingsSearchItem>;
}
