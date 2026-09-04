import { useState, type KeyboardEvent as ReactKeyboardEvent, type ReactElement } from "react";

import type { IpcResult, PersonaDocument, PersonaOption } from "../types.js";
import { CloseIcon } from "./LineIcons.js";
import { ConfirmationDialog } from "./ConfirmationDialog.js";

interface Props {
  readonly option: PersonaOption;
  readonly document: PersonaDocument;
  readonly onSave: (id: string, displayName: string, body: string) => Promise<IpcResult<unknown>>;
  readonly onDelete: (id: string) => Promise<IpcResult<unknown>>;
  readonly onRestore: (id: string, version: string) => Promise<IpcResult<unknown>>;
  readonly onClose: () => void;
}

export function PersonaEditor({ option, document, onSave, onDelete, onRestore, onClose }: Props): ReactElement {
  const [id, setId] = useState(option.builtin ? `${option.id}-custom` : option.id);
  const [displayName, setDisplayName] = useState(option.builtin ? "" : option.displayName);
  const [body, setBody] = useState(document.body);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [deleteConfirmOpen, setDeleteConfirmOpen] = useState(false);
  const save = async (): Promise<void> => {
    setBusy(true);
    const result = await onSave(id, displayName, body);
    setBusy(false);
    if (result.ok) onClose();
    else setError(result.error.message);
  };
  const deletePersona = async (): Promise<void> => {
    setDeleteConfirmOpen(false);
    setBusy(true);
    const result = await onDelete(option.id);
    setBusy(false);
    if (result.ok) onClose();
    else setError(result.error.message);
  };
  return <div className="dialog-overlay" role="presentation" onKeyDown={(event) => {
    closePersonaEditorOnEscape(event, onClose);
  }}><section className="persona-editor" role="dialog" aria-modal="true" aria-label="性格を編集">
    <div className="settings-heading"><h2>{option.builtin ? "複製して自分の性格を作る" : "性格を編集"}</h2><button type="button" aria-label="閉じる" onClick={onClose}><CloseIcon /></button></div>
    <label>ID<input value={id} disabled={!option.builtin} pattern="[A-Za-z0-9-]+" maxLength={64} onChange={(event) => setId(event.target.value)} /></label>
    <label>一覧に表示する名前<input value={displayName} maxLength={40} onChange={(event) => setDisplayName(event.target.value)} /></label>
    <label>本文<textarea rows={14} value={body} onChange={(event) => setBody(event.target.value)} /></label>
    {document.versions.length === 0 ? null : <label>前の版に戻す<select defaultValue="" onChange={(event) => {
      if (event.target.value !== "") void onRestore(option.id, event.target.value).then((result) => result.ok ? onClose() : setError(result.error.message));
    }}><option value="">版を選択</option>{document.versions.map((version) => <option key={version.id} value={version.id}>{version.createdAt}</option>)}</select></label>}
    {error === undefined ? null : <p className="field-error">{error}</p>}
    <div className="button-row"><button className="primary" type="button" disabled={busy || !canSavePersona(id, displayName, body)} onClick={() => void save()}>保存</button>{option.builtin ? null : <button type="button" disabled={busy} onClick={() => setDeleteConfirmOpen(true)}>削除</button>}<button type="button" onClick={onClose}>取り消し</button></div>
  </section>{deleteConfirmOpen ? <ConfirmationDialog id="persona-delete" title="性格を削除しますか？" description="この性格を削除します。" cancelLabel="キャンセル" confirmLabel="削除する" onCancel={() => setDeleteConfirmOpen(false)} onConfirm={() => { void deletePersona(); }} /> : null}</div>;
}

export function shouldClosePersonaEditor(event: Pick<KeyboardEvent, "key" | "isComposing" | "keyCode">): boolean {
  return event.key === "Escape" && !event.isComposing && event.keyCode !== 229;
}

export function closePersonaEditorOnEscape(
  event: Pick<ReactKeyboardEvent, "nativeEvent" | "stopPropagation">,
  onClose: () => void,
): boolean {
  if (!shouldClosePersonaEditor(event.nativeEvent)) return false;
  event.stopPropagation();
  onClose();
  return true;
}

export function canSavePersona(id: string, displayName: string, body: string): boolean {
  return /^[A-Za-z0-9-]{1,64}$/u.test(id) && displayName.trim() !== "" && body.trim() !== "";
}
