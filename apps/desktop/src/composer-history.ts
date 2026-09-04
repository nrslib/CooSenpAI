import type { ConversationEntry } from "./types.js";

const textEncoder = new TextEncoder();

export interface ComposerHistorySelection {
  readonly start: number;
  readonly end: number;
}

export const COMPOSER_HISTORY_MAX_ENTRIES = 50;
export const COMPOSER_HISTORY_MAX_BYTES = 32 * 1024;

export interface ComposerHistoryEntry {
  readonly id: string;
  readonly message: string;
}

export interface ComposerHistory {
  readonly entries: readonly ComposerHistoryEntry[];
  readonly revision: number;
  readonly dateKey: string | undefined;
  readonly byteLength: number;
}

export type ComposerHistoryState =
  | { readonly mode: "normal" }
  | {
      readonly mode: "history";
      readonly position: number;
      readonly selectedId: string;
      readonly draft: string;
      readonly draftSelection: ComposerHistorySelection;
    };

export type ComposerHistoryKeyAction = "up" | "down" | "ignore";

export interface ComposerHistoryNavigation {
  readonly handled: boolean;
  readonly state: ComposerHistoryState;
  readonly value: string;
  readonly selection: ComposerHistorySelection;
}

export interface ComposerHistoryRebase {
  readonly state: ComposerHistoryState;
  readonly restoredDraft?: {
    readonly value: string;
    readonly selection: ComposerHistorySelection;
  };
}

export function createComposerHistoryState(): ComposerHistoryState {
  return { mode: "normal" };
}

export function userMessageHistory(
  entries: readonly ConversationEntry[],
  now: Date,
  revision: number,
): ComposerHistory {
  const today = localDateKey(now);
  if (today === undefined) return { entries: [], revision, dateKey: today, byteLength: 0 };

  let byteLength = 0;
  const history: ComposerHistoryEntry[] = [];
  for (let index = entries.length - 1; index >= 0; --index) {
    const entry = entries[index];
    if (entry === undefined) continue;
    if (entry.role !== "user" || entry.message.length === 0 || localDateKey(new Date(entry.createdAt)) !== today) continue;
    if (history.length >= COMPOSER_HISTORY_MAX_ENTRIES) break;
    const messageBytes = textEncoder.encode(entry.message).byteLength;
    if (byteLength + messageBytes > COMPOSER_HISTORY_MAX_BYTES) continue;
    history.push({ id: entry.id, message: entry.message });
    byteLength += messageBytes;
  }
  return { entries: history, revision, dateKey: today, byteLength };
}

export function composerHistoryDateKey(value: Date): string | undefined {
  return localDateKey(value);
}

export function rebaseComposerHistoryState(
  state: ComposerHistoryState,
  previousHistory: readonly ComposerHistoryEntry[],
  nextHistory: readonly ComposerHistoryEntry[],
): ComposerHistoryRebase {
  if (state.mode === "normal") return { state };
  const selected = previousHistory[state.position];
  if (selected === undefined) return restoreDraft(state);
  const position = nextHistory.findIndex((entry) => entry.id === selected.id);
  return position === -1 ? restoreDraft(state) : { state: { ...state, position } };
}

function restoreDraft(state: Extract<ComposerHistoryState, { mode: "history" }>): ComposerHistoryRebase {
  return {
    state: createComposerHistoryState(),
    restoredDraft: { value: state.draft, selection: state.draftSelection },
  };
}

export function composerHistoryKeyAction(
  key: string,
  value: string,
  selectionStart: number,
  selectionEnd: number,
  composing: boolean,
  keyCode: number,
): ComposerHistoryKeyAction {
  if (composing || keyCode === 229 || selectionStart !== selectionEnd) return "ignore";
  if (key === "ArrowUp" && !value.slice(0, selectionStart).includes("\n")) return "up";
  if (key === "ArrowDown" && !value.slice(selectionEnd).includes("\n")) return "down";
  return "ignore";
}

export function navigateComposerHistory(
  state: ComposerHistoryState,
  action: Exclude<ComposerHistoryKeyAction, "ignore">,
  currentValue: string,
  history: readonly ComposerHistoryEntry[],
  currentSelection: ComposerHistorySelection,
): ComposerHistoryNavigation {
  if (history.length === 0) {
    return {
      handled: false,
      state: createComposerHistoryState(),
      value: currentValue,
      selection: currentSelection,
    };
  }

  if (action === "up") {
    const position = state.mode === "normal"
      ? 0
      : Math.min(state.position + 1, history.length - 1);
    const entry = historyEntry(history, position);
    return {
      handled: true,
      state: state.mode === "normal"
        ? { mode: "history", position, selectedId: entry.id, draft: currentValue, draftSelection: currentSelection }
        : { ...state, position, selectedId: entry.id },
      value: entry.message,
      selection: endSelection(entry.message),
    };
  }

  if (state.mode === "normal") {
    return {
      handled: false,
      state,
      value: currentValue,
      selection: currentSelection,
    };
  }

  if (state.position === 0) {
    return {
      handled: true,
      state: createComposerHistoryState(),
      value: state.draft,
      selection: state.draftSelection,
    };
  }

  const position = state.position - 1;
  const entry = historyEntry(history, position);
  return {
    handled: true,
    state: { ...state, position, selectedId: entry.id },
    value: entry.message,
    selection: endSelection(entry.message),
  };
}

function localDateKey(value: Date): string | undefined {
  if (Number.isNaN(value.getTime())) return undefined;
  return `${value.getFullYear()}-${value.getMonth()}-${value.getDate()}`;
}

function endSelection(value: string): ComposerHistorySelection {
  return { start: value.length, end: value.length };
}

function historyEntry(history: readonly ComposerHistoryEntry[], position: number): ComposerHistoryEntry {
  const value = history[position];
  if (value === undefined) throw new Error("チャット履歴の位置が不正です");
  return value;
}
