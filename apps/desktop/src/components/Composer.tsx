import { useLayoutEffect, useRef, useState, type ReactElement } from "react";
import { initialComposerDom, transitionComposerDom, type ComposerDomEvent } from "../composer-dom.js";
import { resizeComposerTextarea } from "../composer-layout.js";
import { formatShortcutLabel } from "../shortcut-label.js";
import type { ComposerInput, ComposerView } from "../composer-view.js";
import { useI18n } from "../i18n/index.js";

interface Props {
  readonly displayName: string;
  readonly sendKey: "enter" | "cmdEnter";
  readonly microphoneShortcut?: string | null;
  readonly view: ComposerView;
  readonly onEvent: (event: ComposerInput) => void;
  readonly onFocus: () => void;
  readonly onBlur: () => void;
}

export function Composer({ displayName, sendKey, microphoneShortcut, view, onEvent, onFocus, onBlur }: Props): ReactElement {
  const { t } = useI18n();
  const textarea = useRef<HTMLTextAreaElement>(null);
  const [dom, setDom] = useState(() => initialComposerDom(view));
  const current = useRef(dom);
  const dispatch = (event: ComposerDomEvent) => {
    const result = transitionComposerDom(current.current, event);
    if (result.body !== undefined && textarea.current !== null && textarea.current.value !== result.body) {
      textarea.current.value = result.body;
    }
    if (result.apply !== undefined) {
      if (result.apply.focus) textarea.current?.focus();
      textarea.current?.setSelectionRange(result.apply.selection.start, result.apply.selection.end);
    }
    current.current = result.state;
    setDom(result.state);
    if (result.input !== undefined) onEvent(result.input);
  };
  useLayoutEffect(() => { dispatch({ type: "render", view }); }, [view]);
  useLayoutEffect(() => {
    const request = dom.request;
    if (request === null) return;
    const frame = requestAnimationFrame(() => dispatch({ type: "frame", directive: request.directive, revision: request.revision }));
    return () => cancelAnimationFrame(frame);
  }, [dom.request]);
  useLayoutEffect(() => { if (textarea.current !== null) resizeComposerTextarea(textarea.current); }, [dom.message]);
  const value = (input: HTMLTextAreaElement) => ({ text: input.value, selection: { start: input.selectionStart, end: input.selectionEnd } });
  const sendHint = t("composer.sendHint", { key: formatShortcutLabel(sendKey === "cmdEnter" ? "CommandOrControl+Enter" : "Enter") });
  const placeholder = microphoneShortcut === undefined || microphoneShortcut === null
    ? t("composer.speakToPlaceholder", { name: displayName })
    : t("composer.speakToPlaceholderWithVoice", { name: displayName, key: formatShortcutLabel(microphoneShortcut) });
  return <form className="composer" onSubmit={(event) => { event.preventDefault(); if (textarea.current !== null) dispatch({ type: "submit", ...value(textarea.current) }); }}>
    <button className={`microphone-button${view.recording ? " is-recording" : ""}`} type="button" disabled={!view.ready}
      aria-label={view.recording ? t("composer.stopSpeech") : t("composer.startSpeech")}
      onPointerDown={(event) => { event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); onEvent({ type: "microphone", gesture: "press" }); }}
      onPointerUp={(event) => { if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); onEvent({ type: "microphone", gesture: "release" }); }}
      onPointerCancel={() => onEvent({ type: "microphone", gesture: "cancel" })}
      onClick={() => onEvent({ type: "microphone", gesture: "click" })}
    ><svg viewBox="0 0 24 24" aria-hidden="true"><rect x="8.5" y="3" width="7" height="12" rx="3.5" /><path d="M6 11.5a6 6 0 0 0 12 0M12 17.5V21M8.5 21h7" /></svg></button>
    <textarea ref={textarea} rows={1} value={dom.message} aria-label={t("composer.speakTo", { name: displayName })}
      placeholder={view.ready ? placeholder : t("composer.preparing")} disabled={!view.ready}
      onChange={(event) => { resizeComposerTextarea(event.currentTarget); dispatch({ type: "edit", ...value(event.currentTarget) }); }}
      onSelect={(event) => dispatch({ type: "selection", selection: { start: event.currentTarget.selectionStart, end: event.currentTarget.selectionEnd } })}
      onCompositionStart={(event) => dispatch({ type: "composition", active: true, ...value(event.currentTarget) })}
      onCompositionEnd={(event) => dispatch({ type: "composition", active: false, ...value(event.currentTarget) })}
      onKeyDown={(event) => {
        const key = { key: event.key, metaKey: event.metaKey, shiftKey: event.shiftKey, ctrlKey: event.ctrlKey, altKey: event.altKey };
        if (!event.nativeEvent.isComposing && event.keyCode !== 229 && view.bindings.some((binding) => binding.key === key.key && binding.metaKey === key.metaKey && binding.shiftKey === key.shiftKey && binding.ctrlKey === key.ctrlKey && binding.altKey === key.altKey)) event.preventDefault();
        event.stopPropagation();
        dispatch({ type: "key", ...key, composing: event.nativeEvent.isComposing, keyCode: event.keyCode, ...value(event.currentTarget) });
      }} onFocus={onFocus} onBlur={onBlur} />
    <span className="send-key-hint">{sendHint}</span>
    <button className="send-button" type="submit" disabled={!view.canSend} aria-label={t("composer.sendLabel")} title={sendHint}>
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m4 5 16 7-16 7 3-7-3-7Zm3 7h13" /></svg>
    </button>
    {view.error === null ? null : <p role="alert">{view.error}</p>}
  </form>;
}
