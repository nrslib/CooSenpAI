import type { ReactElement } from "react";

import type { PopupQuickAction } from "../types.js";
import { AddIcon } from "./LineIcons.js";

interface Props {
  readonly title: string;
  readonly kind: "text" | "image";
  readonly actions: readonly PopupQuickAction[];
  readonly update: (actions: PopupQuickAction[]) => void;
  readonly errorFor: (path: string) => string | undefined;
}

export const MAX_QUICK_ACTIONS = 12;
const MAX_LABEL_CHARS = 40;
const MAX_MESSAGE_BYTES = 32_768;
const CONTROL_CHARACTER = /\p{Cc}/u;

type MoveDirection = "up" | "down";
type QuickActionField = "label" | "message";

export function moveQuickAction(
  actions: readonly PopupQuickAction[],
  index: number,
  direction: MoveDirection,
): PopupQuickAction[] {
  const targetIndex = direction === "up" ? index - 1 : index + 1;
  if (index < 0 || index >= actions.length || targetIndex < 0 || targetIndex >= actions.length) {
    return [...actions];
  }
  const next = [...actions];
  const current = next[index];
  const target = next[targetIndex];
  if (current === undefined || target === undefined) return next;
  next[index] = target;
  next[targetIndex] = current;
  return next;
}

function localFieldError(actions: readonly PopupQuickAction[], index: number, field: QuickActionField): string | undefined {
  const action = actions[index];
  if (action === undefined) return undefined;
  const value = action[field].trim();
  if (field === "label") {
    if (value.length === 0 || Array.from(value).length > MAX_LABEL_CHARS || CONTROL_CHARACTER.test(value)) {
      return `1以上${MAX_LABEL_CHARS}以下の表示文字列を指定してください。`;
    }
    if (actions.some((other, current) => current !== index && other.label.trim() === value)) {
      return "同じ種類の定型文内で表示ラベルを重複させないでください。";
    }
  } else if (value.length === 0 || new TextEncoder().encode(value).byteLength > MAX_MESSAGE_BYTES) {
    return `1以上${MAX_MESSAGE_BYTES}以下の UTF-8 byte で指定してください。`;
  }
  return undefined;
}

export function newQuickAction(actions: readonly PopupQuickAction[]): PopupQuickAction {
  const baseLabel = "新しい定型文";
  const labels = new Set(actions.map((action) => action.label.trim()));
  let label = baseLabel;
  let suffix = 2;
  while (labels.has(label)) {
    label = `${baseLabel} ${suffix}`;
    suffix += 1;
  }
  return { label, message: label };
}

export function QuickActionsEditor({ title, kind, actions, update, errorFor }: Props): ReactElement {
  const path = `popup.quickActions.${kind}`;
  const listError = errorFor(path) ?? (actions.length > MAX_QUICK_ACTIONS ? `${MAX_QUICK_ACTIONS}件以下で指定してください。` : undefined);
  const replace = (index: number, field: QuickActionField, value: string): void => {
    update(actions.map((action, current) => current === index
      ? { ...action, [field]: value }
      : action));
  };
  const move = (index: number, direction: MoveDirection): void => {
    update(moveQuickAction(actions, index, direction));
  };
  return <div className="quick-action-editor">
    <h3>{title}</h3>
    {listError === undefined ? null : <span className="field-error" role="alert">{listError}</span>}
    {actions.map((action, index) => {
      const labelPath = `${path}[${index}].label`;
      const messagePath = `${path}[${index}].message`;
      const labelError = errorFor(labelPath) ?? localFieldError(actions, index, "label");
      const messageError = errorFor(messagePath) ?? localFieldError(actions, index, "message");
      return <div className="quick-action-row" key={index}>
      <label><span>表示</span><input id={`setting-${labelPath.replaceAll(".", "-")}`} aria-invalid={labelError === undefined ? undefined : true} value={action.label} onChange={(event) => replace(index, "label", event.target.value)} />{labelError === undefined ? null : <span className="field-error" role="alert">{labelError}</span>}</label>
      <label><span>送る文</span><input id={`setting-${messagePath.replaceAll(".", "-")}`} aria-invalid={messageError === undefined ? undefined : true} value={action.message} onChange={(event) => replace(index, "message", event.target.value)} />{messageError === undefined ? null : <span className="field-error" role="alert">{messageError}</span>}</label>
      <span className="quick-action-order">
        <button type="button" aria-label={`${action.label || index + 1} を上へ`} disabled={index === 0} onClick={() => move(index, "up")}>↑</button>
        <button type="button" aria-label={`${action.label || index + 1} を下へ`} disabled={index === actions.length - 1} onClick={() => move(index, "down")}>↓</button>
      </span>
      <button type="button" aria-label={`${action.label || index + 1} を削除`} onClick={() => update(actions.filter((_, current) => current !== index))}>削除</button>
    </div>;
    })}
    <button type="button" disabled={actions.length >= MAX_QUICK_ACTIONS} onClick={() => update([...actions, newQuickAction(actions)])}><AddIcon /> 追加</button>
  </div>;
}
