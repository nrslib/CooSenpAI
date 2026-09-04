import { useEffect, useRef, type FormEvent, type KeyboardEvent, type PointerEvent, type ReactElement } from "react";
import { insertSpeechAtSelection, speechTranscriptDisposition } from "../composer-speech.js";
import { composerHistoryKeyAction, createComposerHistoryState, navigateComposerHistory, rebaseComposerHistoryState, type ComposerHistory } from "../composer-history.js";
import { desktopApi } from "../ipc.js";
import { formatShortcutLabel } from "../shortcut-label.js";
import { composerKeyAction, microphoneAction } from "../view-model.js";
import type { SpeechView } from "../types.js";

interface Props {
  readonly displayName: string;
  readonly ready: boolean;
  readonly sending: boolean;
  readonly sendKey: "enter" | "cmdEnter";
  readonly message: string;
  readonly history: ComposerHistory;
  readonly onMessage: (value: string) => void;
  readonly onSubmit: (event: FormEvent) => void;
  readonly onFocus: () => void;
  readonly onBlur: () => void;
  readonly onCancel: () => void;
  readonly speech: SpeechView;
  readonly speechMode: "pushToTalk" | "toggle";
  readonly onSpeechStart: () => Promise<void>;
  readonly onSpeechFinish: () => Promise<void>;
  readonly onSpeechCancel: () => Promise<void>;
}

