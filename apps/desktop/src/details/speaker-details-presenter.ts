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
    readonly version: string;
    readonly decidedAt: string;
    readonly decisionKind: string;
    readonly roleNotice: string | null;
    readonly pendingSummary: string | null;
    readonly unknownSummary: string | null;
    readonly noRepresentatives: string | null;
    readonly interval: string;
    readonly criteria: string;
    readonly margin: string;
    readonly evidence: string;
    readonly candidateTitle: string;
    readonly candidateSimilarity: string;
    readonly recentComparisonTitle: string;
    readonly candidates: readonly { readonly id: string; readonly label: string; readonly score: string; readonly rankReason: string }[];
    readonly support: readonly { readonly key: string; readonly label: string; readonly score: string }[];
    readonly supportTitle: string;
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

const REASONS: Record<string, TranslationKey> = {
  "enrolled-new": "details.speakerReasonNew",
  "enrolled-pending": "details.speakerReasonNew",
  "pending-candidate": "details.speakerReasonPending",
  "mixed-clusters": "details.speakerReasonMixed",
  "no-evidence": "details.speakerReasonNone",
  "recent-consensus": "details.speakerReasonRecentConsensus",
  "replayed-confirmed": "details.speakerReasonReplay",
  "ambiguous-representatives": "details.speakerReasonMargin",
  "matched-samples": "details.speakerReasonMatchedSamples",
};
const PHASES: Record<string, TranslationKey> = {
  initial: "details.speakerInitial",
  "initial-recent": "details.speakerInitial",
  "backfill-samples": "details.speakerBackfillSamples",
};
const STATUSES: Record<string, TranslationKey> = {
  identified: "details.speakerIdentified",
  unknown: "details.logSpeakerUnknown",
  mixed: "details.logSpeakerMixed",
  unavailable: "details.logSpeakerNotIdentified",
};
const CANDIDATE_ROLES: Record<NonNullable<SpeakerDecisionDetails["candidates"][number]["role"]>, TranslationKey> = {
  adoption: "details.speakerCandidateAdoption",
  "top-below-threshold": "details.speakerCandidateBelowThreshold",
  "top-margin-shortfall": "details.speakerCandidateMarginShortfall",
  "lower-ranked": "details.speakerCandidateLowerRanked",
};
const RECENT_ROLES: Record<NonNullable<NonNullable<SpeakerDecisionDetails["recentComparisons"]>[number]["roles"]>[number], TranslationKey> = {
  adoption: "details.speakerRecentRoleAdoption",
  "enrollment-check": "details.speakerRecentRoleEnrollmentCheck",
  "enrollment-veto": "details.speakerRecentRoleEnrollmentVeto",
  "triad-check": "details.speakerRecentRoleTriadCheck",
  "triad-member": "details.speakerRecentRoleTriadMember",
  "triad-support": "details.speakerRecentRoleTriadSupport",
  unused: "details.speakerRecentRoleUnused",
};
const PENDING_CONDITIONS: Record<NonNullable<SpeakerDecisionDetails["pendingConditions"]>[number], TranslationKey> = {
  "evidence-windows": "details.speakerPendingEvidenceWindows",
  "voiced-frames": "details.speakerPendingVoicedFrames",
  "no-triad": "details.speakerPendingNoTriad",
  "no-mutual-consensus": "details.speakerPendingNoMutualConsensus",
  "triad-voiced-frames": "details.speakerPendingTriadVoicedFrames",
  "recent-similarity-veto": "details.speakerPendingRecentVeto",
  duplicate: "details.speakerPendingDuplicate",
  "already-corrected": "details.speakerPendingAlreadyCorrected",
};

function translatedValue(locale: Locale, values: Record<string, TranslationKey>, value: string, label: string): string {
  const key = Object.prototype.hasOwnProperty.call(values, value) ? values[value] : undefined;
  return key === undefined ? label + ": " + value : t(locale, key);
}

function isInitialDecision(decision: SpeakerDecisionDetails): boolean {
  return decision.decisionKind === "initial" || (decision.decisionKind === undefined
    && (decision.phase === "initial" || (decision.phase === "initial-recent"
      && decision.reason !== "recent-consensus")));
}

