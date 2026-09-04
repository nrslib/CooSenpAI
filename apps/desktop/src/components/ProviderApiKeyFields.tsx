import { useEffect, useState, type ReactElement } from "react";

import type { IpcResult, ProviderApiKeyStatus, ProviderName } from "../types.js";
import { ConfirmationDialog } from "./ConfirmationDialog.js";

interface Props {
  readonly status?: ProviderApiKeyStatus;
  readonly error?: string;
  readonly disabled: boolean;
  readonly onSave: (provider: ProviderName, apiKey: string) => Promise<IpcResult<ProviderApiKeyStatus>>;
  readonly onDelete: (provider: ProviderName) => Promise<IpcResult<ProviderApiKeyStatus>>;
}

export function ProviderApiKeyFields({ status, error, disabled, onSave, onDelete }: Props): ReactElement {
  return <div id="settings-provider-api-keys" className="provider-api-key-section">
    <h3>API キー（任意）</h3>
    <p className="field-help">必要なプロバイダだけ入力してください。未設定なら CLI の既存ログインを使います。キーは macOS のキーチェーンに保存し、設定ファイルには保存しません。</p>
    {error === undefined ? null : <p className="field-error" role="alert">{error}</p>}
    {status === undefined ? <p className="field-help">保存状態を確認しています…</p> : <>
      <ProviderApiKeyField provider="claude" label="Claude Code" environment="ANTHROPIC_API_KEY" configured={status.claude} disabled={disabled} onSave={onSave} onDelete={onDelete} />
      <ProviderApiKeyField provider="codex" label="Codex" environment="OPENAI_API_KEY" configured={status.codex} disabled={disabled} onSave={onSave} onDelete={onDelete} />
      <ProviderApiKeyField provider="opencode" label="OpenCode" environment="OPENCODE_API_KEY / OPENCODE_ZEN_API_KEY" configured={status.opencode} disabled={disabled} onSave={onSave} onDelete={onDelete} />
    </>}
  </div>;
}

function ProviderApiKeyField({ provider, label, environment, configured, disabled, onSave, onDelete }: { readonly provider: ProviderName; readonly label: string; readonly environment: string; readonly configured: boolean; readonly disabled: boolean; readonly onSave: (provider: ProviderName, apiKey: string) => Promise<IpcResult<ProviderApiKeyStatus>>; readonly onDelete: (provider: ProviderName) => Promise<IpcResult<ProviderApiKeyStatus>> }): ReactElement {
  const [draft, setDraft] = useState("");
  const [editing, setEditing] = useState(!configured);
  const [busy, setBusy] = useState(false);
  const [fieldError, setFieldError] = useState<string>();
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  useEffect(() => {
    setDraft("");
    setEditing(!configured);
    setFieldError(undefined);
  }, [configured]);
  const save = async (): Promise<void> => {
    if (draft.trim() === "") {
      setFieldError("API キーを入力してください");
      return;
    }
    setBusy(true);
    setFieldError(undefined);
    try {
      const result = await onSave(provider, draft);
      if (result.ok) {
        setDraft("");
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
        setDraft("");
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
  return <div className="provider-api-key-field">
    <label htmlFor={inputId}><span>{label}</span><small>{environment}</small></label>
    <input id={inputId} type="password" autoComplete="new-password" spellCheck={false} value={inputValue} readOnly={!editing} disabled={disabled || busy} placeholder={editing ? "未設定" : undefined} onFocus={() => { if (!editing) { setDraft(""); setEditing(true); } }} onChange={(event) => setDraft(event.target.value)} />
    <span className="provider-api-key-actions">
      {configured && !editing ? <small>保存済み（伏字）</small> : null}
      <button type="button" disabled={disabled || busy} onClick={() => { if (editing) void save(); else setEditing(true); }}>{editing ? "保存" : "変更"}</button>
      {configured ? <button type="button" disabled={disabled || busy} onClick={remove}>削除</button> : null}
    </span>
    {fieldError === undefined ? null : <span className="field-error" role="alert">{fieldError}</span>}
    {deleteConfirmOpen ? <ConfirmationDialog id={`provider-api-key-delete-${provider}`} title="保存済み API キーを削除しますか？" description={`${label} の保存済み API キーを削除します。`} cancelLabel="キャンセル" confirmLabel="削除する" onCancel={() => setDeleteConfirmOpen(false)} onConfirm={() => { void removeConfirmed(); }} /> : null}
  </div>;
}
