import { useEffect, useRef, useState, type KeyboardEvent, type ReactElement } from "react";
import { speechPopupApi } from "../ipc.js";
import type { SpeechPopupSnapshot } from "../types.js";
import {
  isSpeechPopupErrorCurrent,
  shouldInitializeConfirmationDraft,
  shouldKeepSpeechPopupError,
  speechPopupErrorMessage,
  speechPopupKeyAction,
  type SpeechPopupTransientError,
} from "./state.js";
import { applyAppearance } from "../appearance.js";
import { AvatarBlob } from "../components/AvatarBlob.js";

export function SpeechPopup(): ReactElement {
  const [snapshot, setSnapshot] = useState<SpeechPopupSnapshot>();
  const [text, setText] = useState("");
  const [error, setError] = useState<SpeechPopupTransientError>();
  const [loadError, setLoadError] = useState<string>();
  const [sending, setSending] = useState(false);
  const sendingRef = useRef(false);
  const nextErrorId = useRef(0);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const initializedGeneration = useRef<number | undefined>(undefined);
  useEffect(() => {
    let revision = 0;
    const apply = (next: SpeechPopupSnapshot): void => {
      if (next.revision <= revision) return;
      revision = next.revision;
      applyAppearance({ theme: next.theme, font: next.font });
      setSnapshot(next);
      setLoadError(undefined);
      setError((current) => shouldKeepSpeechPopupError(current, next.speech) ? current : undefined);
      if (shouldInitializeConfirmationDraft(initializedGeneration.current, next.speech)) {
        initializedGeneration.current = next.speech.generation;
        setText(next.speech.partial);
      }
    };
    const events = speechPopupApi.subscribeSnapshots((event) => apply({
      revision: event.revision,
      companionDisplayName: event.snapshot.companionDisplayName,
      speech: event.snapshot.speech,
      theme: event.snapshot.config.ui.theme,
      font: event.snapshot.config.ui.font,
      avatarColor: event.snapshot.config.ui.avatarColor ?? undefined,
      avatarImagePng: event.snapshot.avatarImagePng,
    }));
    void events.ready.then(() => speechPopupApi.getSnapshot()).then((result) => {
      if (result.ok) apply(result.value); else setLoadError(result.error.message);
    });
    return () => events.dispose();
  }, []);
  useEffect(() => {
    if (snapshot?.speech.phase === "confirming") requestAnimationFrame(() => textarea.current?.focus());
  }, [snapshot?.speech.phase]);
  const cancel = (): void => {
    if (sending || snapshot?.speech.phase === "sending") return;
    void speechPopupApi.cancel();
  };
  const send = async (): Promise<void> => {
    if (sendingRef.current) return;
    sendingRef.current = true;
    setSending(true);
    const result = await speechPopupApi.send(text);
    if (result.ok) {
      setError(undefined);
    } else {
      const id = nextErrorId.current + 1;
      nextErrorId.current = id;
      setError({ id, generation: snapshot?.speech.generation, message: result.error.message });
      window.setTimeout(() => {
        setError((current) => isSpeechPopupErrorCurrent(current, id) ? undefined : current);
      }, 3_000);
    }
    sendingRef.current = false;
    setSending(false);
  };
  const keyDown = (event: KeyboardEvent): void => {
    const action = speechPopupKeyAction(
      event.key,
      event.shiftKey,
      event.nativeEvent.isComposing,
      event.keyCode,
      snapshot?.speech.phase ?? "idle",
    );
    if (action === "ignore") return;
    event.preventDefault();
    if (action === "cancel") cancel();
    if (action === "send") void send();
  };
  const speech = snapshot?.speech;
  const errorMessage = speechPopupErrorMessage(speech, error, loadError);
  return <main className="speech-card" onKeyDown={keyDown}>
    <header><AvatarBlob color={snapshot?.avatarColor} image={snapshot?.avatarImagePng} size={24} state={speech?.phase === "recording" ? "thinking" : "open"} /><span className={`recording-dot${speech?.phase === "recording" ? " active" : ""}`} /><strong>{snapshot?.companionDisplayName ?? "Coo"} に話しかける</strong></header>
    {speech?.phase === "confirming"
      ? <textarea ref={textarea} value={text} disabled={sending} onChange={(event) => setText(event.target.value)} aria-label="文字起こしを確認" />
      : <p className={speech?.partial ? "transcript" : "listening"}>{speech?.partial || "聞いています…"}</p>}
    {errorMessage === undefined ? null : <p className="error">{errorMessage}</p>}
    <footer>
      <button type="button" disabled={sending || speech?.phase === "sending"} onClick={cancel}>取り消し</button>
      {speech?.phase === "confirming" ? <button className="primary" type="button" disabled={sending || text.trim().length === 0} onClick={() => void send()}>{sending ? "送信中…" : "送信"}</button> : null}
    </footer>
  </main>;
}
