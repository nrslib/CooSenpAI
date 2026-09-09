import { useEffect, useState, type ReactElement } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { avatarApi, setIpcLocale } from "../ipc.js";
import { AvatarScene } from "./AvatarScene.js";
import type { AvatarSceneInput, AvatarSceneView } from "./scene-view.js";
import { renderUiText } from "../app-view.js";
import { I18nProvider, t } from "../i18n/index.js";

export function AvatarOverlay(): ReactElement {
  const [view, setView] = useState<AvatarSceneView>();
  const send = (event: AvatarSceneInput) => { void avatarApi.sceneInput(event); };
  useEffect(() => {
    let disposed = false;
    const subscription = avatarApi.subscribeSceneView((next) => { setIpcLocale(next.snapshot.language); setView(next); });
    void subscription.ready.then(() => { if (!disposed) send({ type: "mounted", hidden: document.hidden }); });
    const visibility = () => send({ type: "visibility", hidden: document.hidden });
    document.addEventListener("visibilitychange", visibility);
    return () => { disposed = true; subscription.dispose(); document.removeEventListener("visibilitychange", visibility); send({ type: "unmounted" }); };
  }, []);
  const locale = view?.snapshot.language ?? "ja";
  useEffect(() => {
    if (view === undefined || view.dragRequest === 0) return;
    void getCurrentWindow().startDragging().catch((cause: unknown) => send({ type: "operationFailed", error: t(locale, "avatar.moveFailed", { cause: String(cause) }) }));
  }, [view?.dragRequest]);
  return <I18nProvider locale={locale}><main className="avatar-overlay">
    <div className="avatar-toolbar">
      <button className="avatar-drag" type="button" aria-label={t(locale, "avatar.move")} title={t(locale, "avatar.dragToMove")} onPointerDown={(event) => send({ type: "drag", button: event.button })}>⠿</button>
      <button className="avatar-close" type="button" aria-label={t(locale, "avatar.hide")} title={t(locale, "avatar.hideTitle")} onClick={() => send({ type: "hide" })}>×</button>
    </div>
    {view === undefined ? null : <AvatarScene view={view} onEvent={send} />}
    {view?.status === null || view?.status === undefined ? null : <div className="avatar-notice"><p role={view.retryable ? "alert" : "status"}>{renderUiText(view.status, (key, args) => t(locale, key, args))}</p>{view.retryable ? <button type="button" onClick={() => send({ type: "retry" })}>{t(locale, "avatar.reload")}</button> : null}</div>}
  </main></I18nProvider>;
}
