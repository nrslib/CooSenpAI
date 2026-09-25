import { t, type Locale, type TranslationKey } from "../i18n/index.js";
import type { ConversationLogEntry, SpeakerDecisionDetails } from "../types.js";
import { formatTime, speakerDisplayName } from "../view-model.js";

export interface SpeakerDetailsView {
  readonly currentStatus: string;
  readonly missingInitial: boolean;
  readonly decisions: readonly {
    readonly key: string;
    readonly phase: string;
    readonly status: string;
    readonly reason: string;
    readonly interval: string;
    readonly criteria: string;
    readonly margin: string;
    readonly evidence: string;
    readonly candidates: readonly { readonly id: string; readonly label: string; readonly score: string }[];
    readonly support: readonly { readonly key: string; readonly label: string; readonly score: string }[];
    readonly supportCriteria: string | null;
    readonly recent: string | null;
    readonly recentComparisons: readonly {
      readonly key: string;
      readonly sourceLabel: string;
      readonly timeValue: string | null;
      readonly timeLabel: string | null;
      readonly excerpt: string | null;
      readonly score: string;
      readonly basis: string;
      readonly sourceMissing: boolean;
    }[] | null;
  }[];
}

const REASONS: Record<SpeakerDecisionDetails["reason"], TranslationKey> = {
  "matched-known": "details.speakerReasonKnown",
  "enrolled-new": "details.speakerReasonNew",
  "enrolled-pending": "details.speakerReasonNew",
  "pending-candidate": "details.speakerReasonPending",
  "below-known-above-new": "details.speakerReasonBelow",
  "single-window": "details.speakerReasonShort",
  "mixed-clusters": "details.speakerReasonMixed",
  "no-evidence": "details.speakerReasonNone",
  "matched-samples": "details.speakerReasonSamples",
  "replayed-confirmed": "details.speakerReasonReplay",
  "ambiguous-representatives": "details.speakerReasonMargin",
};
const PHASES: Record<SpeakerDecisionDetails["phase"], TranslationKey> = {
  initial: "details.speakerInitial",
  "initial-recent": "details.speakerInitial",
  "backfill-anchor": "details.speakerBackfillAnchor",
  "backfill-samples": "details.speakerBackfillSamples",
};
const STATUSES: Record<SpeakerDecisionDetails["status"], TranslationKey> = {
  identified: "details.speakerIdentified", unknown: "details.logSpeakerUnknown",
  mixed: "details.logSpeakerMixed", unavailable: "details.logSpeakerNotIdentified",
};

