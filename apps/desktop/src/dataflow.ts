import { t, type Locale, type TranslationKey } from "./i18n/index.js";
import type {
  AppSnapshot,
  AudioObservation,
  AudioLogEvent,
  CompanionDecision,
  JudgeDecision,
  NoChangeObservation,
  ObservationRecord,
  TranscriptRecord,
  VisualObservation,
} from "./types.js";

export type DataFlowSubject = "vision" | "hearing" | "coo";
export type DataFlowKindFilter = "all" | DataFlowSubject;
export type DataFlowMarker =
  | "screen-observation"
  | "audio-observation"
  | "ocr"
  | "transcript"
  | "audio-status"
  | "session"
  | "diagnostic"
  | "interruption"
  | "thought"
  | "speech"
  | "judge";

export interface DataFlowReference {
  readonly type: "frame" | "transcript" | "observation" | "conversation";
  readonly id: string;
  readonly path?: string;
  readonly available?: boolean;
  readonly observationKind?: DataFlowMarker;
  readonly source?: string;
  readonly text?: string;
}

export interface DataFlowEvent {
  readonly id: string;
  readonly subject: DataFlowSubject;
  readonly marker: DataFlowMarker;
  readonly at: string;
  readonly summary: string;
  readonly detail?: string;
  readonly references: readonly DataFlowReference[];
}

export const DATAFLOW_KIND_LABELS: Readonly<Record<DataFlowSubject, TranslationKey>> = {
  vision: "details.dataflowFilterVisual",
  hearing: "details.dataflowFilterHearing",
  coo: "details.dataflowFilterCoo",
};

