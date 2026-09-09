import { useEffect, useState, type ReactElement } from "react";

import { usePanelPresenter, type PanelCommand } from "../usePanelPresenter.js";
import { useI18n } from "../i18n/index.js";
import type { IpcResult, PersonaDocument, PersonaOption } from "../types.js";
import { CloseIcon } from "./LineIcons.js";
import { ConfirmationDialog } from "./ConfirmationDialog.js";

interface Props {
  readonly option: PersonaOption;
  readonly document: PersonaDocument;
  readonly onSave: (id: string, displayName: string, body: string) => Promise<IpcResult<unknown>>;
  readonly onDelete: (id: string) => Promise<IpcResult<unknown>>;
  readonly onRestore: (id: string, version: string) => Promise<IpcResult<unknown>>;
  readonly onRefresh: () => Promise<IpcResult<unknown>>;
  readonly onClose: () => void;
}

export function PersonaEditor({ option, document, onSave, onDelete, onRestore, onRefresh, onClose }: Props): ReactElement {
  const { t } = useI18n();
  const [id, setId] = useState(option.builtin ? `${option.id}-custom` : option.id);
  const [displayName, setDisplayName] = useState(option.builtin ? "" : option.displayName);
  const [body, setBody] = useState(document.body);
  const presenter = usePanelPresenter<{
    busy: boolean; canSave: boolean; deleteConfirmOpen: boolean; error: string | null;
  }, PanelCommand & { payload: string | { id: string; displayName: string; body: string; version: string } }>("persona",
    { builtin: option.builtin, originalId: option.id, draft: { id, displayName, body } }, async (command) => {
      switch (command.kind) {
        case "save": { const value = command.payload as { id: string; displayName: string; body: string }; return onSave(value.id, value.displayName, value.body); }
        case "delete": return onDelete(command.payload as string);
        case "restore": { const value = command.payload as { id: string; version: string }; return onRestore(value.id, value.version); }
        case "refresh": return onRefresh();
        case "close": onClose(); return { ok: true, value: null };
        default: throw new Error(`Unknown persona command: ${command.kind}`);
      }
    });
  useEffect(() => presenter.send({ type: "change", value: { id, displayName, body } }), [id, displayName, body, presenter.send]);
  const action = (name: string, value: unknown = null): void => presenter.send({ type: "action", name, value });
  const close = (): void => action("close");
  const busy = presenter.state?.busy ?? true;
  const error = presenter.error ?? presenter.state?.error ?? undefined;
  const deleteConfirmOpen = presenter.state?.deleteConfirmOpen === true;
  return <div className="dialog-overlay" role="presentation" onKeyDown={(event) => {
    if (event.key === "Escape" && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229 && !busy) event.stopPropagation();
    action("key", { key: event.key, composing: event.nativeEvent.isComposing, keyCode: event.nativeEvent.keyCode });
  }}><section className="persona-editor" role="dialog" aria-modal="true" aria-label={t("settings.personaEditor.aria")}>
    <div className="settings-heading"><h2>{option.builtin ? t("settings.personaEditor.duplicateTitle") : t("settings.personaEditor.editTitle")}</h2><button type="button" aria-label={t("settings.personaEditor.close")} disabled={busy} onClick={close}><CloseIcon /></button></div>
    <label>{t("settings.personaEditor.id")}<input value={id} disabled={!option.builtin} pattern="[A-Za-z0-9-]+" maxLength={64} onChange={(event) => setId(event.target.value)} /></label>
    <label>{t("settings.personaEditor.displayName")}<input value={displayName} maxLength={40} onChange={(event) => setDisplayName(event.target.value)} /></label>
    <label>{t("settings.personaEditor.body")}<textarea rows={14} value={body} onChange={(event) => setBody(event.target.value)} /></label>
    {document.versions.length === 0 ? null : <label>{t("settings.personaEditor.restore")}<select defaultValue="" onChange={(event) => {
      action("restore", event.target.value);
    }}><option value="">{t("settings.personaEditor.selectVersion")}</option>{document.versions.map((version) => <option key={version.id} value={version.id}>{version.createdAt}</option>)}</select></label>}
    {error === undefined ? null : <p className="field-error">{error}</p>}
    <div className="button-row"><button className="primary" type="button" disabled={presenter.state?.canSave !== true} onClick={() => action("save", { id, displayName, body })}>{t("settings.personaEditor.save")}</button>{option.builtin ? null : <button type="button" disabled={busy} onClick={() => action("requestDelete")}>{t("settings.personaEditor.delete")}</button>}<button type="button" disabled={busy} onClick={close}>{t("common.cancel")}</button></div>
  </section>{deleteConfirmOpen ? <ConfirmationDialog id="persona-delete" title={t("settings.personaEditor.deleteTitle")} description={t("settings.personaEditor.deleteDescription")} cancelLabel={t("common.cancel")} confirmLabel={t("settings.personaEditor.delete")} onCancel={() => action("cancelDelete")} onConfirm={() => action("delete")} /> : null}</div>;
}