export function presentSpeakerDetails(entry: ConversationLogEntry | null | undefined, locale: Locale): SpeakerDetailsView | null {
  if (entry == null) return null;
  const details = entry.speakerDecisionDetails ?? [];
  const translate = (key: TranslationKey): string => t(locale, key);
  return {
    currentStatus: translate(entry.speakerStatus === undefined ? "details.logSpeakerNotIdentified" : STATUSES[entry.speakerStatus]),
    missingInitial: !details.some((decision) => decision.phase === "initial" || decision.phase === "initial-recent"),
    decisions: details.map((decision) => {
      const recentOnly = decision.phase === "initial-recent" && decision.candidates.length === 0;
      const margin = decision.candidates.length >= 2
        ? decision.candidates[0]!.score - decision.candidates[1]!.score : null;
      const reason = decision.reason === "below-known-above-new"
        && decision.candidates[0] !== undefined && decision.candidates[0].score >= decision.knownThreshold
        && margin !== null && margin < decision.marginThreshold
        ? "details.speakerReasonMargin"
        : recentOnly && decision.reason === "matched-samples"
          ? "details.speakerReasonRecentSamples" : REASONS[decision.reason];
      const comparisonSources = new Map((decision.recentComparisonSources ?? []).map((source) => [
        `${source.segmentId}:${source.startMs}:${source.endMs}`, source,
      ]));
      const supportKeys = new Set(decision.supportingSamples.map((sample) =>
        `${sample.segmentId}:${sample.startMs}:${sample.endMs}`));
      const recentComparisons = decision.recentComparisons?.map((comparison) => {
        const key = `${comparison.segmentId}:${comparison.startMs}:${comparison.endMs}`;
        const source = comparisonSources.get(key);
        const text = source?.text;
        const characters = text === undefined ? null : Array.from(text);
        const excerpt = characters === null ? null
          : `${characters.slice(0, 100).join("")}${characters.length > 100 ? "…" : ""}`;
        const basis = supportKeys.has(key)
          ? recentOnly ? "details.speakerRecentUsedRecent" : "details.speakerRecentUsed"
          : comparison.score >= decision.knownThreshold
            ? recentOnly ? "details.speakerRecentAboveNotUsedRecent" : "details.speakerRecentAboveNotUsed"
            : "details.speakerRecentBelow";
        return {
          key,
          sourceLabel: `${comparison.segmentId.slice(0, 8)} (${(comparison.startMs / 1000).toFixed(3)}–${(comparison.endMs / 1000).toFixed(3)} s)`,
          timeValue: source?.time ?? null,
          timeLabel: source?.time === undefined ? null : formatTime(source.time, locale),
          excerpt,
          score: comparison.score.toFixed(3),
          basis: translate(basis),
          sourceMissing: source?.time === undefined && source?.text === undefined,
        };
      }) ?? null;
      return {
        key: `${decision.startMs}:${decision.endMs}:${decision.phase}`,
        phase: translate(PHASES[decision.phase]), status: translate(STATUSES[decision.status]), reason: translate(reason),
        interval: `${(decision.startMs / 1000).toFixed(3)}–${(decision.endMs / 1000).toFixed(3)} s`,
        criteria: recentOnly
          ? decision.reason === "replayed-confirmed"
            ? translate("details.speakerReplayCriteria")
            : `${translate("details.speakerRecentCriteria")}: ${decision.knownThreshold.toFixed(3)}`
          : `${translate("details.speakerThreshold")}: ${decision.knownThreshold.toFixed(3)} / ${translate("details.speakerMarginThreshold")}: ${decision.marginThreshold.toFixed(3)}`,
        margin: recentOnly ? translate("details.speakerRecentNoMargin")
          : `${translate("details.speakerMargin")}: ${margin === null ? translate("details.speakerNoRunnerUp") : margin.toFixed(3)}`,
        evidence: `${translate("details.speakerWindows")}: ${decision.evidenceWindowCount} / ${translate("details.speakerVoiced")}: ${(decision.voicedFrameCount / 100).toFixed(2)} s`,
        candidates: decision.candidates.map((candidate) => ({
          id: candidate.speakerId,
          label: speakerDisplayName(candidate.speakerId, decision.candidateNames[candidate.speakerId]),
          score: candidate.score.toFixed(3),
        })),
        support: decision.supportingSamples.map((sample) => ({
          key: `${sample.segmentId}:${sample.startMs}:${sample.endMs}`,
          label: `${sample.segmentId.slice(0, 8)} (${(sample.startMs / 1000).toFixed(3)}–${(sample.endMs / 1000).toFixed(3)} s)`,
          score: recentOnly
            ? `${sample.score.toFixed(3)} / ${translate("details.speakerRecentGroupScore")} ${sample.anchorScore.toFixed(3)}`
            : `${sample.score.toFixed(3)} / anchor ${sample.anchorScore.toFixed(3)}`,
        })),
        recent: decision.recentCandidateCount === undefined ? null
          : `${translate("details.speakerRecentCandidates")}: ${decision.recentCandidateCount} / ${translate("details.speakerRecentMatches")}: ${decision.recentMatchCount} / ${translate("details.speakerRecentBest")}: ${decision.recentBestScore?.toFixed(3) ?? "—"}`,
        supportCriteria: decision.supportThreshold === undefined ? null
          : `${translate("details.speakerSupport")}: ${decision.supportingSamples.length} / ${translate("details.speakerThreshold")}: ${decision.supportThreshold.toFixed(3)}`,
        recentComparisons,
      };
    }),
  };
}