export const DATAFLOW_MARKER_LABELS: Readonly<Record<DataFlowMarker, TranslationKey>> = {
  "screen-observation": "details.dataflowMarkerScreen",
  "audio-observation": "details.dataflowMarkerAudioObservation",
  ocr: "details.dataflowMarkerOcr",
  transcript: "details.dataflowMarkerTranscript",
  "audio-status": "details.dataflowMarkerAudioStatus",
  session: "details.dataflowMarkerSession",
  diagnostic: "details.dataflowMarkerDiagnostic",
  interruption: "details.dataflowMarkerInterruption",
  thought: "details.dataflowMarkerThought",
  speech: "details.dataflowMarkerSpeech",
  judge: "details.dataflowMarkerJudge",
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

function normalizeFocus(focus: NonNullable<VisualObservation["frames"][number]["focus"]>) {
  const value = focus.role === "AXSecureTextField" || focus.value === undefined
    ? undefined
    : Array.from(focus.value).slice(0, 600).join("");
  return { ...focus, value };
}

function normalizeVisualObservation(record: VisualObservation): VisualObservation {
  return {
    ...record,
    frames: record.frames.map((frame) => frame.focus === undefined
      ? frame
      : { ...frame, focus: normalizeFocus(frame.focus) }),
  };
}

function visualEvent(record: VisualObservation, locale: Locale): string {
  const wake = record.wakeCompanion ? ` · ${t(locale, "details.dataflowWake")}` : "";
  const window = record.audioSegments?.length ? ` · ${record.windowStart} – ${record.windowEnd}` : "";
  const focus = record.frames.find((frame) => frame.focus !== undefined)?.focus;
  const focusText = focus === undefined
    ? ""
    : ` · ${t(locale, "details.dataflowFocus")}: ${preview(`${focus.bundleId} / ${focus.windowTitle ?? t(locale, "details.dataflowNoWindowTitle")} / ${focus.role} / ${focus.value ?? t(locale, "details.dataflowNoValue")}`)}`;
  return `${preview(record.activity)}${wake}${window}${focusText}`;
}

function noChangeEvent(record: NoChangeObservation, locale: Locale): string {
  const stagnation = record.stagnation === undefined || record.stagnation.detail === ""
    ? ""
    : ` · ${preview(record.stagnation.detail)}`;
  return `${t(locale, "details.dataflowNoChange")}${stagnation}`;
}

function transcriptText(
  record: AudioObservation,
  transcript: string | TranscriptRecord | null | undefined,
): string {
  if (typeof transcript === "string") return transcript;
  return transcript?.text ?? record.text;
}

function speakerAnnotation(
  record: AudioObservation,
  transcript: string | TranscriptRecord | null | undefined,
): string {
  if (record.source !== "speaker") return "";
  const status = typeof transcript === "string" || transcript == null
    ? record.speakerStatus
    : transcript.speakerStatus ?? record.speakerStatus;
  const speakerID = typeof transcript === "string" || transcript == null
    ? record.speakerId
    : transcript.speakerTag ?? record.speakerId;
  if (status === "identified" && speakerID !== undefined) return ` · ${speakerID}`;
  return status === undefined ? "" : ` · ${status}`;
}

function hearingObservationEvent(
  record: AudioObservation,
  transcript: string | TranscriptRecord | null | undefined,
  locale: Locale,
): string {
  return `${audioSourceLabel(record.source, locale)}${speakerAnnotation(record, transcript)} · ${t(locale, "details.dataflowTranscriptConfirmed")}: ${preview(transcriptText(record, transcript))}`;
}

function decisionEvent(decision: CompanionDecision, locale: Locale): string {
  return decision.thought === undefined
    ? t(locale, decision.emit ? "details.dataflowEmit" : "details.dataflowNoEmit", { kind: decision.messageKind })
    : preview(decision.thought);
}

function judgeScore(score: JudgeDecision["novelty"]): string {
  return score === null ? "null" : score.toFixed(2);
}

function referencesOf(record: DataFlowRecord): readonly DataFlowReference[] {
  return record.references;
}

export interface DataFlowRecord {
  readonly id: string;
  readonly subject: DataFlowSubject;
  readonly marker: DataFlowMarker;
  readonly at: string;
  readonly references: readonly DataFlowReference[];
  readonly content:
    | {
        readonly type: "observation";
        readonly data: {
          readonly record: ObservationRecord;
          readonly transcript?: string | TranscriptRecord | null;
          readonly transcripts?: readonly TranscriptRecord[];
        };
      }
    | { readonly type: "hearing"; readonly data: AudioLogEvent }
    | { readonly type: "remark"; readonly data: AppSnapshot["conversation"][number] }
    | { readonly type: "ocr"; readonly data: { readonly observationId: string; readonly frameId: string; readonly text: string } }
    | { readonly type: "decision"; readonly data: CompanionDecision }
    | { readonly type: "session"; readonly data: AppSnapshot["audio"] }
    | { readonly type: "diagnostic"; readonly data: { readonly kind: string; readonly message: string } }
    | { readonly type: "interruption"; readonly data: null }
    | { readonly type: "judge"; readonly data: JudgeDecision };
}

export function renderDataFlowRecord(record: DataFlowRecord, locale: Locale): DataFlowEvent {
  const content = record.content;
  let summary: string;
  let detail: string | undefined;
  switch (content.type) {
    case "observation": {
      const observation = content.data.record.kind === "visual"
        ? normalizeVisualObservation(content.data.record)
        : content.data.record;
      switch (observation.kind) {
        case "visual":
          summary = visualEvent(observation, locale);
          break;
        case "no-change":
          summary = noChangeEvent(observation, locale);
          break;
        case "audio":
          summary = hearingObservationEvent(observation, content.data.transcript, locale);
          break;
      }
      detail = detailJson({ ...content.data, record: observation });
      break;
    }
    case "ocr":
      summary = preview(content.data.text);
      detail = detailJson(content.data);
      break;
    case "hearing": {
      const event = content.data;
      const text = event.stage === "confirmed"
        ? `${t(locale, "details.dataflowTranscriptConfirmed")}: ${preview(event.text)}`
        : event.stage === "recognizing"
          ? t(locale, "details.dataflowRecognizing")
          : t(locale, "details.dataflowNoSpeech");
      summary = `${audioSourceLabel(event.source, locale)} · ${text}`;
      detail = detailJson(event);
      break;
    }
    case "decision":
      summary = decisionEvent(content.data, locale);
      detail = detailJson(content.data);
      break;
    case "remark":
      summary = preview(content.data.message);
      detail = detailJson(content.data);
      break;
    case "session": {
      const audio = content.data;
      summary = t(locale, audio.phase === "listening"
        ? "details.dataflowSessionStart"
        : audio.phase === "off"
          ? "details.dataflowSessionStop"
          : "details.dataflowSessionError");
      if (audio.phase === "error" && audio.message !== undefined) {
        summary += `: ${preview(audio.message)}`;
        detail = audio.message;
      }
      break;
    }
    case "diagnostic":
      summary = t(locale, "details.dataflowHearingDiagnostic", { message: preview(content.data.message, 120) });
      detail = detailJson(content.data);
      break;
    case "interruption":
      summary = t(locale, "details.dataflowUserInterrupted");
      break;
    case "judge":
      summary = t(locale, "details.dataflowJudge", {
        action: content.data.action,
        novelty: judgeScore(content.data.novelty),
        relevance: judgeScore(content.data.relevance),
      });
      detail = detailJson(content.data);
      break;
  }
  return {
    id: record.id,
    subject: record.subject,
    marker: record.marker,
    at: record.at,
    summary,
    detail,
    references: referencesOf(record),
  };
}
