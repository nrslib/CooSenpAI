use crate::state::SpeakerIdentificationStatus;
use serde::{Deserialize, Serialize};
use serde_json::Number;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SpeakerDecisionPhase {
    Initial,
    InitialRecent,
    BackfillAnchor,
    BackfillSamples,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerCandidateScore {
    pub speaker_id: String,
    pub score: Number,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerSupportingSample {
    pub segment_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub anchor_id: String,
    pub anchor_score: Number,
    pub score: Number,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerRecentComparison {
    pub segment_id: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub score: Number,
}

/// 判定時のスコアと参照。声紋・音声を含めず、Numberで非有限値を排除する。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpeakerDecisionDetails {
    pub start_ms: u64,
    pub end_ms: u64,
    pub decision_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_id: Option<String>,
    pub model_package_digest: String,
    pub phase: SpeakerDecisionPhase,
    pub status: SpeakerIdentificationStatus,
    pub reason: String,
    pub candidates: Vec<SpeakerCandidateScore>,
    pub candidate_count: usize,
    pub known_threshold: Number,
    pub margin_threshold: Number,
    pub evidence_window_count: usize,
    pub voiced_frame_count: usize,
    pub supporting_samples: Vec<SpeakerSupportingSample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub support_threshold: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_candidate_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_match_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_best_score: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_comparisons: Option<Vec<SpeakerRecentComparison>>,
}

fn score_in(value: &Number, min: f64, max: f64) -> bool {
    value
        .as_f64()
        .is_some_and(|score| score.is_finite() && (min..=max).contains(&score))
}

fn intervals_overlap(left_start: u64, left_end: u64, right_start: u64, right_end: u64) -> bool {
    left_start < right_end && right_start < left_end
}

impl SpeakerDecisionDetails {
    pub(crate) fn validation_reason(&self) -> Option<&'static str> {
        macro_rules! invalid_if {
            ($condition:expr, $reason:literal) => {
                if $condition {
                    return Some($reason);
                }
            };
        }

        invalid_if!(self.end_ms <= self.start_ms, "decision.period-bounds");
        invalid_if!(
            !matches!(
                self.decision_version.as_str(),
                "speaker-cosine-ledger-v6"
                    | "speaker-cosine-ledger-v7"
                    | "speaker-cosine-ledger-v8"
            ),
            "decision.version"
        );
        invalid_if!(
            self.registry_id
                .as_deref()
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_err()),
            "decision.registry-id"
        );
        invalid_if!(
            self.model_package_digest.len() != 64
                || !self
                    .model_package_digest
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "decision.model-digest"
        );
        invalid_if!(
            self.candidate_count > 1_000
                || self.candidates.len() > 3
                || self.candidates.len() != self.candidate_count.min(3),
            "decision.candidate-count"
        );
        invalid_if!(
            !self.candidates.is_empty() && self.registry_id.is_none(),
            "decision.candidate-registry"
        );
        invalid_if!(
            self.evidence_window_count > 64,
            "decision.evidence-window-count"
        );
        invalid_if!(
            self.voiced_frame_count > 3_200,
            "decision.voiced-frame-count"
        );
        invalid_if!(
            !score_in(&self.known_threshold, 0.0, 1.0)
                || !score_in(&self.margin_threshold, 0.0, 1.0),
            "decision.threshold"
        );
        invalid_if!(
            !self.candidates.iter().all(|candidate| {
                crate::speaker_id::is_valid_speaker_id(&candidate.speaker_id)
                    && score_in(&candidate.score, -1.000_001, 1.000_001)
            }),
            "decision.candidate"
        );
        invalid_if!(
            self.candidates
                .windows(2)
                .any(|pair| pair[0].score.as_f64() < pair[1].score.as_f64()),
            "decision.candidate-order"
        );
        invalid_if!(
            self.candidates
                .iter()
                .enumerate()
                .any(|(index, candidate)| {
                    self.candidates[..index]
                        .iter()
                        .any(|prior| prior.speaker_id == candidate.speaker_id)
                }),
            "decision.candidate-duplicate"
        );
        if let Some(comparisons) = self.recent_comparisons.as_ref() {
            invalid_if!(comparisons.len() > 64, "decision.recent-comparison-count");
            for (index, comparison) in comparisons.iter().enumerate() {
                invalid_if!(
                    uuid::Uuid::parse_str(&comparison.segment_id).is_err()
                        || comparison.end_ms <= comparison.start_ms
                        || !score_in(&comparison.score, -1.0, 1.0),
                    "decision.recent-comparison"
                );
                invalid_if!(
                    comparisons[..index].iter().any(|previous| {
                        previous.segment_id == comparison.segment_id
                            && intervals_overlap(
                                previous.start_ms,
                                previous.end_ms,
                                comparison.start_ms,
                                comparison.end_ms,
                            )
                    }),
                    "decision.recent-comparison-overlap"
                );
            }
        }
        invalid_if!(
            self.recent_comparisons.is_some() && self.phase != SpeakerDecisionPhase::InitialRecent,
            "decision.recent-comparison-phase"
        );
        invalid_if!(
            !matches!(
                self.reason.as_str(),
                "matched-known"
                    | "enrolled-new"
                    | "enrolled-pending"
                    | "pending-candidate"
                    | "below-known-above-new"
                    | "single-window"
                    | "mixed-clusters"
                    | "no-evidence"
                    | "matched-samples"
                    | "recent-consensus"
                    | "replayed-confirmed"
                    | "ambiguous-representatives"
            ),
            "decision.reason"
        );
        let expected_status = match self.reason.as_str() {
            "matched-known" | "enrolled-new" | "enrolled-pending" | "matched-samples"
            | "recent-consensus" | "replayed-confirmed" => SpeakerIdentificationStatus::Identified,
            "mixed-clusters" => SpeakerIdentificationStatus::Mixed,
            _ => SpeakerIdentificationStatus::Unknown,
        };
        if self.status != expected_status
            || (self.status == SpeakerIdentificationStatus::Identified
                && self.registry_id.is_none())
            || (self.phase == SpeakerDecisionPhase::BackfillAnchor
                && self.reason != "matched-known")
        {
            return Some("decision.status");
        }
        if !matches!(
            self.phase,
            SpeakerDecisionPhase::Initial | SpeakerDecisionPhase::InitialRecent
        ) && (self.status != SpeakerIdentificationStatus::Identified
            || self.registry_id.is_none())
        {
            return Some("decision.phase-status");
        }
        if self.reason == "replayed-confirmed" {
            if self.phase == SpeakerDecisionPhase::InitialRecent
                && self.candidates.is_empty()
                && self.candidate_count == 0
                && self.supporting_samples.is_empty()
                && self.recent_candidate_count == Some(0)
                && self.recent_match_count == Some(0)
                && self.recent_best_score.is_none()
                && self
                    .support_threshold
                    .as_ref()
                    .is_some_and(|value| score_in(value, 0.0, 1.0))
            {
                return None;
            }
            return Some("decision.replay-shape");
        }
        if self.phase == SpeakerDecisionPhase::InitialRecent {
            let counts_valid = match (self.recent_candidate_count, self.recent_match_count) {
                (Some(candidates), Some(matches)) => {
                    candidates <= 64
                        && matches <= candidates
                        && self
                            .recent_comparisons
                            .as_ref()
                            .is_none_or(|items| items.len() <= candidates)
                        && self
                            .recent_best_score
                            .as_ref()
                            .map_or(candidates == 0, |score| {
                                candidates > 0 && score_in(score, -1.000_001, 1.000_001)
                            })
                }
                (None, None) => {
                    self.recent_best_score.is_none()
                        && self.recent_comparisons.is_none()
                        && matches!(self.reason.as_str(), "mixed-clusters" | "no-evidence")
                }
                _ => false,
            };
            if !counts_valid {
                return Some("decision.recent-counts");
            }
            if self.status != SpeakerIdentificationStatus::Identified {
                return if self.supporting_samples.is_empty()
                    && self
                        .support_threshold
                        .as_ref()
                        .is_some_and(|value| score_in(value, 0.0, 1.0))
                {
                    None
                } else {
                    Some("decision.recent-support")
                };
            }

            let Some(threshold) = &self.support_threshold else {
                return Some("decision.support-threshold");
            };
            if !score_in(threshold, 0.0, 1.0) {
                return Some("decision.support-threshold");
            }
            let valid = match self.reason.as_str() {
                // v8 の初回登録は current period 自身を seed にするため、
                // 直近の支持サンプルを持たない。
                "enrolled-new" => self.supporting_samples.is_empty(),
                // 直接一致は直近の一観測、独立三観測の成立後は二観測を
                // 支持として記録する。同じ境界で両方を受理する。
                "matched-samples" => {
                    valid_supporting_samples(&self.supporting_samples, threshold, 1, 2)
                }
                "enrolled-pending" => {
                    valid_supporting_samples(&self.supporting_samples, threshold, 2, 2)
                }
                // recent consensus は period 自身の候補ではなく、二つの
                // 独立した recent 支持から過去の unknown period を訂正する。
                "recent-consensus" => {
                    self.candidates.is_empty()
                        && self.candidate_count == 0
                        && self.margin_threshold.as_f64() == Some(0.0)
                        && self.recent_candidate_count == Some(2)
                        && self.recent_match_count == Some(2)
                        && self
                            .recent_comparisons
                            .as_ref()
                            .is_some_and(|items| items.len() == 2)
                        && valid_supporting_samples(&self.supporting_samples, threshold, 2, 2)
                }
                _ => false,
            };
            return if valid {
                None
            } else {
                Some("decision.recent-reason")
            };
        } else if self.recent_candidate_count.is_some()
            || self.recent_match_count.is_some()
            || self.recent_best_score.is_some()
        {
            return Some("decision.recent-fields");
        }
        if !matches!(
            self.phase,
            SpeakerDecisionPhase::BackfillSamples | SpeakerDecisionPhase::InitialRecent
        ) {
            return if self.supporting_samples.is_empty()
                && self.support_threshold.is_none()
                && self.reason != "matched-samples"
            {
                None
            } else {
                Some("decision.support-shape")
            };
        }
        let Some(threshold) = &self.support_threshold else {
            return Some("decision.support-threshold");
        };
        if !matches!(self.reason.as_str(), "matched-samples" | "enrolled-pending") {
            return Some("decision.support-reason");
        }
        if !valid_supporting_samples(&self.supporting_samples, threshold, 2, 2) {
            return Some("decision.supporting-samples");
        }
        None
    }

    pub fn is_valid(&self) -> bool {
        self.validation_reason().is_none()
    }

    pub(crate) fn validation_reason_for_segment(&self, segment_id: &str) -> Option<&'static str> {
        if let Some(reason) = self.validation_reason() {
            return Some(reason);
        }
        // Swift の同一発話内の複数期間は同じ segmentId を再利用するため、重複期間だけ拒否する。
        if self.supporting_samples.iter().any(|sample| {
            sample.segment_id == segment_id
                && intervals_overlap(self.start_ms, self.end_ms, sample.start_ms, sample.end_ms)
        }) || self
            .recent_comparisons
            .as_deref()
            .unwrap_or_default()
            .iter()
            .any(|comparison| {
                comparison.segment_id == segment_id
                    && intervals_overlap(
                        self.start_ms,
                        self.end_ms,
                        comparison.start_ms,
                        comparison.end_ms,
                    )
            })
        {
            return Some("decision.self-reference");
        }
        None
    }

    pub(crate) fn is_valid_for_segment(&self, segment_id: &str) -> bool {
        self.validation_reason_for_segment(segment_id).is_none()
    }
}

fn valid_supporting_samples(
    samples: &[SpeakerSupportingSample],
    threshold: &Number,
    minimum: usize,
    maximum: usize,
) -> bool {
    if !score_in(threshold, 0.0, 1.0)
        || !(minimum..=maximum).contains(&samples.len())
        || samples.is_empty()
    {
        return false;
    }
    let anchor_id = &samples[0].anchor_id;
    samples.iter().enumerate().all(|(index, sample)| {
        uuid::Uuid::parse_str(&sample.segment_id).is_ok()
            && crate::speaker_id::is_valid_speaker_id(&sample.anchor_id)
            && sample.anchor_id == *anchor_id
            && sample.end_ms > sample.start_ms
            && score_in(&sample.anchor_score, -1.000_001, 1.000_001)
            && score_in(&sample.score, -1.000_001, 1.000_001)
            && sample.score.as_f64() >= threshold.as_f64()
            && samples[..index]
                .iter()
                .all(|previous| previous.segment_id != sample.segment_id)
    })
}
