export interface SpeechInsertion {
  readonly value: string;
  readonly caret: number;
}

export type SpeechTranscriptDisposition = "defer" | "insert";

export function speechTranscriptDisposition(composing: boolean): SpeechTranscriptDisposition {
  return composing ? "defer" : "insert";
}

export function insertSpeechAtSelection(
  value: string,
  selectionStart: number,
  selectionEnd: number,
  transcript: string,
): SpeechInsertion {
  const start = Math.max(0, Math.min(selectionStart, value.length));
  const end = Math.max(start, Math.min(selectionEnd, value.length));
  return {
    value: `${value.slice(0, start)}${transcript}${value.slice(end)}`,
    caret: start + transcript.length,
  };
}
