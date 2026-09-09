import { useEffect, useRef, useState, type ReactElement } from "react";
import { usePanelPresenter, type PanelCommand } from "../usePanelPresenter.js";

import { avatarApi } from "../ipc.js";
import { useI18n } from "../i18n/index.js";
import { MAX_VRM_BYTES } from "./model-file.js";
import { readVrmImageQuality, readVrmModel, saveVrmImageQuality, saveVrmModel } from "./model-store.js";
import { disposeVrmModel, loadVrmModel, verifyVrmRender } from "./runtime.js";
import { localizeVrmError, vrmError } from "./errors.js";
import { VRM_IMAGE_QUALITIES, type VrmImageQuality } from "./texture-quality.js";
import "./vrm.css";
import MotionControls from "./MotionControls.js";

interface Props {
  readonly controlsOpen: boolean;
  readonly onCloseControls: () => void;
  readonly showClosedError: boolean;
}

export default function VrmControls({ controlsOpen, onCloseControls, showClosedError }: Props): ReactElement {
  const { locale, t } = useI18n();
  const [consent, setConsent] = useState(false);
  const files = useRef(new Map<string, File>());
  const storedRef = useRef<{ name: string; data: ArrayBuffer } | undefined>(undefined);
  const loading = useRef<AbortController | undefined>(undefined);
  const presenter = usePanelPresenter<{
    modelName: string | null; imageQuality: VrmImageQuality | null; phase: "loading" | "saving" | null; busy: boolean; error: string | null; aborted: boolean;
  }, PanelCommand & { payload: VrmImageQuality | "select" | "remove" | "quality" | { fileId: string; quality: VrmImageQuality } }>("vrm", null, async (command) => {
    try {
      switch (command.kind) {
        case "load": {
          const controller = new AbortController();
          loading.current = controller;
          const [stored, quality] = await Promise.all([readVrmModel(), readVrmImageQuality()]);
          controller.signal.throwIfAborted();
          return { ok: true, value: { name: stored?.name ?? null, quality } };
        }
        case "verify": {
          const request = command.payload as { fileId: string; quality: VrmImageQuality };
          const file = files.current.get(request.fileId);
          if (file === undefined) throw new Error("VRM file is missing");
          const controller = new AbortController();
          loading.current = controller;
          if (file.size > MAX_VRM_BYTES) throw vrmError("vrm.errors.tooLarge");
          const stored = { name: file.name, data: await file.arrayBuffer() };
          controller.signal.throwIfAborted();
          const candidate = await loadVrmModel(stored, controller.signal, request.quality);
          try { controller.signal.throwIfAborted(); verifyVrmRender(candidate); storedRef.current = stored; }
          finally { disposeVrmModel(candidate); }
          break;
        }
        case "save": {
          if (storedRef.current === undefined) throw new Error("Verified VRM is missing");
          await saveVrmModel(storedRef.current); break;
        }
        case "remove": await saveVrmModel(undefined); break;
        case "saveQuality": await saveVrmImageQuality(command.payload as VrmImageQuality); break;
        case "notify": {
          const result = await avatarApi.modelChanged();
          if (!result.ok) return { ok: false, error: { message: t(command.payload === "remove" ? "vrm.avatarNotifyAfterRemoveFailed" : command.payload === "quality" ? "vrm.qualityNotifyFailed" : "vrm.avatarNotifyFailed", { cause: result.error.message }) } };
          break;
        }
        case "show": {
          const result = await avatarApi.show();
          if (!result.ok) return { ok: false, error: { message: t("vrm.avatarShowFailed", { cause: result.error.message }) } };
          break;
        }
        case "hide": return avatarApi.hide();
        case "toggle": return avatarApi.toggle();
        case "abort": loading.current?.abort(); break;
        case "release": storedRef.current = undefined; files.current.clear(); break;
        default: throw new Error(`Unknown VRM command: ${command.kind}`);
      }
      return { ok: true, value: null };
    } catch (cause) {
      const message = loading.current?.signal.aborted === true ? t("vrm.errors.aborted") : t(
        command.kind === "load" ? "vrm.savedLoadFailed" : command.kind === "saveQuality" ? "vrm.qualityChangeFailed" : command.kind === "remove" ? "vrm.removeFailed" : "vrm.changeFailed",
        { cause: localizeVrmError(cause, locale) },
      );
      return { ok: false, error: { message } };
    }
  });
  useEffect(() => () => { loading.current?.abort(); storedRef.current = undefined; files.current.clear(); }, []);
  const action = (name: string, value: unknown = null): void => presenter.send({ type: "action", name, value });
  const modelName = presenter.state?.modelName ?? undefined;
  const imageQuality = presenter.state?.imageQuality;
  const phase = presenter.state?.phase;
  const busy = presenter.state?.busy ?? true;
  const error = presenter.state?.aborted === true ? t("vrm.errors.aborted") : presenter.error ?? presenter.state?.error ?? undefined;
  return <>
    {!controlsOpen && showClosedError && error !== undefined ? <p className="vrm-error" role="alert">{error}</p> : null}
    {controlsOpen ? <section className="vrm-controls" aria-label={t("vrm.settings")}>
      <div className="vrm-heading"><strong>{t("vrm.label")}</strong><button type="button" disabled={busy} onClick={onCloseControls}>{t("vrm.close")}</button></div>
      <p>{t("vrm.description")}</p>
      <p>{t("vrm.privacy")}</p>
      <p>{t("vrm.avatarWindow")}</p>
      <p><a href="/vrm-licenses.txt" target="_blank" rel="noreferrer">{t("vrm.licenses")}</a></p>
      <label><input type="checkbox" checked={consent} onChange={(event) => setConsent(event.target.checked)} />{t("vrm.consent")}</label>
      <label>{t("vrm.file")}<input type="file" accept=".vrm" disabled={busy || !consent} onChange={(event) => {
        const file = event.currentTarget.files?.[0];
        event.currentTarget.value = "";
        if (file !== undefined) { const fileId = crypto.randomUUID(); files.current.set(fileId, file); action("select", { fileId, name: file.name }); }
      }} /></label>
      <label>{t("vrm.imageQuality")}<select value={imageQuality ?? ""} disabled={busy} onChange={(event) => { action("quality", event.target.value); }}>
        {VRM_IMAGE_QUALITIES.map((quality) => <option key={quality} value={quality}>{t(`vrm.quality.${quality}`)}</option>)}
      </select><small>{t("vrm.imageQualityHelp")}</small></label>
      <p role="status">{busy ? t("vrm.loading") : modelName === undefined ? t("vrm.unset") : t("vrm.using", { name: modelName })}</p>
      <button type="button" disabled={busy} onClick={() => { action("toggle"); }}>{t("vrm.avatarToggle")}</button>
      {phase === "loading" ? <button type="button" onClick={() => action("cancel")}>{t("vrm.cancelLoading")}</button> : null}
      {error === undefined ? null : <p role="alert">{error}</p>}
      <button type="button" disabled={busy} onClick={() => action("remove")}>{t("vrm.remove")}</button>
      <MotionControls />
    </section> : null}
  </>;
}
