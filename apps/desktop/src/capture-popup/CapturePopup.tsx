import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent, type ReactElement } from "react";

import { capturePopupApi } from "../ipc.js";
import {
  applyCaptureSnapshot,
  applySnapshotEvent,
  captureHasSendableAttachment,
  expectCapture,
  initialCapturePopupState,
} from "./state.js";
import { composerKeyAction } from "../view-model.js";
import { applyAppearance } from "../appearance.js";
import { AvatarBlob } from "../components/AvatarBlob.js";

export function CapturePopup(): ReactElement {
  const [viewState, setViewState] = useState(initialCapturePopupState);
  const snapshot = viewState.snapshot;
  const [message, setMessage] = useState("");
  const [error, setError] = useState<string>();
  const [sending, setSending] = useState(false);
  const input = useRef<HTMLTextAreaElement>(null);
  const canSend = captureHasSendableAttachment(snapshot);
  useEffect(() => {
    if (snapshot !== undefined) applyAppearance({ theme: snapshot.theme, font: snapshot.font });
  }, [snapshot?.font, snapshot?.theme]);
  const preview = useMemo(() => {
    if (snapshot === undefined) return undefined;
    if (snapshot.png === undefined) return undefined;
    const bytes = new Uint8Array(snapshot.png);
    return URL.createObjectURL(new Blob([bytes.buffer as ArrayBuffer], { type: "image/png" }));
  }, [snapshot]);

  const loadSnapshot = useCallback((captureId?: string): void => {
    setError(undefined);
    setSending(false);
    setMessage("");
    if (captureId !== undefined) {
      setViewState((current) => expectCapture(current, captureId));
    }
    void capturePopupApi.getSnapshot().then((result) => {
      if (result.ok) {
        setViewState((current) => applyCaptureSnapshot(current, result.value));
      } else {
        setError(result.error.message);
      }
    });
  }, []);
  useEffect(() => {
    if (snapshot === undefined) return;
    const frame = requestAnimationFrame(() => focusCapturePopupInput(input.current));
    return () => cancelAnimationFrame(frame);
  }, [snapshot?.captureId]);
  useEffect(() => {
    const subscription = capturePopupApi.subscribeReady((captureId) => loadSnapshot(captureId));
    const snapshots = capturePopupApi.subscribeSnapshots((event) => {
      setViewState((current) => applySnapshotEvent(current, event));
    });
    void Promise.all([subscription.ready, snapshots.ready]).then(() => loadSnapshot());
    return () => {
      subscription.dispose();
      snapshots.dispose();
    };
  }, [loadSnapshot]);
  useEffect(() => () => { if (preview !== undefined) URL.revokeObjectURL(preview); }, [preview]);
  useEffect(() => {
    const keydown = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        event.preventDefault();
        void capturePopupApi.cancel();
      }
    };
    window.addEventListener("keydown", keydown);
    return () => window.removeEventListener("keydown", keydown);
  }, []);

  const sendMessage = async (value: string): Promise<void> => {
    if (snapshot === undefined || !canSend || sending) return;
    setSending(true);
    setError(undefined);
    const result = await capturePopupApi.send(snapshot.captureId, value);
    if (!result.ok) {
      setError(result.error.message);
      setSending(false);
    }
  };
  const submit = (event: FormEvent): void => {
    event.preventDefault();
    void sendMessage(message);
  };
  const openAccessibilitySettings = async (): Promise<void> => {
    const result = await capturePopupApi.openAccessibilitySettings();
    if (!result.ok) setError(result.error.message);
  };
  const previewState = capturePreviewState(preview, error);

  return <main className="capture-shell">
    <section className="capture-card" aria-label="添付を送信">
      <header><AvatarBlob color={snapshot?.avatarColor} image={snapshot?.avatarImagePng} size={28} /><div><strong>{snapshot?.companionDisplayName ?? "Coo"}</strong><span>に{snapshot?.attachmentKind === "text" ? "文章" : "この画面"}を見せる</span></div></header>
      {snapshot?.attachmentKind === "text"
        ? <div className="text-attachment-preview"><pre>{snapshot.textPreview}</pre>{snapshot.textTruncated ? <small>末尾を切りました（{snapshot.textTruncatedCharacters} 文字）</small> : snapshot.textPreviewTruncated ? <small>先頭 2,000 文字を表示しています。送信時は全文を添付します。</small> : null}</div>
        : previewState !== "ready" ? <div className="capture-loading">{previewState === "loading" ? "画像を準備しています…" : <><span>画像を読み込めませんでした。</span><button type="button" onClick={() => loadSnapshot(snapshot?.captureId)}>もう一度読み込む</button></>}</div> : <img className="capture-preview" src={preview ?? ""} alt="選択した画面" />}
      {snapshot?.accessibilityPermissionRequired === true ? <div className="capture-accessibility-notice" role="status"><span>選択した文章を自動で使うには、アクセシビリティの許可が必要です</span><button type="button" className="secondary" disabled={sending} onClick={() => void openAccessibilitySettings()}>設定を開く</button></div> : null}
      {snapshot === undefined || snapshot.quickActions.length === 0 ? null : <div className="quick-actions" aria-label="定型文">{snapshot.quickActions.map((action, index) => <button type="button" key={`${action.label}-${index}`} disabled={!canSend || sending} onClick={() => void sendMessage(combineQuickAction(action.message, message))}>{action.label}</button>)}</div>}
      <form onSubmit={submit}>
        <textarea
          ref={input}
          value={message}
          maxLength={32_768}
          placeholder="ひとこと（なくても送れます）"
          aria-label="添えるひとこと"
          onChange={(event) => setMessage(event.target.value)}
          onKeyDown={(event) => {
            if (snapshot === undefined || !canSend) return;
            const action = composerKeyAction(event.key, event.metaKey, event.shiftKey, event.nativeEvent.isComposing, event.keyCode, snapshot.sendKey);
            if (action === "send") {
              event.preventDefault();
              void sendMessage(message);
            }
          }}
        />
        <div className="capture-actions">
          <button type="button" className="secondary" onClick={() => void capturePopupApi.cancel()}>取り消し</button>
          <button type="submit" disabled={!canSend || sending}>{sending ? "送信中…" : "送信"}</button>
        </div>
      </form>
      {error === undefined ? null : <p className="capture-error" role="alert">{error}</p>}
    </section>
  </main>;
}

export function combineQuickAction(action: string, typed: string): string {
  return typed === "" ? action : `${action}\n${typed}`;
}

export function focusCapturePopupInput(input: Pick<HTMLTextAreaElement, "focus"> | null): void {
  input?.focus();
}

export function capturePreviewState(preview?: string, error?: string): "loading" | "failed" | "ready" {
  if (preview !== undefined) return "ready";
  return error === undefined ? "loading" : "failed";
}
