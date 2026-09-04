import type { SpeechView } from "../types.js";

export function shouldInitializeConfirmationDraft(
  initializedGeneration: number | undefined,
  speech: SpeechView,
): boolean {
  return speech.phase === "confirming" && initializedGeneration !== speech.generation;
}

export interface SpeechPopupTransientError {
  readonly id: number;
  readonly generation: number | undefined;
  readonly message: string;
}

export function shouldKeepSpeechPopupError(
  error: SpeechPopupTransientError | undefined,
  speech: SpeechView,
): boolean {
  return error !== undefined
    && speech.message === undefined
    && speech.phase === "confirming"
    && error.generation === speech.generation;
}

export function isSpeechPopupErrorCurrent(
  error: SpeechPopupTransientError | undefined,
  expectedId: number,
): boolean {
  return error?.id === expectedId;
}

export function speechPopupErrorMessage(
  speech: SpeechView | undefined,
  transientError: SpeechPopupTransientError | undefined,
  loadError: string | undefined,
): string | undefined {
  if (speech?.message !== undefined) return speech.message;
  if (
    transientError !== undefined
    && (speech === undefined
      || (speech.phase === "confirming" && transientError.generation === speech.generation))
  ) {
    return transientError.message;
  }
  return loadError;
}

export type SpeechPopupKeyAction = "cancel" | "send" | "ignore";

export function speechPopupKeyAction(
  key: string,
  shiftKey: boolean,
  composing: boolean,
  keyCode: number,
  phase: SpeechView["phase"],
): SpeechPopupKeyAction {
  if (key === "Escape") return phase === "sending" ? "ignore" : "cancel";
  if (key !== "Enter" || shiftKey || phase !== "confirming") return "ignore";
  return composing || keyCode === 229 ? "ignore" : "send";
}
