import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactElement } from "react";
import { capturePopupApi, setIpcLocale } from "../ipc.js";
import type { CapturePopupSnapshot } from "../types.js";
import { applyAppearance } from "../appearance.js";
import { AvatarBlob } from "../components/AvatarBlob.js";
import { t, type Locale } from "../i18n/index.js";
import { composerKeyAction } from "../view-model.js";
import { usePresenterDraft } from "../usePresenterDraft.js";

export function CapturePopupView(): ReactElement {
  const [generation, setGeneration] = useState<number>();
  const currentGeneration = useRef<number | undefined>(undefined);
  const [snapshot, setSnapshot] = useState<CapturePopupSnapshot>();
  const { value: message, begin, receive, edit } = usePresenterDraft();
  const [error, setError] = useState<string>();
  const [sending, setSending] = useState(false);
  const [focusRequest, setFocusRequest] = useState(0);
  const [previewState, setPreviewState] = useState<"loading" | "ready" | "failed">("loading");
  const [canSend, setCanSend] = useState(false);
  const input = useRef<HTMLTextAreaElement>(null);
  const locale: Locale = snapshot?.language ?? "ja";
  useEffect(() => {
    const session = capturePopupApi.subscribeSession((next) => {
      if (currentGeneration.current !== undefined && next <= currentGeneration.current) return;
      currentGeneration.current = next;
      setGeneration(next); begin(next); setSnapshot(undefined); setError(undefined); setSending(false); setCanSend(false); setPreviewState("loading");
    });
    const load = capturePopupApi.subscribeLoad((next) => {
      if (currentGeneration.current !== undefined && next.generation < currentGeneration.current) return;
      currentGeneration.current = next.generation; setGeneration(next.generation);
      begin(next.generation);
      setSnapshot(next);
      setIpcLocale(next.language);
      applyAppearance({ theme: next.theme, font: next.font });
    });
    const update = capturePopupApi.subscribeInput((next) => {
      if (currentGeneration.current !== undefined && next.generation < currentGeneration.current) return;
      currentGeneration.current = next.generation; setGeneration(next.generation);
      receive(next, next.message);
      setSending(next.sending);
      setCanSend(next.canSend);
      setPreviewState(next.previewState);
      setError(next.error ?? undefined);
    });
    const focus = capturePopupApi.subscribeFocus(() => {
      setFocusRequest((request) => request + 1);
    });
    void Promise.all([session.ready, load.ready, update.ready, focus.ready]).then(() => capturePopupApi.ready());
    return () => { session.dispose(); load.dispose(); update.dispose(); focus.dispose(); };
  }, [begin, receive]);
  useLayoutEffect(() => { if (focusRequest > 0) focusCapturePopupInput(input.current); }, [focusRequest]);
  const preview = useMemo(() => {
    if (snapshot?.png === undefined) return undefined;
    const bytes = new Uint8Array(snapshot.png);
    return URL.createObjectURL(new Blob([bytes.buffer as ArrayBuffer], { type: "image/png" }));
  }, [snapshot?.png]);
  useEffect(() => () => { if (preview !== undefined) URL.revokeObjectURL(preview); }, [preview]);
  useEffect(() => {
    const keydown = (event: KeyboardEvent): void => {
      if (generation === undefined) return;
      if (event.key === "Escape") { event.preventDefault(); void capturePopupApi.cancel(generation, "esc"); return; }
      if (snapshot === undefined) return;
      const action = composerKeyAction(event.key, event.metaKey, event.shiftKey, event.isComposing, event.keyCode, snapshot.sendKey);
      if (action === "newline") return;
      if (action === "send") event.preventDefault();
      void capturePopupApi.key(generation, {
        key: event.key, metaKey: event.metaKey, shiftKey: event.shiftKey,
        composing: event.isComposing, keyCode: event.keyCode,
      });
    };
    window.addEventListener("keydown", keydown);
    return () => window.removeEventListener("keydown", keydown);
  }, [generation, snapshot?.sendKey]);
  return <main className="capture-shell">
    <section className="capture-card" aria-label={t(locale, "capture.sendAttachment")}>
      <header><AvatarBlob color={snapshot?.avatarColor} image={snapshot?.avatarImagePng} size={28} /><div><strong>{snapshot?.companionDisplayName ?? t(locale, "modelPopup.companionDefault")}</strong><span>{t(locale, "capture.showTo", { kind: snapshot?.attachmentKind === "text" ? t(locale, "capture.text") : t(locale, "capture.screen") })}</span></div></header>
      {snapshot?.attachmentKind === "text"
        ? <div className="text-attachment-preview"><pre>{snapshot.textPreview}</pre>{snapshot.textTruncated ? <small>{t(locale, "capture.textTruncated", { count: snapshot.textTruncatedCharacters })}</small> : snapshot.textPreviewTruncated ? <small>{t(locale, "capture.textPreviewTruncated")}</small> : null}</div>
        : previewState !== "ready" ? <CapturePreviewPlaceholder state={previewState} locale={locale} onRetry={() => { if (generation !== undefined) void capturePopupApi.retry(generation); }} /> : <img className="capture-preview" src={preview ?? ""} alt={t(locale, "capture.selectedScreen")} />}
      {snapshot?.accessibilityPermissionRequired === true ? <div className="capture-accessibility-notice" role="status"><span>{t(locale, "capture.accessibilityRequired")}</span><button type="button" className="secondary" disabled={sending} onClick={() => { void capturePopupApi.openAccessibilitySettings(); }}>{t(locale, "common.openSettings")}</button></div> : null}
      {snapshot === undefined || snapshot.quickActions.length === 0 ? null : <div className="quick-actions" aria-label={t(locale, "capture.quickActions")}>{snapshot.quickActions.map((action, index) => <button type="button" key={`${action.label}-${index}`} disabled={!canSend || sending} onClick={() => { void capturePopupApi.action(snapshot.generation, index); }}>{action.label}</button>)}</div>}
      <form onSubmit={(event) => { event.preventDefault(); if (snapshot !== undefined) void capturePopupApi.send(snapshot.captureId); }}>
        <textarea ref={input} value={message} disabled={sending} maxLength={32_768}
          placeholder={t(locale, "capture.messagePlaceholder")} aria-label={t(locale, "capture.messageLabel")}
          onChange={(event) => {
            if (generation === undefined) return;
            const message = event.target.value;
            const editRevision = edit(generation, message);
            if (editRevision !== undefined) void capturePopupApi.edit(generation, editRevision, message);
          }} />
        <div className="capture-actions">
          <button type="button" className="secondary" onClick={() => { if (generation !== undefined) void capturePopupApi.cancel(generation, "closeButton"); }}>{t(locale, "common.cancel")}</button>
          <button type="submit" disabled={!canSend || sending}>{sending ? t(locale, "common.sending") : t(locale, "common.send")}</button>
        </div>
      </form>
      {error === undefined ? null : <p className="capture-error" role="alert">{error}</p>}
    </section>
  </main>;
}

/// プレビュー領域の画像未到着時の表示。ローディング中は回転スピナー（CSS のみ）と文字、
/// 失敗時は文言と再読込ボタン。
export function CapturePreviewPlaceholder({ state, locale, onRetry }: { readonly state: "loading" | "failed"; readonly locale: Locale; readonly onRetry: () => void }): ReactElement {
  return <div className="capture-loading" role="status" aria-busy={state === "loading"}>
    {state === "loading"
      ? <><span className="capture-spinner" aria-hidden="true" /><span>{t(locale, "capture.preparingImage")}</span></>
      : <><span>{t(locale, "capture.imageLoadFailed")}</span><button type="button" onClick={onRetry}>{t(locale, "common.retry")}</button></>}
  </div>;
}


export function focusCapturePopupInput(input: Pick<HTMLTextAreaElement, "focus"> | null): void {
  input?.focus();
}
