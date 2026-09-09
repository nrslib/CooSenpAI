import { useEffect, useRef, useState } from "react";
import { motionApi } from "../ipc.js";
import { useI18n } from "../i18n/index.js";
import { renderUiText } from "../app-view.js";
import { motionSlots, type MotionSlot } from "./motion-settings.js";
import type { MotionInput, MotionView } from "./motion-view.js";
import { executeMotionStorage } from "./motion-storage-port.js";

export default function MotionControls() {
  const { locale, t } = useI18n();
  const [view, setView] = useState<MotionView>();
  const [consent, setConsent] = useState(false);
  const files = useRef(new Map<string, File>());
  const currentLocale = useRef(locale); currentLocale.current = locale;
  const send = (event: MotionInput) => { void motionApi.input(event); };
  useEffect(() => {
    let disposed = false;
    const operations = new Set<AbortController>();
    const views = motionApi.subscribeView(setView);
    const storage = motionApi.subscribeStorage((command) => {
      if (command.kind === "release") { files.current.delete(command.fileId); return; }
      const controller = new AbortController(); operations.add(controller);
      void executeMotionStorage(command, files.current, controller.signal, currentLocale.current).then(send).finally(() => operations.delete(controller));
    });
    void Promise.all([views.ready, storage.ready]).then(() => { if (!disposed) send({ type: "mounted" }); });
    return () => { disposed = true; views.dispose(); storage.dispose(); for (const operation of operations) operation.abort(); files.current.clear(); send({ type: "unmounted" }); };
  }, []);
  return <section aria-label={t("motions.settings")} className="motion-controls">
    <h3>{t("motions.title")}</h3><p>{t("motions.storageHelp")}</p><p>{t("motions.playbackHelp")}</p>
    <label><input type="checkbox" checked={consent} onChange={event => setConsent(event.target.checked)} />{t("motions.consent")}</label><p>{t("motions.formatHelp")}</p>
    {view === undefined ? null : (Object.keys(motionSlots) as MotionSlot[]).map(slot => {
      const selection = view.settings?.[slot];
      const value = selection?.kind === "builtin" ? selection.id : selection?.kind === "file" ? "file" : "none";
      return <fieldset key={slot} disabled={view.busy} className="motion-row">
        <legend>{t(motionSlots[slot])}</legend>
        <label>{t("motions.selection")}<select aria-label={t("motions.slotMotion", { slot: t(motionSlots[slot]) })} value={value} disabled={view.settings === null} onChange={event => send({ type: "select", slot, value: event.target.value })}>
          {slot !== "idle" ? <option value="none">{t("motions.unassigned")}</option> : null}
          <option value="idle">{t("motions.builtinIdle")}</option><option value="reply">{t("motions.builtinReply")}</option>
          {selection?.kind === "file" ? <option value="file">{t("motions.imported", { name: selection.name })}</option> : null}
        </select></label>
        <label>{t("motions.import")}<input aria-label={t("motions.slotFile", { slot: t(motionSlots[slot]) })} type="file" accept=".vrma" disabled={!consent || view.settings === null} onChange={event => {
          const file = event.currentTarget.files?.[0]; event.currentTarget.value = "";
          if (file !== undefined) { const fileId = crypto.randomUUID(); files.current.set(fileId, file); send({ type: "import", slot, fileId, name: file.name, size: file.size, consent }); }
        }} /></label>
        <div className="motion-actions"><button type="button" disabled={!view.previewable.includes(slot)} onClick={() => send({ type: "preview", slot })}>{t("motions.preview")}</button>
          <button type="button" onClick={() => send({ type: "reset", slot })}>{t("motions.reset")}</button></div>
      </fieldset>;
    })}
    {view?.busy ? <p role="status">{t("motions.processing")}</p> : null}
    {view?.status == null ? null : <p role="status">{renderUiText(view.status, t)}</p>}
    {view?.error == null ? null : <p role="alert">{renderUiText(view.error, t)}</p>}
  </section>;
}
