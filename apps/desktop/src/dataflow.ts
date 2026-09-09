import { t, type Locale, type TranslationKey } from "./i18n/index.js";
import type {
  AppSnapshot,
  AudioObservation,
  AudioLogEvent,
  CompanionDecision,
  NoChangeObservation,
  ObservationRecord,
  VisualObservation,
} from "./types.js";

export type DataFlowKind = "visual" | "hearing" | "companion";
export type DataFlowKindFilter = "all" | DataFlowKind;

export interface DataFlowEvent {
  readonly id: string;
  readonly kind: DataFlowKind;
  readonly at: string;
  readonly summary: string;
  readonly detail?: string;
}

export const DATAFLOW_KIND_LABELS: Readonly<Record<DataFlowKind, TranslationKey>> = {
  visual: "details.dataflowFilterVisual",
  hearing: "details.dataflowFilterHearing",
  companion: "details.dataflowFilterSpeech",
};

function preview(text: string, max = 80): string {
  const collapsed = text.replace(/\s+/gu, " ").trim();
  return collapsed.length > max ? `${collapsed.slice(0, max)}…` : collapsed;
}

function detailJson(value: unknown): string {
  return JSON.stringify(value, null, 2);
}

function audioSourceLabel(source: "microphone" | "speaker", locale: Locale): string {
  return source === "speaker" ? t(locale, "now.speaker") : t(locale, "now.microphone");
}

function visualEvent(record: VisualObservation, locale: Locale): DataFlowEvent {
  const wake = record.wakeCompanion ? ` · ${t(locale, "details.dataflowWake")}` : "";
  return {
    id: `visual:${record.id}`,
    kind: "visual",
    at: record.createdAt,
    summary: `${preview(record.activity)}${wake}`,
    detail: detailJson(record),
  };
}

function noChangeEvent(record: NoChangeObservation, locale: Locale): DataFlowEvent {
  const stagnation = record.stagnation === undefined || record.stagnation.detail === ""
    ? ""
    : ` · ${preview(record.stagnation.detail)}`;
  return {
    id: `visual:${record.id}`,
    kind: "visual",
    at: record.createdAt,
    summary: `${t(locale, "details.dataflowNoChange")}${stagnation}`,
    detail: detailJson(record),
  };
}

function hearingSegmentEvent(
  record: AudioObservation,
  transcriptText: string | undefined,
  locale: Locale,
): DataFlowEvent {
  return {
    id: `hearing:${record.id}`,
    kind: "hearing",
    at: record.createdAt,
    summary: `${audioSourceLabel(record.source, locale)} · ${t(locale, "details.dataflowTranscriptConfirmed")}: ${preview(transcriptText ?? record.text)}`,
    detail: detailJson(record),
  };
}

function hearingLogEvent(event: AudioLogEvent, locale: Locale): DataFlowEvent {
  let summary: string;
  switch (event.stage) {
    case "recognizing":
      summary = t(locale, "details.dataflowRecognizing");
      break;
    case "no-speech":
      summary = t(locale, "details.dataflowNoSpeech");
      break;
    case "confirmed":
      summary = `${t(locale, "details.dataflowTranscriptConfirmed")}: ${preview(event.text)}`;
      break;
  }
  return {
    id: `hearing:${event.id}`,
    kind: "hearing",
    at: event.createdAt,
    summary: `${audioSourceLabel(event.source, locale)} · ${summary}`,
    detail: detailJson(event),
  };
}

function observationEvent(
  record: ObservationRecord,
  transcriptByObservationId: ReadonlyMap<string, string>,
  locale: Locale,
): DataFlowEvent {
  switch (record.kind) {
    case "visual":
      return visualEvent(record, locale);
    case "no-change":
      return noChangeEvent(record, locale);
    case "audio":
      return hearingSegmentEvent(record, transcriptByObservationId.get(record.id), locale);
  }
}

function decisionEvent(decision: CompanionDecision, locale: Locale): DataFlowEvent {
  const summary = decision.emit
    ? `${t(locale, "details.dataflowEmit", { kind: decision.messageKind })}: ${preview(decision.message ?? "")}`
    : `${t(locale, "details.dataflowNoEmit")}: ${preview(decision.thought ?? "")}`;
  return {
    id: `companion-decision:${decision.sequence}`,
    kind: "companion",
    at: decision.occurredAt,
    summary,
    detail: detailJson(decision),
  };
}

export interface DataFlowRecord {
  readonly id: string;
  readonly kind: DataFlowKind;
  readonly at: string;
  readonly content:
    | { readonly type: "observation"; readonly data: { readonly record: ObservationRecord; readonly transcript: string | null } }
    | { readonly type: "hearing"; readonly data: AudioLogEvent }
    | { readonly type: "remark"; readonly data: AppSnapshot["conversation"][number] }
    | { readonly type: "decision"; readonly data: CompanionDecision }
    | { readonly type: "session"; readonly data: AppSnapshot["audio"] }
    | { readonly type: "diagnostic"; readonly data: { readonly kind: string; readonly message: string } }
    | { readonly type: "interruption"; readonly data: null };
}

export function renderDataFlowRecord(record: DataFlowRecord, locale: Locale): DataFlowEvent {
  const content = record.content;
  let summary: string;
  let detail: string | undefined;
  switch (content.type) {
    case "observation": {
      const transcripts = new Map<string, string>();
      if (content.data.transcript !== null) transcripts.set(content.data.record.id, content.data.transcript);
      ({ summary, detail } = observationEvent(content.data.record, transcripts, locale));
      break;
    }
    case "hearing": ({ summary, detail } = hearingLogEvent(content.data, locale)); break;
    case "decision": ({ summary, detail } = decisionEvent(content.data, locale)); break;
    case "remark": summary = preview(content.data.message); detail = detailJson(content.data); break;
    case "session": {
      const audio = content.data;
      summary = t(locale, audio.phase === "listening" ? "details.dataflowSessionStart" : audio.phase === "off" ? "details.dataflowSessionStop" : "details.dataflowSessionError");
      if (audio.phase === "error" && audio.message !== undefined) { summary += `: ${preview(audio.message)}`; detail = audio.message; }
      break;
    }
    case "diagnostic": summary = t(locale, "details.dataflowHearingDiagnostic", { message: preview(content.data.message, 120) }); detail = detailJson(content.data); break;
    case "interruption": summary = t(locale, "details.dataflowUserInterrupted"); break;
  }
  return { id: record.id, kind: record.kind, at: record.at, summary, detail };
}
