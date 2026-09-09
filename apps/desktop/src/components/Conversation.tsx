import { useEffect, useRef, useState, type ReactElement } from "react";

import { WorkApproval } from "./WorkApproval.js";
import { Composer } from "./Composer.js";
import { desktopApi } from "../ipc.js";
import { AttachmentImage } from "./AttachmentImage.js";
import { DebugDrawer } from "./DebugDrawer.js";
import type { ComposerView, ConversationView, ConversationAction } from "../composer-view.js";
import type { AppSnapshot, DebugDetail } from "../types.js";
import { attachmentFailureState, attachmentTextView, conversationDate, findDebugDetail, formatTime, unreadBoundaryIndex } from "../view-model.js";
import { t, useI18n } from "../i18n/index.js";

interface Props {
  readonly snapshot: AppSnapshot;
  readonly onInputActive: (active: boolean) => void;
  readonly onRead: () => void;
}

export function Conversation({ snapshot, onInputActive, onRead }: Props): ReactElement {
  const { locale, t } = useI18n();
  const [composer, setComposer] = useState<ComposerView>();
  const [view, setView] = useState<ConversationView>();
  const [detail, setDetail] = useState<DebugDetail>();
  const listRef = useRef<HTMLDivElement>(null);
  const programmaticScroll = useRef(false);
  const previousScrollTop = useRef(0);
  const entryElements = useRef(new Map<string, HTMLDivElement>());
  const selectedId = view?.selectedId;
  const newestEntryId = snapshot.conversation.at(-1)?.id;
  const unreadIndex = unreadBoundaryIndex(snapshot.conversation, snapshot.unreadCount);
  const respondedUserIds = new Set(snapshot.conversation.flatMap((entry) => entry.role === "companion" ? entry.causedByIds ?? [] : []));
  const thinking = view?.thinking === true;
  const action = (inputId: string, action: ConversationAction): void => {
    if (view !== undefined) void desktopApi.conversationInput({ type: "action", action, inputId, generation: view.operationGeneration });
  };
  useEffect(() => {
    const composer = desktopApi.subscribeComposerView(setComposer);
    const conversation = desktopApi.subscribeConversationView(setView);
    void Promise.all([composer.ready, conversation.ready]).then(() => desktopApi.conversationInput({ type: "mounted" }));
    const opened = () => { void desktopApi.conversationInput({ type: "opened" }); };
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") void desktopApi.conversationInput({ type: "escape", composing: event.isComposing, keyCode: event.keyCode });
    };
    window.addEventListener("focus", opened);
    document.addEventListener("visibilitychange", opened);
    window.addEventListener("keydown", escape);
    return () => { composer.dispose(); conversation.dispose(); window.removeEventListener("focus", opened); document.removeEventListener("visibilitychange", opened); window.removeEventListener("keydown", escape); };
  }, []);
  useEffect(() => {
    if (view === undefined) return;
    programmaticScroll.current = true;
    const frame = requestAnimationFrame(() => {
      const list = listRef.current;
      if (list === null) return;
      const entry = view.entryId === null ? undefined : entryElements.current.get(view.entryId);
      if (view.target === "entry-center" && entry !== undefined) list.scrollTop = entry.offsetTop - list.offsetTop - Math.max(0, (list.clientHeight - entry.clientHeight) / 2);
      else if (view.target === "entry-start" && entry !== undefined) list.scrollTop = entry.offsetTop - list.offsetTop;
      else if (view.target === "bottom") list.scrollTop = list.scrollHeight;
      previousScrollTop.current = list.scrollTop;
      requestAnimationFrame(() => { programmaticScroll.current = false; });
    });
    return () => cancelAnimationFrame(frame);
  }, [view?.scrollRequest]);
  useEffect(() => {
    void desktopApi.conversationInput({ type: "layout" });
  }, [newestEntryId]);

  return <><section className="conversation" aria-label={t("conversation.label")}>
    <div
      className="conversation-list"
      ref={listRef}
      onLoadCapture={() => { void desktopApi.conversationInput({ type: "layout" }); }}
      onWheel={(event) => { void desktopApi.conversationInput({ type: "wheel", delta: event.deltaY }); }}
      onScroll={(event) => {
        const top = event.currentTarget.scrollTop;
        void desktopApi.conversationInput({ type: "scroll", previousTop: previousScrollTop.current, top, programmatic: programmaticScroll.current });
        previousScrollTop.current = top;
      }}
    >
      {snapshot.conversation.length === 0 ? <div className="conversation-empty"><p>{t("conversation.empty")}</p><span>{t("conversation.emptyPrompt", { name: snapshot.companionDisplayName })}</span></div> : null}
      {snapshot.conversation.map((entry, index) => {
        const previous = snapshot.conversation[index - 1];
        const showDate = previous === undefined || conversationDate(previous.createdAt, locale) !== conversationDate(entry.createdAt, locale);
        const sourceIds = entry.role === "user"
          ? [entry.id]
          : entry.causedByIds ?? [];
        const entryDetail = findDebugDetail(snapshot.debugCatalog, sourceIds);
        const attachmentFailure = snapshot.lastError?.attachmentOcr?.inputId === entry.id
          ? snapshot.lastError.attachmentOcr
          : undefined;
        const attachmentState = attachmentFailure === undefined
          ? undefined
          : attachmentFailureState(attachmentFailure, locale);
        const attachedText = entry.attachmentText === undefined
          ? undefined
          : attachmentTextView(entry.attachmentText, locale);
        return <div className="conversation-block" key={entry.id}>
          {showDate ? <div className="date-divider"><span>{conversationDate(entry.createdAt, locale)}</span></div> : null}
          {unreadIndex === index ? <div className="unread-divider"><span>{t("conversation.unread")}</span></div> : null}
          <div
            ref={(element) => {
              if (element === null) entryElements.current.delete(entry.id);
              else entryElements.current.set(entry.id, element);
            }}
            className={`message-row role-${entry.role}${selectedId === entry.id ? " selected" : ""}`}
          >
            <div className="message-meta">
              <span>{entry.role === "user" ? t("conversation.user") : snapshot.companionDisplayName}</span>
              <time dateTime={entry.createdAt}>{formatTime(entry.createdAt, locale)}</time>
            </div>
            <div className={`message-bubble kind-${entry.role === "companion" ? entry.notificationPriority : "user"}`}>
              {entry.attachmentPath === undefined ? null : <AttachmentImage path={entry.attachmentPath} />}
              {attachedText === undefined ? null : <blockquote className="attachment-text-preview">{attachedText.preview}{attachedText.previewTruncated ? "…" : ""}{attachedText.truncationNotice === undefined ? null : <small>{attachedText.truncationNotice}</small>}</blockquote>}
              <p>{entry.message}</p>
              {snapshot.config.debug.enabled && entryDetail !== undefined
                ? <button className="detail-link" type="button" onClick={() => setDetail(entryDetail)}>{t("conversation.details")}</button>
                : null}
            </div>
            {entry.role === "user" && attachmentState !== undefined
              ? <div className="message-state">{attachmentState.message}{attachmentState.terminal ? <><button type="button" disabled={!view?.actions[entry.id]?.cancel} onClick={() => action(entry.id, "cancel")}>{t("conversation.cancel")}</button><button type="button" disabled={!view?.actions[entry.id]?.retry} onClick={() => action(entry.id, "retry")}>{t("common.retry")}</button></> : null}</div>
              : entry.role === "user" && snapshot.cancelledUserMessageIds.includes(entry.id)
              ? <div className="message-state">{t("conversation.cancelled")} <button type="button" disabled={!view?.actions[entry.id]?.resend} onClick={() => action(entry.id, "resend")}>{cancelledRetryLabel(entry.attachmentPath !== undefined || entry.attachmentText !== undefined, locale)}</button></div>
              : entry.role === "user" && !respondedUserIds.has(entry.id)
                ? snapshot.config.chat.whileThinking === "append" && snapshot.activeUserMessageId !== undefined && snapshot.activeUserMessageId !== entry.id
                  ? null
                  : <div className="message-state">{snapshot.activeUserMessageId === entry.id ? t("conversation.thinking") : t("conversation.queued")}</div>
                : null}
          </div>
        </div>;
      })}
      {view?.workInputId != null ? <WorkApproval
        key={view.workInputId} inputId={view.workInputId}
        onLayoutChange={() => { void desktopApi.conversationInput({ type: "layout" }); }} /> : null}
      {thinking ? <div className="thinking-row" aria-live="polite">{snapshot.companionDraft === undefined || snapshot.companionDraft.length === 0 ? <span className="thinking-dots"><i /><i /><i /></span> : <span className="streaming-draft">{snapshot.companionDraft}</span>}<span>{snapshot.companionDraft === undefined || snapshot.companionDraft.length === 0 ? t("conversation.thinkingWithName", { name: snapshot.companionDisplayName }) : ""}</span></div> : null}
    </div>
    {view?.error == null ? null : <p role="alert">{view.error}</p>}
  </section>
    {composer === undefined ? null : <Composer displayName={snapshot.companionDisplayName} view={composer}
      sendKey={snapshot.config.keymap.sendKey} microphoneShortcut={snapshot.config.keymap.microphone}
      onEvent={(event) => { void desktopApi.composerInput(event); }}
      onFocus={() => { onInputActive(true); onRead(); }} onBlur={() => onInputActive(false)} />}

    {detail === undefined ? null : <DebugDrawer detail={detail} close={() => setDetail(undefined)} />}
  </>;
}

export function cancelledRetryLabel(hasAttachment: boolean, locale: "ja" | "en" = "ja"): string {
  return hasAttachment ? t(locale, "conversation.retryText") : t(locale, "conversation.retry");
}
