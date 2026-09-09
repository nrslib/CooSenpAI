import { useEffect, useLayoutEffect, useRef, useState, type ReactElement } from "react";

import type { BubbleRecord } from "../types.js";
import type { BubbleControlsView, BubbleViewInput } from "./view.js";
import { AvatarBlob } from "../components/AvatarBlob.js";
import { CloseIcon } from "../components/LineIcons.js";
import { useI18n } from "../i18n/index.js";

interface Props {
  readonly record: BubbleRecord;
  readonly revealed?: number | null;
  readonly avatarColor?: string;
  readonly avatarImage?: readonly number[];
  readonly exiting: boolean;
  readonly reading: boolean;
  readonly onHover: (hovering: boolean) => void;
  readonly controls: BubbleControlsView;
  readonly onEvent: (event: BubbleViewInput) => void;
}

export function Bubble({ record, revealed, avatarColor, avatarImage, exiting, reading, onHover, controls, onEvent }: Props): ReactElement {
  const [selected, setSelected] = useState(record.interaction?.select?.selected ?? "");
  const [secretDraft, setSecretDraft] = useState("");
  const article = useRef<HTMLElement>(null);
  const lastFocus = useRef(controls.focusRequest);
  const sending = controls.busy;
  useLayoutEffect(() => {
    if (controls.focusRequest !== lastFocus.current) article.current?.focus({ preventScroll: true });
    lastFocus.current = controls.focusRequest;
  }, [controls.focusRequest]);
  useEffect(() => setSecretDraft(""), [controls.clearSecret]);
  const hovered = useRef(false);
  const hoverCallback = useRef(onHover);
  hoverCallback.current = onHover;
  useEffect(() => () => {
    if (hovered.current) hoverCallback.current(false);
  }, []);
  const { locale, t } = useI18n();
  useEffect(() => {
    setSelected(record.interaction?.select?.selected ?? "");
    setSecretDraft("");
  }, [record.id, record.interaction?.select?.selected, record.interaction?.secretInput?.action]);
  const interactive = record.interaction !== undefined;
  const dismissible = controls.dismissible;
  const message = formatBubbleMessage(record.messageKind, record.message);
  const typedMessage = revealed == null || !reading || exiting || window.matchMedia("(prefers-reduced-motion: reduce)").matches ? message : Array.from(message).slice(0, revealed).join("");
  const typing = typedMessage !== message;
  const thought = record.messageKind === "thought";
  return <article
    className={`speech-item priority-${record.notificationPriority}${typing ? " is-typing" : ""}${thought ? " is-thought" : ""}${exiting ? " is-exiting" : ""}`}
    inert={exiting}
    tabIndex={-1}
    onMouseEnter={() => { hovered.current = true; onHover(true); }}
    onMouseLeave={() => { hovered.current = false; onHover(false); }}
    ref={article}
    onPointerDown={(event) => onEvent({ type: "pointer", id: record.id, control: isControl(event.target) })}
    onKeyDown={(event) => {
      if (["Escape", "ArrowUp", "ArrowDown", "End"].includes(event.key)) event.preventDefault();
      onEvent({ type: "key", id: record.id, key: event.key, composing: event.nativeEvent.isComposing, keyCode: event.nativeEvent.keyCode, control: isControl(event.target) });
    }}
  >
    <div className="speaker"><span>{record.displayName}</span><AvatarBlob color={avatarColor} image={avatarImage} size={24} /></div>
    <div className={`speech-bubble${interactive ? " is-interactive" : ""}`} role={controls.bodyButton ? "button" : undefined} tabIndex={controls.bodyButton ? 0 : undefined} onClick={(event) => onEvent({ type: "body", id: record.id, control: isControl(event.target) })}>
      <svg className="bubble-tail" viewBox="0 0 34 24" aria-hidden="true"><path d="M1 23c9-1 14-6 17-13 2-5 7-8 15-9-5 5-7 11-5 19-9-3-18-2-27 3Z" /></svg>
      {dismissible ? <button className="bubble-close" type="button" aria-label={t("common.close")} onClick={(event) => { event.stopPropagation(); onEvent({ type: "dismiss", id: record.id }); }}><CloseIcon /></button> : null}
      <p><span>{typedMessage}{typing ? <span className="typing-caret" aria-hidden="true">▏</span> : null}</span></p>
      {record.interaction?.detail === undefined ? null : <small className="interaction-detail">{record.interaction.detail}</small>}
      {record.interaction?.technicalDetail === undefined ? null : <details className="interaction-technical"><summary>{t("bubble.details")}</summary><small>{record.interaction.technicalDetail}</small></details>}
      {controls.requiredInput ? <small className="interaction-error" role="alert">{t("bubble.apiKeyRequired")}</small> : controls.error === null ? null : <small className="interaction-error" role="alert">{controls.error}</small>}
      {record.interaction?.select === undefined ? null : <div className="bubble-select"><select aria-label={t("bubble.options")} value={selected} disabled={sending} onChange={(event) => setSelected(event.target.value)}>{record.interaction.select.options.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select><button type="button" disabled={sending} onClick={() => { onEvent({ type: "select", id: record.id, value: selected }); }}>{record.interaction.select.confirmLabel}</button></div>}
      {record.interaction?.secretInput === undefined ? null : <div className="bubble-secret-input"><label htmlFor={`bubble-secret-input-${record.id}`}>{record.interaction.secretInput.label}</label><input id={`bubble-secret-input-${record.id}`} type="password" autoComplete="new-password" spellCheck={false} value={secretDraft} placeholder={record.interaction.secretInput.placeholder} disabled={sending} onChange={(event) => setSecretDraft(event.target.value)} /><button type="button" disabled={sending} onClick={() => { onEvent({ type: "secret", id: record.id, value: secretDraft }); }}>{record.interaction.secretInput.submitLabel}</button></div>}
      {record.interaction?.actions.length ? <div className="tutorial-actions">{record.interaction.actions.map((action) => <button key={action.id} type="button" disabled={sending} onClick={() => { onEvent({ type: "action", id: record.id, action: action.id }); }}>{action.label}</button>)}</div> : null}
      <time dateTime={record.createdAt}>{formatTime(record.createdAt, locale)}</time>
    </div>
  </article>;
}

function isControl(target: EventTarget): boolean {
  return target instanceof HTMLElement && target.closest("button,input,select,summary") !== null;
}

export function formatBubbleMessage(messageKind: string, message: string): string {
  if (messageKind !== "tutorial") return message;
  return message.replace(/。(?=[^\n])/g, "。\n");
}

function formatTime(value: string, locale: "ja" | "en"): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime())
    ? ""
    : date.toLocaleTimeString(locale === "ja" ? "ja-JP" : "en-US", { hour: "2-digit", minute: "2-digit" });
}
