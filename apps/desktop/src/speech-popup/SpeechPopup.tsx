import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactElement } from "react";
import { setIpcLocale, speechPopupApi } from "../ipc.js";
import type { SpeechPopupSnapshot } from "../types.js";
import { applyAppearance } from "../appearance.js";
import { AvatarBlob } from "../components/AvatarBlob.js";
import { t, type Locale } from "../i18n/index.js";
import { usePresenterDraft } from "../usePresenterDraft.js";

export function SpeechPopupView(): ReactElement {
  const [snapshot, setSnapshot] = useState<SpeechPopupSnapshot>();
  const { value: text, begin, receive, edit } = usePresenterDraft();
  const generation = useRef<number | undefined>(undefined);
  const [error, setError] = useState<string>();
  const [sending, setSending] = useState(false);
  const [focusRequest, setFocusRequest] = useState(0);
  const [canSend, setCanSend] = useState(false);
  const textarea = useRef<HTMLTextAreaElement>(null);
  useEffect(() => {
    const load = speechPopupApi.subscribeLoad((next) => {
      if (generation.current !== undefined && next.speech.generation < generation.current) return;
      generation.current = next.speech.generation;
      begin(next.speech.generation);
      setIpcLocale(next.language);
      applyAppearance({ theme: next.theme, font: next.font });
      setSnapshot(next);
    });
    const input = speechPopupApi.subscribeInput((next) => {
      if (generation.current !== undefined && next.generation < generation.current) return;
      generation.current = next.generation;
      receive(next, next.text);
      setSending(next.sending);
      setCanSend(next.canSend);
      setError(next.error);
    });
    const focus = speechPopupApi.subscribeFocus(() => setFocusRequest((request) => request + 1));
    void Promise.all([load.ready, input.ready, focus.ready]).then(() => speechPopupApi.ready());
    return () => { load.dispose(); input.dispose(); focus.dispose(); };
  }, [begin, receive]);
  useLayoutEffect(() => { if (focusRequest > 0) textarea.current?.focus(); }, [focusRequest]);
  const cancel = (): void => { if (snapshot) void speechPopupApi.cancel(snapshot.speech.generation); };
  const send = (): void => {
    if (snapshot) void speechPopupApi.send(snapshot.speech.generation);
  };
  const keyDown = (event: KeyboardEvent): void => {
    if (!snapshot) return;
    if (event.key === "Escape" || (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing && event.keyCode !== 229)) event.preventDefault();
    void speechPopupApi.key(snapshot.speech.generation, {
      key: event.key, metaKey: event.metaKey, shiftKey: event.shiftKey,
      composing: event.nativeEvent.isComposing, keyCode: event.keyCode,
    });
  };
  const speech = snapshot?.speech;
  const locale: Locale = snapshot?.language ?? "ja";
  const errorMessage = error ?? speech?.message ?? undefined;
  return <main className="speech-card" onKeyDown={keyDown}>
    <header><AvatarBlob color={snapshot?.avatarColor} image={snapshot?.avatarImagePng} size={24} state={speech?.phase === "recording" ? "thinking" : "open"} /><span className={`recording-dot${speech?.phase === "recording" ? " active" : ""}`} /><strong>{t(locale, "speechPopup.title", { name: snapshot?.companionDisplayName ?? t(locale, "modelPopup.companionDefault") })}</strong></header>
    {speech?.phase === "confirming"
      ? <textarea ref={textarea} value={text} disabled={sending} onChange={(event) => {
        if (!snapshot) return;
        const text = event.target.value;
        const editRevision = edit(snapshot.speech.generation, text);
        if (editRevision !== undefined) void speechPopupApi.edit(snapshot.speech.generation, editRevision, text);
      }} aria-label={t(locale, "speechPopup.transcriptLabel")} />
      : <p className={speech?.partial ? "transcript" : "listening"}>{speech?.partial || t(locale, "speechPopup.listening")}</p>}
    {errorMessage === undefined ? null : <p className="error">{errorMessage}</p>}
    <footer>
      <button type="button" disabled={sending || speech?.phase === "sending"} onClick={cancel}>{t(locale, "common.cancel")}</button>
      {speech?.phase === "confirming" ? <button className="primary" type="button" disabled={!canSend} onClick={() => void send()}>{sending ? t(locale, "common.sending") : t(locale, "common.send")}</button> : null}
    </footer>
  </main>;
}