export function presentSpeakerDetails(entry: ConversationLogEntry | null | undefined, locale: Locale): SpeakerDetailsView | null {
  if (entry == null) return null;
  const details = entry.speakerDecisionDetails ?? [];
  const translate = (key: TranslationKey): string => t(locale, key);
  const unrecorded = translate("details.speakerUnrecorded");
  return {
    currentStatus: entry.speakerStatus === undefined ? translate("details.logSpeakerNotIdentified")
      : translatedValue(locale, STATUSES, entry.speakerStatus, translate("details.speakerStatusLabel")),
    missingInitial: !details.some(isInitialDecision),
    decisions: details.map((decision) => {
      const rolesRecorded = decision.rolesRecorded === true;
      const comparisonSources = new Map((decision.recentComparisonSources ?? []).map((source) => [
        source.segmentId + ":" + source.startMs + ":" + source.endMs, source,
      ]));
      const recentComparisons = decision.recentComparisons?.map((comparison) => {
        const key = comparison.segmentId + ":" + comparison.startMs + ":" + comparison.endMs;
        const source = comparisonSources.get(key);
        const characters = source?.text === undefined ? null : Array.from(source.text);
        if (rolesRecorded && (comparison.roles === undefined || comparison.roles.length === 0)) {
          throw new Error("Recorded speaker comparison has no role");
        }
        return {
          key,
          sourceLabel: comparison.segmentId.slice(0, 8) + " ("
            + (comparison.startMs / 1000).toFixed(3) + "–" + (comparison.endMs / 1000).toFixed(3) + " s)",
          timeValue: source?.time ?? null,
          timeLabel: source?.time === undefined ? null : formatTime(source.time, locale),
          excerpt: characters === null ? null
            : characters.slice(0, 100).join("") + (characters.length > 100 ? "…" : ""),
          score: comparison.score.toFixed(3),
          basis: rolesRecorded
            ? comparison.roles!.map((role) => translate(RECENT_ROLES[role])).join("・") : "—",
          sourceMissing: source?.time === undefined && source?.text === undefined,
        };
      }) ?? null;
      const candidates = decision.candidates.map((candidate) => {
        if (rolesRecorded && candidate.role === undefined) {
          throw new Error("Recorded speaker candidate has no role");
        }
        return {
          id: candidate.speakerId,
          label: speakerDisplayName(candidate.speakerId, decision.candidateNames[candidate.speakerId]),
          score: candidate.score === -1 ? "—" : candidate.score.toFixed(3),
          rankReason: rolesRecorded ? translate(CANDIDATE_ROLES[candidate.role!]) : "—",
        };
      });
      const recentMatchCount = decision.recentMatchCount?.toString() ?? unrecorded;
      const recentBestScore = decision.recentBestScore?.toFixed(3) ?? unrecorded;
      const criteria = translate("details.speakerThreshold") + ": " + decision.knownThreshold.toFixed(3)
        + " / " + translate("details.speakerMarginThreshold") + ": " + decision.marginThreshold.toFixed(3)
        + (decision.newSpeakerSimilarityThreshold === undefined ? ""
          : " / " + translate("details.speakerNewThreshold") + ": "
            + decision.newSpeakerSimilarityThreshold.toFixed(3));
      const pendingMetric = (condition: NonNullable<SpeakerDecisionDetails["pendingConditions"]>[number]): string | null => {
        const metrics: Partial<Record<typeof condition, [TranslationKey, number | undefined, number | undefined]>> = {
          "evidence-windows": ["details.speakerWindows", decision.evidenceWindowCount,
            decision.minimumEnrollmentEvidenceWindowCount],
          "voiced-frames": ["details.speakerPendingVoicedFramesMetric", decision.voicedFrameCount,
            decision.minimumEnrollmentVoicedFrameCount],
          "no-triad": ["details.speakerPendingIndependentPairs", decision.independentPriorCandidatePairCount, 1],
          "no-mutual-consensus": ["details.speakerPendingMutualPairs", decision.mutualConsensusPairCount, 1],
          "triad-voiced-frames": ["details.speakerPendingSpeechEligiblePairs", decision.speechEligibleConsensusPairCount, 1],
        };
        const metric = metrics[condition];
        return metric === undefined ? null : translate(metric[0]) + " "
          + (metric[1]?.toString() ?? unrecorded) + " / " + (metric[2]?.toString() ?? unrecorded);
      };
      const pendingSummary = decision.pendingConditions?.map((condition) => {
        const metric = pendingMetric(condition);
        return translate(PENDING_CONDITIONS[condition]) + (metric === null ? "" : "（" + metric + "）");
      }).join("・") ?? null;
      const top = decision.candidates[0];
      const second = decision.candidates[1];
      const marginOutcome = decision.representativeMargin === undefined
        ? translate("details.speakerMarginOutcomeUnrecorded")
        : translate(decision.representativeMargin >= decision.marginThreshold
          ? "details.speakerMarginReached" : "details.speakerMarginShortfall");
      const unknownSummary = decision.status === "unknown" ? t(locale, "details.speakerUnknownSummary", {
        topScore: top?.score.toFixed(2) ?? unrecorded,
        topName: top === undefined ? unrecorded : speakerDisplayName(top.speakerId, decision.candidateNames[top.speakerId]),
        secondScore: second?.score.toFixed(2) ?? unrecorded,
        secondName: second === undefined ? unrecorded : speakerDisplayName(second.speakerId, decision.candidateNames[second.speakerId]),
        margin: decision.representativeMargin?.toString() ?? unrecorded,
        required: decision.marginThreshold.toFixed(2), outcome: marginOutcome,
      }) : null;
      return {
        key: decision.startMs + ":" + decision.endMs + ":" + decision.phase,
        phase: translatedValue(locale, PHASES, decision.phase, translate("details.speakerPhaseLabel")),
        status: translatedValue(locale, STATUSES, decision.status, translate("details.speakerStatusLabel")),
        reason: translatedValue(locale, REASONS, decision.reason, translate("details.speakerReasonLabel")),
        version: decision.decisionVersion.replace("speaker-cosine-ledger-", ""),
        decidedAt: decision.decidedAt ?? unrecorded,
        decisionKind: decision.decisionKind === undefined ? unrecorded
          : translate(decision.decisionKind === "initial"
            ? "details.speakerDecisionInitial" : "details.speakerDecisionCorrection"),
        roleNotice: rolesRecorded ? null : translate("details.speakerRolesUnrecorded"),
        pendingSummary,
        unknownSummary,
        noRepresentatives: decision.representativeIDCount === 0
          ? translate("details.speakerNoRepresentatives") : null,
        interval: (decision.startMs / 1000).toFixed(3) + "–" + (decision.endMs / 1000).toFixed(3) + " s",
        criteria,
        margin: translate("details.speakerMargin") + ": "
          + (decision.representativeMargin?.toFixed(9) ?? unrecorded),
        evidence: translate("details.speakerWindows") + ": " + decision.evidenceWindowCount
          + " / " + translate("details.speakerVoiced") + ": " + (decision.voicedFrameCount / 100).toFixed(2) + " s",
        candidateTitle: translate("details.speakerCandidateNeutralTitle"),
        candidateSimilarity: translate("details.speakerCandidateNeutralSimilarity"),
        recentComparisonTitle: translate("details.speakerRecentNeutralTitle"),
        candidates,
        support: decision.supportingSamples.map((sample) => ({
          key: sample.segmentId + ":" + sample.startMs + ":" + sample.endMs,
          label: sample.segmentId.slice(0, 8) + " (" + (sample.startMs / 1000).toFixed(3)
            + "–" + (sample.endMs / 1000).toFixed(3) + " s)",
          score: translate("details.speakerSupportSimilarity") + ": " + sample.score.toFixed(3)
            + " / " + translate("details.speakerAnchorSimilarity") + ": " + sample.anchorScore.toFixed(3),
        })),
        supportTitle: translate("details.speakerSupportNeutralTitle"),
        supportCriteria: decision.supportThreshold === undefined ? null
          : translate("details.speakerThreshold") + ": " + decision.supportThreshold.toFixed(3),
        recent: decision.recentCandidateCount === undefined ? null
          : translate("details.speakerRecentCandidates") + ": " + decision.recentCandidateCount
            + " / " + translate("details.speakerRecentMatches") + ": " + recentMatchCount
            + " / " + translate("details.speakerRecentBest") + ": " + recentBestScore,
        recentComparisons,
      };
    }),
  };
}
