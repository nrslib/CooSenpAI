import { useEffect, useMemo, useRef, useState, type FormEvent, type ReactElement } from "react";

import { Composer } from "./Composer.js";
import { desktopApi } from "../ipc.js";
import { AttachmentImage } from "./AttachmentImage.js";
import { DebugDrawer } from "./DebugDrawer.js";
import { composerHistoryDateKey, userMessageHistory } from "../composer-history.js";
import type { AppSnapshot, DebugDetail } from "../types.js";
import { tutorialChatInputReady } from "../tutorial-ui.js";
import { attachmentFailureState, attachmentTextView, conversationDate, conversationThinking, didUserScrollUp, findDebugDetail, formatTime, shouldFollowConversation, shouldRevealNewestTutorialNotice, unreadBoundaryIndex, type ConversationScrollCause } from "../view-model.js";

interface Selection {
  readonly id: string;
  readonly request: number;
}

interface Props {
  readonly snapshot: AppSnapshot;
  readonly selection?: Selection;
  readonly onSend: (message: string) => Promise<boolean>;
  readonly onInputActive: (active: boolean) => void;
  readonly onRead: () => void;
  readonly onCancel: () => Promise<void>;
  readonly onRetryAttachment: () => Promise<void>;
}

export function Conversation({ snapshot, selection, onSend, onInputActive, onRead, onCancel, onRetryAttachment }: Props): ReactElement {
  const [message, setMessage] = useState("");
  const [pendingSends, setPendingSends] = useState(0);
  const [scrollRequest, setScrollRequest] = useState(0);
  const [panelRequest, setPanelRequest] = useState(0);
  const [detail, setDetail] = useState<DebugDetail>();
  const listRef = useRef<HTMLDivElement>(null);
  const userScrolledUp = useRef(false);
  const programmaticScroll = useRef(false);
  const previousScrollTop = useRef(0);
  const entryElements = useRef(new Map<string, HTMLDivElement>());
  const previousNewestId = useRef<string | undefined>(undefined);
  const previousSelectionRequest = useRef(0);
  const previousScrollRequest = useRef(0);
  const previousPanelRequest = useRef(0);
  const previousThinkingLayout = useRef<string | undefined>(undefined);
  const draft = useRef("");
  const draftRevision = useRef(0);
  const newestId = snapshot.conversation.at(-1)?.id;
  const newestEntry = snapshot.conversation.at(-1);
  const revealNewestTutorialNotice = shouldRevealNewestTutorialNotice(newestEntry);
  const selectedId = selection?.id;
  const unreadIndex = unreadBoundaryIndex(snapshot.conversation, snapshot.unreadCount);
  const respondedUserIds = new Set(snapshot.conversation.flatMap((entry) => entry.role === "companion" ? entry.causedByIds ?? [] : []));
  const historyNow = new Date();
  const historyDateKey = composerHistoryDateKey(historyNow);
  const history = useMemo(
    () => userMessageHistory(snapshot.conversation, historyNow, snapshot.revision),
    [snapshot.conversation, snapshot.revision, historyDateKey],
  );
  const composerReady = tutorialChatInputReady(snapshot);
  const thinking = conversationThinking(snapshot, pendingSends > 0);
  const thinkingLayout = thinking ? snapshot.companionDraft ?? "" : undefined;

  useEffect(() => {
    const opened = (): void => setPanelRequest((value) => value + 1);
    window.addEventListener("focus", opened);
    document.addEventListener("visibilitychange", opened);
    return () => {
      window.removeEventListener("focus", opened);
      document.removeEventListener("visibilitychange", opened);
    };
  }, []);

  useEffect(() => {
    const cancel = (event: globalThis.KeyboardEvent): void => {
      if (event.key === "Escape" && snapshot.activeUserMessageId !== undefined) {
        event.preventDefault();
        void onCancel();
      }
    };
    window.addEventListener("keydown", cancel);
    return () => window.removeEventListener("keydown", cancel);
  }, [onCancel, snapshot.activeUserMessageId]);

  useEffect(() => {
    const selectionChanged = selection !== undefined && selection.request !== previousSelectionRequest.current;
    const sendChanged = scrollRequest !== previousScrollRequest.current;
    const panelChanged = panelRequest !== previousPanelRequest.current;
    const thinkingChanged = thinkingLayout !== undefined && thinkingLayout !== previousThinkingLayout.current;
    const cause: ConversationScrollCause = selectionChanged
      ? "selection"
      : previousNewestId.current === undefined
        ? "panel-open"
        : previousNewestId.current !== newestId
          ? "new-entry"
          : sendChanged
            ? "user-send"
            : panelChanged
              ? "panel-open"
              : thinkingChanged
                ? "thinking"
                : "layout";
    previousNewestId.current = newestId;
    previousSelectionRequest.current = selection?.request ?? 0;
    previousScrollRequest.current = scrollRequest;
    previousPanelRequest.current = panelRequest;
    previousThinkingLayout.current = thinkingLayout;
    if (!shouldFollowConversation(cause, userScrolledUp.current)) return;
    userScrolledUp.current = false;
    programmaticScroll.current = true;
    const frame = requestAnimationFrame(() => {
      const list = listRef.current;
      if (cause === "selection" && selectedId !== undefined && list !== null) {
        const entry = entryElements.current.get(selectedId);
        if (entry !== undefined) {
          list.scrollTop = entry.offsetTop - list.offsetTop - Math.max(0, (list.clientHeight - entry.clientHeight) / 2);
        }
      } else if (list !== null && revealNewestTutorialNotice) {
        const entry = newestId === undefined ? undefined : entryElements.current.get(newestId);
        list.scrollTop = entry === undefined ? list.scrollHeight : entry.offsetTop - list.offsetTop;
      } else if (list !== null) {
        list.scrollTop = list.scrollHeight;
      }
      previousScrollTop.current = list?.scrollTop ?? 0;
      requestAnimationFrame(() => { programmaticScroll.current = false; });
    });
    return () => cancelAnimationFrame(frame);
  }, [newestId, panelRequest, revealNewestTutorialNotice, scrollRequest, selectedId, selection?.request, thinkingLayout]);

  const submit = async (event: FormEvent): Promise<void> => {
    event.preventDefault();
    if (!composerReady || message.length === 0) return;
    const current = message;
    const activeInput = document.activeElement instanceof HTMLTextAreaElement
      ? document.activeElement
      : undefined;
    const selection = failedSendSelection(
      current,
      activeInput?.selectionStart,
      activeInput?.selectionEnd,
    );
    draft.current = "";
    const clearedRevision = ++draftRevision.current;
    setMessage("");
    userScrolledUp.current = false;
    setScrollRequest((value) => value + 1);
    setPendingSends((value) => value + 1);
    try {
      const accepted = await onSend(current);
      if (!accepted && shouldRestoreFailedSend(draft.current, draftRevision.current, clearedRevision)) {
        draft.current = current;
        ++draftRevision.current;
        setMessage(current);
        requestAnimationFrame(() => {
          const input = document.querySelector<HTMLTextAreaElement>(".composer textarea");
          input?.focus();
          input?.setSelectionRange(selection.start, selection.end);
        });
      }
    } finally {
      setPendingSends((value) => Math.max(0, value - 1));
    }
  };

  return <section className="conversation" aria-label="会話">
    <div
      className="conversation-list"
      ref={listRef}
      onWheel={(event) => { if (event.deltaY < 0) userScrolledUp.current = true; }}
      onScroll={(event) => {
        const top = event.currentTarget.scrollTop;
        if (didUserScrollUp(previousScrollTop.current, top, programmaticScroll.current)) {
          userScrolledUp.current = true;
        }
        previousScrollTop.current = top;
      }}
    >
      {snapshot.conversation.length === 0 ? <div className="conversation-empty"><p>まだ会話はありません。</p><span>{snapshot.companionDisplayName}に話しかけてみてください。</span></div> : null}
      {snapshot.conversation.map((entry, index) => {
        const previous = snapshot.conversation[index - 1];
        const showDate = previous === undefined || conversationDate(previous.createdAt) !== conversationDate(entry.createdAt);
        const sourceIds = entry.role === "user"
          ? [entry.id]
          : entry.causedByIds ?? [];
        const entryDetail = findDebugDetail(snapshot.debugCatalog, sourceIds);
        const attachmentFailure = snapshot.lastError?.attachmentOcr?.inputId === entry.id
          ? snapshot.lastError.attachmentOcr
          : undefined;
        const attachmentState = attachmentFailure === undefined
          ? undefined
          : attachmentFailureState(attachmentFailure);
        const attachedText = entry.attachmentText === undefined
          ? undefined
          : attachmentTextView(entry.attachmentText);
        return <div className="conversation-block" key={entry.id}>
          {showDate ? <div className="date-divider"><span>{conversationDate(entry.createdAt)}</span></div> : null}
          {unreadIndex === index ? <div className="unread-divider"><span>ここから未読</span></div> : null}
          <div
            ref={(element) => {
              if (element === null) entryElements.current.delete(entry.id);
              else entryElements.current.set(entry.id, element);
            }}
            className={`message-row role-${entry.role}${selectedId === entry.id ? " selected" : ""}`}
          >
            <div className="message-meta">
              <span>{entry.role === "user" ? "あなた" : snapshot.companionDisplayName}</span>
              <time dateTime={entry.createdAt}>{formatTime(entry.createdAt)}</time>
            </div>
            <div className={`message-bubble kind-${entry.role === "companion" ? entry.notificationPriority : "user"}`}>
              {entry.attachmentPath === undefined ? null : <AttachmentImage path={entry.attachmentPath} />}
              {attachedText === undefined ? null : <blockquote className="attachment-text-preview">{attachedText.preview}{attachedText.previewTruncated ? "…" : ""}{attachedText.truncationNotice === undefined ? null : <small>{attachedText.truncationNotice}</small>}</blockquote>}
              <p>{entry.message}</p>
              {snapshot.config.debug.enabled && entryDetail !== undefined
                ? <button className="detail-link" type="button" onClick={() => setDetail(entryDetail)}>詳細</button>
                : null}
            </div>
            {entry.role === "user" && attachmentState !== undefined
              ? <div className="message-state">{attachmentState.message}{attachmentState.terminal ? <><button type="button" onClick={() => void onCancel()}>取り消す</button><button type="button" onClick={() => void onRetryAttachment()}>もう一度試す</button></> : null}</div>
              : entry.role === "user" && snapshot.cancelledUserMessageIds.includes(entry.id)
              ? <div className="message-state">取り消しました <button type="button" onClick={() => void onSend(entry.message)}>{cancelledRetryLabel(entry.attachmentPath !== undefined || entry.attachmentText !== undefined)}</button></div>
              : entry.role === "user" && !respondedUserIds.has(entry.id)
                ? snapshot.config.chat.whileThinking === "append" && snapshot.activeUserMessageId !== undefined && snapshot.activeUserMessageId !== entry.id
                  ? null
                  : <div className="message-state">{snapshot.activeUserMessageId === entry.id ? "考えています…" : "順番待ち"}</div>
                : null}
          </div>
        </div>;
      })}
      {thinking ? <div className="thinking-row" aria-live="polite">{snapshot.companionDraft === undefined || snapshot.companionDraft.length === 0 ? <span className="thinking-dots"><i /><i /><i /></span> : <span className="streaming-draft">{snapshot.companionDraft}</span>}<span>{snapshot.companionDraft === undefined || snapshot.companionDraft.length === 0 ? `${snapshot.companionDisplayName}が考えています…` : ""}</span></div> : null}
    </div>
    <Composer
      displayName={snapshot.companionDisplayName}
      ready={composerReady}
      sending={pendingSends > 0}
      sendKey={snapshot.config.keymap.sendKey}
      message={message}
      history={history}
      onMessage={(value) => {
        draft.current = value;
        ++draftRevision.current;
        setMessage(value);
      }}
      onSubmit={(event) => void submit(event)}
      onFocus={() => { onInputActive(true); onRead(); }}
      onBlur={() => onInputActive(false)}
      onCancel={() => void onCancel()}
      speech={snapshot.speech}
      speechMode={snapshot.config.speech.mode}
      onSpeechStart={async () => { await desktopApi.startSpeech(); }}
      onSpeechFinish={async () => { await desktopApi.finishSpeech(); }}
      onSpeechCancel={async () => { await desktopApi.cancelSpeech(); }}
    />
    {detail === undefined ? null : <DebugDrawer detail={detail} close={() => setDetail(undefined)} />}
  </section>;
}

export function failedSendSelection(message: string, start?: number | null, end?: number | null): { readonly start: number; readonly end: number } {
  const fallback = message.length;
  const clampedStart = Math.min(Math.max(start ?? fallback, 0), fallback);
  const clampedEnd = Math.min(Math.max(end ?? clampedStart, clampedStart), fallback);
  return { start: clampedStart, end: clampedEnd };
}

export function shouldRestoreFailedSend(
  currentDraft: string,
  currentRevision: number,
  clearedRevision: number,
): boolean {
  return currentDraft.length === 0 && currentRevision === clearedRevision;
}

export function cancelledRetryLabel(hasAttachment: boolean): string {
  return hasAttachment ? "文だけ再送" : "再送";
}
