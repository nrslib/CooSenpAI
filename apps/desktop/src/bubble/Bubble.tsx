import { memo, useEffect, useState, type ReactElement } from "react";

import type { BubbleRecord } from "../types.js";
import { actionRequest, selectedValue, selectRequest } from "./interaction.js";
import { AvatarBlob } from "../components/AvatarBlob.js";
import { CloseIcon } from "../components/LineIcons.js";

interface Props {
  readonly record: BubbleRecord;
  readonly avatarColor?: string;
  readonly avatarImage?: readonly number[];
  readonly exiting: boolean;
  readonly onHover: (hovering: boolean) => void;
  readonly onRequestFocus: () => void;
  readonly onDismiss: () => void;
  readonly onClick: () => void;
  readonly onInteract: (action: string, value?: string) => Promise<string | undefined>;
}

function BubbleView({ record, avatarColor, avatarImage, exiting, onHover, onRequestFocus, onDismiss, onClick, onInteract }: Props): ReactElement {
  const [selected, setSelected] = useState(selectedValue(record.interaction));
  const [sending, setSending] = useState(false);
  const [interactionError, setInteractionError] = useState<string>();
  useEffect(() => {
    setSelected(selectedValue(record.interaction));
    setSending(false);
    setInteractionError(undefined);
  }, [record.id, record.interaction?.select?.selected]);
  const interact = async (action: string, value?: string): Promise<void> => {
    if (sending) return;
    setSending(true);
    try {
      setInteractionError(await onInteract(action, value));
    } finally {
      setSending(false);
    }
  };
  const interactive = record.interaction !== undefined;
  const dismissible = bubbleIsManuallyDismissible(record.messageKind);
  const typing = record.messageKind === "tutorial-typing";
  const thought = record.messageKind === "thought";
  return <article
    className={`speech-item priority-${record.notificationPriority}${typing ? " is-typing" : ""}${thought ? " is-thought" : ""}${exiting ? " is-exiting" : ""}`}
    tabIndex={-1}
    onMouseEnter={() => onHover(true)}
    onMouseLeave={() => onHover(false)}
    onPointerDown={(event) => {
      event.currentTarget.focus({ preventScroll: true });
      onRequestFocus();
    }}
    onKeyDown={(event) => {
      if (!shouldDismissBubbleOnKey(
        record.messageKind,
        event.key,
        event.nativeEvent.isComposing,
        event.nativeEvent.keyCode,
      )) return;
      event.preventDefault();
      event.stopPropagation();
      onDismiss();
    }}
  >
    <div className="speaker"><span>{record.displayName}</span><AvatarBlob color={avatarColor} image={avatarImage} size={24} /></div>
    <div className={`speech-bubble${interactive ? " is-interactive" : ""}`} role={interactive ? undefined : "button"} tabIndex={interactive ? undefined : 0} onClick={interactive ? undefined : onClick} onKeyDown={interactive ? undefined : (event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onClick(); } }}>
      <svg className="bubble-tail" viewBox="0 0 34 24" aria-hidden="true"><path d="M1 23c9-1 14-6 17-13 2-5 7-8 15-9-5 5-7 11-5 19-9-3-18-2-27 3Z" /></svg>
      {dismissible ? <button className="bubble-close" type="button" aria-label="閉じる" onClick={(event) => { event.stopPropagation(); onDismiss(); }}><CloseIcon /></button> : null}
      <p aria-label={typing ? "次の案内を入力中" : undefined}>{formatBubbleMessage(record.messageKind, record.message)}</p>
      {record.interaction?.detail === undefined ? null : <small className="interaction-detail">{record.interaction.detail}</small>}
      {record.interaction?.technicalDetail === undefined ? null : <details className="interaction-technical"><summary>詳しい情報</summary><small>{record.interaction.technicalDetail}</small></details>}
      {interactionError === undefined ? null : <small className="interaction-error" role="alert">{interactionError}</small>}
      {record.interaction?.select === undefined ? null : <div className="bubble-select"><select aria-label="選択肢" value={selected} disabled={sending} onChange={(event) => setSelected(event.target.value)}>{record.interaction.select.options.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select><button type="button" disabled={sending} onClick={() => { const request = selectRequest(record.interaction!, selected); if (request !== undefined) void interact(request.action, request.value); }}>{record.interaction.select.confirmLabel}</button></div>}
      {record.interaction?.actions.length ? <div className="tutorial-actions">{record.interaction.actions.map((action) => <button key={action.id} type="button" disabled={sending} onClick={() => { const request = actionRequest(record.interaction!, action.id); if (request !== undefined) void interact(request.action); }}>{action.label}</button>)}</div> : null}
      <time dateTime={record.createdAt}>{formatTime(record.createdAt)}</time>
    </div>
  </article>;
}

export const Bubble = memo(BubbleView, (previous, next) => {
  return previous.record === next.record
    && previous.avatarColor === next.avatarColor
    && previous.avatarImage === next.avatarImage
    && previous.exiting === next.exiting;
});

export function shouldDismissBubbleOnKey(
  messageKind: string,
  key: string,
  isComposing: boolean,
  keyCode: number,
): boolean {
  return bubbleIsManuallyDismissible(messageKind)
    && key === "Escape"
    && !isComposing
    && keyCode !== 229;
}

export function bubbleIsManuallyDismissible(messageKind: string): boolean {
  return messageKind !== "tutorial" && messageKind !== "tutorial-typing";
}

export function formatBubbleMessage(messageKind: string, message: string): string {
  if (messageKind !== "tutorial") return message;
  return message.replace(/。(?=[^\n])/g, "。\n");
}

function formatTime(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? ""
    : date.toLocaleTimeString("ja-JP", { hour: "2-digit", minute: "2-digit" });
}