export function Composer({ displayName, ready, sending, sendKey, message, history, onMessage, onSubmit, onFocus, onBlur, onCancel, speech, speechMode, onSpeechStart, onSpeechFinish, onSpeechCancel }: Props): ReactElement {
  const heldMicrophone = useRef(false);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const messageValue = useRef(message);
  const expectedMessage = useRef(message);
  const selection = useRef({ start: message.length, end: message.length });
  const historyState = useRef(createComposerHistoryState());
  const previousHistory = useRef(history);
  const composing = useRef(false);
  const pendingTranscript = useRef<string | undefined>(undefined);
  const speechOperations = useRef(Promise.resolve());
  messageValue.current = message;

  const publishMessage = (value: string, nextSelection: { readonly start: number; readonly end: number }): void => {
    expectedMessage.current = value;
    messageValue.current = value;
    selection.current = nextSelection;
    onMessage(value);
    requestAnimationFrame(() => {
      textarea.current?.setSelectionRange(nextSelection.start, nextSelection.end);
    });
  };

  useEffect(() => {
    if (message !== expectedMessage.current) historyState.current = createComposerHistoryState();
    expectedMessage.current = message;
  }, [message]);

  useEffect(() => {
    const rebased = rebaseComposerHistoryState(
      historyState.current,
      previousHistory.current.entries,
      history.entries,
    );
    historyState.current = rebased.state;
    if (rebased.restoredDraft !== undefined) {
      publishMessage(rebased.restoredDraft.value, rebased.restoredDraft.selection);
    }
    previousHistory.current = history;
  }, [history.dateKey, history.revision]);

  const insertTranscript = (text: string): void => {
    const input = textarea.current;
    const start = input?.selectionStart ?? selection.current.start;
    const end = input?.selectionEnd ?? selection.current.end;
    const insertion = insertSpeechAtSelection(messageValue.current, start, end, text);
    historyState.current = createComposerHistoryState();
    publishMessage(insertion.value, { start: insertion.caret, end: insertion.caret });
  };

  useEffect(() => {
    const transcript = desktopApi.subscribeSpeechComposerFinal((text) => {
      if (speechTranscriptDisposition(composing.current) === "defer") {
        pendingTranscript.current = text;
      } else {
        insertTranscript(text);
      }
    });
    return () => transcript.dispose();
  }, [onMessage]);
  const submitOnEnter = (event: KeyboardEvent<HTMLTextAreaElement>): void => {
    if (event.key === "Escape") {
      if (composing.current || event.nativeEvent.isComposing) return;
      if (historyState.current.mode === "history") {
        event.preventDefault();
        event.stopPropagation();
        historyState.current = createComposerHistoryState();
        return;
      }
      if (sending) {
        event.preventDefault();
        onCancel();
      }
      return;
    }
    if (event.shiftKey || event.metaKey || event.ctrlKey || event.altKey) return;
    const input = event.currentTarget;
    const historyAction = composerHistoryKeyAction(
      event.key,
      input.value,
      input.selectionStart,
      input.selectionEnd,
      composing.current || event.nativeEvent.isComposing,
      event.keyCode,
    );
    if (historyAction !== "ignore") {
      const navigation = navigateComposerHistory(
        historyState.current,
        historyAction,
        input.value,
        history.entries,
        { start: input.selectionStart, end: input.selectionEnd },
      );
      if (navigation.handled) {
        event.preventDefault();
        historyState.current = navigation.state;
        publishMessage(navigation.value, navigation.selection);
      }
      return;
    }
    const action = composerKeyAction(event.key, event.metaKey, event.shiftKey, event.nativeEvent.isComposing, event.keyCode, sendKey);
    if (action !== "send") return;
    event.preventDefault();
    event.currentTarget.form?.requestSubmit();
  };
  const dispatchSpeech = (operation: () => Promise<void>): void => {
    speechOperations.current = speechOperations.current.then(operation, operation);
  };
  const recording = ["starting", "recording", "finalizing"].includes(speech.phase)
    && speech.source === "composer";
  const sendKeyLabel = formatShortcutLabel(sendKey === "cmdEnter" ? "CommandOrControl+Enter" : "Enter");
  const micPointerDown = (event: PointerEvent<HTMLButtonElement>): void => {
    const action = microphoneAction(speechMode, recording, "press");
    if (action !== "start") return;
    event.preventDefault();
    heldMicrophone.current = true;
    event.currentTarget.setPointerCapture(event.pointerId);
    dispatchSpeech(onSpeechStart);
  };
  const micPointerUp = (event: PointerEvent<HTMLButtonElement>): void => {
    if (speechMode !== "pushToTalk" || !heldMicrophone.current) return;
    event.preventDefault();
    heldMicrophone.current = false;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    dispatchSpeech(onSpeechFinish);
  };
  return <form className="composer" onSubmit={onSubmit}>
    <button
      className={`microphone-button${recording ? " is-recording" : ""}`}
      type="button"
      disabled={!ready}
      aria-label={recording ? "音声入力を停止" : "音声入力を開始"}
      onPointerDown={micPointerDown}
      onPointerUp={micPointerUp}
      onPointerCancel={() => { if (heldMicrophone.current) { heldMicrophone.current = false; dispatchSpeech(onSpeechCancel); } }}
      onClick={() => {
        const action = microphoneAction(speechMode, recording, "click");
        if (action === "finish") dispatchSpeech(onSpeechFinish);
        if (action === "start") dispatchSpeech(onSpeechStart);
      }}
    ><svg viewBox="0 0 24 24" aria-hidden="true"><rect x="8.5" y="3" width="7" height="12" rx="3.5" /><path d="M6 11.5a6 6 0 0 0 12 0M12 17.5V21M8.5 21h7" /></svg></button>
    <textarea
      ref={textarea}
      rows={1}
      value={message}
      aria-label={`${displayName}に話しかける`}
      placeholder={ready ? `${displayName}に話しかける…` : "準備中…"}
      disabled={!ready}
      onChange={(event) => {
        historyState.current = createComposerHistoryState();
        messageValue.current = event.target.value;
        expectedMessage.current = event.target.value;
        onMessage(event.target.value);
      }}
      onSelect={(event) => {
        selection.current = {
          start: event.currentTarget.selectionStart,
          end: event.currentTarget.selectionEnd,
        };
      }}
      onCompositionStart={() => { composing.current = true; }}
      onCompositionEnd={(event) => {
        composing.current = false;
        messageValue.current = event.currentTarget.value;
        selection.current = {
          start: event.currentTarget.selectionStart,
          end: event.currentTarget.selectionEnd,
        };
        const pending = pendingTranscript.current;
        pendingTranscript.current = undefined;
        if (pending !== undefined) requestAnimationFrame(() => insertTranscript(pending));
      }}
      onKeyDown={submitOnEnter}
      onFocus={onFocus}
      onBlur={onBlur}
    />
    <span className="send-key-hint">{sendKeyLabel} で送信</span>
    <button className="send-button" type="submit" disabled={!ready || message.length === 0} aria-label="送信">
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m4 5 16 7-16 7 3-7-3-7Zm3 7h13" /></svg>
    </button>
  </form>;
}
