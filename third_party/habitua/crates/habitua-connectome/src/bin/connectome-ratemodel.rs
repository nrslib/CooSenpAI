use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use habitua::{
    BrainConfig, BrainInput, BrainInputSpec, BrainLearningPolicy, BrainMode, BrainRunner,
    ConsensusAction, ConsensusPolicy, DeletionPolicy, LearningMode, ModelRegistry, ModelSettings,
    ModelSpec, ReactionKind, ReactionPolicy, ResourcePolicy, StatePolicy, Timestamp,
};
use habitua_connectome::{
    FeedbackEvent, FeedbackKind, FeedbackStore, FrozenReadoutFeedback, FrozenReadoutHistoryEntry,
    HabituationConfig, HabituationState, RateGraph, RateHelperArtifact, RateHelperBrain,
    RateHelperCaseMemory, RateHelperResponseGroup, RateLearningConfig, RateLearningState,
    RateParameters, RetinaMap, RetinaMapConfig, ScreenFixture, ScreenFixtureKind, TrainingMode,
    final_activity_with_parameters_profile, frozen_readout_augmented_features_with_expiry,
    register_frozen_readout_model, register_rate_brain_model, shuffle_feedback, shuffle_weights,
    shuffle_wiring,
};
use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const RATE_EVALUATION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RateAuditCriteria {
    same_l2_max: f64,
    near_direction_min: f64,
    unrelated_direction_min: f64,
    ordering_margin: f64,
    expected_same_pair_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RateFrozenCriteria {
    steps: usize,
    response_definition: String,
    component_scaling: String,
    component_scale_floor: f64,
    zero_response_epsilon: f64,
    minimum_active_readout_fraction: f64,
    maximum_saturation_fraction: f64,
    minimum_separation_margin: f64,
    response_groups: Vec<RateResponseGroupCriteria>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RateResponseGroupCriteria {
    name: String,
    prefixes: Vec<String>,
    excluded_population_names: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RateTrainingCriteria {
    projection_dimension: usize,
    seed: u64,
    wiring_swaps: usize,
    weight_shuffle_seed: u64,
    feedback_shuffle_seed: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RateNumericalCriteria {
    f32_reference_abs_tolerance: f64,
    finite_difference_abs_tolerance: f64,
    nonlinear_boundary_margin: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RateRetinaCriteria {
    coordinate_rule: String,
    unknown_side_policy: String,
    maximum_inferred_coordinate_count: usize,
    maximum_inferred_spread_p95: f64,
    maximum_inferred_spread: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RateEvaluationCriteria {
    schema_version: u32,
    width: usize,
    height: usize,
    pair_count: usize,
    input_types: Vec<String>,
    readout_population: String,
    simulation_seed_base: u64,
    audit: RateAuditCriteria,
    frozen: RateFrozenCriteria,
    numerical: RateNumericalCriteria,
    retina: RateRetinaCriteria,
    training: RateTrainingCriteria,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FixtureRecord {
    id: String,
    kind: ScreenFixtureKind,
    pair_index: usize,
    side: String,
    path: String,
    width: usize,
    height: usize,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FixtureManifest {
    schema_version: u32,
    width: usize,
    height: usize,
    pair_count: usize,
    records: Vec<FixtureRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RealScreenRecord {
    id: String,
    path: String,
    captured_at_s: f64,
    stream_id: String,
    split: String,
    app: String,
    #[serde(alias = "document")]
    title: String,
    independent_label: String,
    label_reason: String,
    width: usize,
    height: usize,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RealScreenManifest {
    schema_version: u32,
    source: String,
    capture_status: String,
    label_contract: IndependentLabelCriteria,
    records: Vec<RealScreenRecord>,
}

#[derive(Clone, Debug, Deserialize)]
struct CoordinatorScreenRecord {
    index: usize,
    path: String,
    timestamp: String,
    app: String,
    title: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndependentLabelEntry {
    key: String,
    session: String,
    #[serde(
        default = "missing_independent_label",
        deserialize_with = "deserialize_independent_label"
    )]
    label: IndependentLabelField,
}

#[derive(Clone, Debug)]
enum IndependentLabelField {
    Missing,
    Null,
    Text(String),
}

fn missing_independent_label() -> IndependentLabelField {
    IndependentLabelField::Missing
}

fn deserialize_independent_label<'de, D>(deserializer: D) -> Result<IndependentLabelField, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
        .map(|label| label.map_or(IndependentLabelField::Null, IndependentLabelField::Text))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndependentLabelDocument {
    #[serde(alias = "labels")]
    annotations: Vec<IndependentLabelEntry>,
    #[serde(default)]
    annotator: Option<String>,
    #[serde(default)]
    question: Option<String>,
    #[serde(default, alias = "timestamp", alias = "annotated_at")]
    annotated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum IndependentLabelFile {
    Page(Vec<IndependentLabelEntry>),
    Document(IndependentLabelDocument),
}

#[derive(Clone, Debug)]
struct IndependentLabelImport {
    labels: BTreeMap<String, Option<String>>,
    format: String,
    annotator: Option<String>,
    question: Option<String>,
    annotated_at: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct IndependentLabelImportStats {
    missing_count: usize,
    null_count: usize,
}

const INDEPENDENT_LABEL_PAGE_QUESTION: &str = "この画面が出た時点で、Coo が声をかける価値があるか（前の画面との関係で判断。同じ作業の続きなら「不要」）";

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RateEvaluationManifest {
    schema_version: u32,
    canonical_data_root: String,
    canonical_pack: String,
    canonical_pack_manifest_sha256: String,
    criteria_file: String,
    criteria_sha256: String,
    fixtures_file: String,
    fixtures_sha256: String,
    retina_input_types: Vec<String>,
    retina_coordinate_rule: String,
    retina_unknown_side_policy: String,
    seed_schedule_canonical_bytes: String,
    seed_schedule_sha256: String,
}

struct EvaluationContext {
    manifest: RateEvaluationManifest,
    manifest_sha256: String,
    criteria: RateEvaluationCriteria,
    fixtures: FixtureManifest,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LearningBrainCriteria {
    id: String,
    role: String,
    seed: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LearningSeeds {
    base: u64,
    reward_shuffle: u64,
    wiring_shuffle: u64,
    weight_shuffle: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LearningAcceptanceCriteria {
    minimum_balanced_accuracy: f64,
    minimum_auroc: f64,
    minimum_consensus_accuracy: f64,
    minimum_unused_screen_accuracy: f64,
    consensus_margin: f64,
    saturation_is_reported_not_silently_ignored: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LearningResourceContract {
    maximum_peak_rss_bytes: u64,
    inference_target_peak_rss_bytes: u64,
    measurement: String,
    build_jobs: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LearningEvaluationCriteria {
    schema_version: u32,
    presentation_count: usize,
    pair_count: usize,
    train_pair_count: usize,
    tuning_pair_count: usize,
    evaluation_pair_count: usize,
    labeled_presentation_count: usize,
    held_out_pair_count: usize,
    input_order: String,
    unlabeled_side: String,
    labels: LearningLabelCriteria,
    recent_observations: usize,
    distance_metric: String,
    distance_threshold: f64,
    previous_absolute_difference_scale: f64,
    history_elapsed_time_scale_seconds: f64,
    training_mode: String,
    rate_steps_per_observation: usize,
    projection_dimension: usize,
    response_projection_dimension: usize,
    readout_feature_definition: String,
    readout_l2_regularization: f64,
    readout_learning_rate: f64,
    readout_reaction_threshold: f64,
    decision_threshold_selection: String,
    batch_epochs: usize,
    batch_learning_rates: Vec<f64>,
    batch_l2_values: Vec<f64>,
    minimum_near_neural_l2: f64,
    minimum_unrelated_image_count: usize,
    feature_separation: LearningFeatureSeparationCriteria,
    behavior: LearningBehaviorCriteria,
    behavior_acceptance: LearningBehaviorAcceptanceCriteria,
    case_memory: LearningCaseMemoryCriteria,
    real_screen: RealScreenEvaluationCriteria,
    brain_roles: Vec<LearningBrainCriteria>,
    human_feedback: Vec<LearningHumanFeedback>,
    seeds: LearningSeeds,
    acceptance: LearningAcceptanceCriteria,
    resource_contract: LearningResourceContract,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LearningLabelCriteria {
    automatic_teacher: String,
    independent: IndependentLabelCriteria,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct IndependentLabelCriteria {
    no_change: String,
    small_change: String,
    change: String,
    hold: String,
    positive_mapping: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LearningBehaviorCriteria {
    initial_input_id: String,
    repeat_input_id: String,
    different_input_id: String,
    rest_duration_s: f64,
    reversal_input_id: String,
    reversal_strength: f64,
    rest_protocol: String,
    long_repeat_count: usize,
    history_max_age_seconds: f64,
    food_branch_protocol: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LearningFeatureSeparationCriteria {
    minimum_pair_l2: f64,
    minimum_unrelated_to_near_p50_ratio: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LearningBehaviorAcceptanceCriteria {
    minimum_probability_margin: f64,
    minimum_rest_recovery_delta: f64,
    rest_recovery_rule: String,
    maximum_other_input_side_effect_delta: f64,
    require_food_action_change: bool,
    require_food_reaction_change: bool,
    require_reverse_food_change: bool,
    require_automatic_overwrite_check: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LearningCaseMemoryCriteria {
    distance_metric: String,
    distance_threshold: f64,
    match_policy: String,
    time_constant_seconds: f64,
    logit_scale: f64,
    max_cases: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RealScreenTrialCriteria {
    bootstrap_replicates: usize,
    bootstrap_seed: u64,
    ba_ci_half_width_target: f64,
    required_class_count_method: String,
    confidence_interval_method: String,
    minimum_total_record_count: usize,
    maximum_total_record_count: usize,
    maximum_hold_fraction: f64,
    expected_record_counts_by_split: BTreeMap<String, usize>,
    session_split_by_manifest: BTreeMap<String, String>,
    split_assignment: String,
    history_boundary: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RealScreenAdoptionCriteria {
    minimum_evaluation_record_count: usize,
    minimum_independent_positive_count_for_claim: usize,
    minimum_independent_negative_count_for_claim: usize,
    minimum_independent_positive_screen_unit_count_for_claim: usize,
    minimum_independent_negative_screen_unit_count_for_claim: usize,
    minimum_evaluation_session_count: usize,
    maximum_hold_fraction: f64,
    performance_condition: String,
    minimum_balanced_accuracy: f64,
    minimum_auroc: f64,
    minimum_paired_balanced_accuracy_difference_lower_bound: f64,
    paired_difference_rationale: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RealScreenEvaluationCriteria {
    trial: RealScreenTrialCriteria,
    adoption: RealScreenAdoptionCriteria,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LearningHumanFeedback {
    input_id: String,
    event_id: String,
    kind: String,
    strength: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LearningEvaluationManifest {
    schema_version: u32,
    canonical_pack: String,
    canonical_pack_manifest_sha256: String,
    base_evaluation_manifest: String,
    base_evaluation_manifest_sha256: String,
    criteria_file: String,
    criteria_sha256: String,
    fixtures_file: String,
    fixtures_sha256: String,
    seed_schedule_canonical_bytes: String,
    seed_schedule_sha256: String,
}

struct LearningEvaluationContext {
    manifest: LearningEvaluationManifest,
    manifest_sha256: String,
    criteria: LearningEvaluationCriteria,
    base: EvaluationContext,
}

#[derive(Clone, Debug, Serialize)]
struct LearningBrainResult {
    brain_id: String,
    role: String,
    balanced_accuracy: f64,
    auroc: f64,
    decision_threshold: f64,
    sample_count: usize,
    positive_count: usize,
    negative_count: usize,
    score_min: f64,
    score_max: f64,
    predicted_positive_count: usize,
    excluded_readiness_count: usize,
    true_positive: usize,
    false_positive: usize,
    true_negative: usize,
    false_negative: usize,
}

#[derive(Clone, Debug, Serialize)]
struct LearningConsensusResult {
    accuracy: f64,
    error_rate: f64,
    hold_rate: f64,
    sample_count: usize,
    novel_count: usize,
    habituated_count: usize,
    hold_count: usize,
    scored_sample_count: usize,
    readiness_insufficient_count: usize,
}

#[derive(Clone, Debug, Serialize)]
struct LearningConditionResult {
    name: String,
    brain_results: Vec<LearningBrainResult>,
    consensus: LearningConsensusResult,
    held_out_brain_results: Vec<LearningBrainResult>,
    held_out_consensus: LearningConsensusResult,
    unused_screen_accuracy: f64,
    training_presentations: usize,
    tuning_presentations: usize,
    evaluation_presentations: usize,
    held_out_presentations: usize,
    positive_training_events: usize,
    human_feedback_event_count: usize,
    wiring_changed_edge_count: usize,
    weight_changed_edge_count: usize,
    peak_rss_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
struct LearningEvaluationReport {
    manifest_sha256: String,
    criteria_sha256: String,
    base_evaluation_manifest_sha256: String,
    fixtures_sha256: String,
    pack_manifest_sha256: String,
    seed_schedule_sha256: String,
    input_order: String,
    distance_metric: String,
    distance_threshold: f64,
    acceptance: LearningAcceptanceCriteria,
    conditions: Vec<LearningConditionResult>,
    all_rate_values_finite: bool,
    peak_rss_bytes: Option<u64>,
    rss_measurement: String,
}

#[derive(Clone, Debug, Serialize)]
struct RealScreenConditionReport {
    name: String,
    feature_definition: String,
    feature_dimension: usize,
    training_sample_count: usize,
    tuning_sample_count: usize,
    evaluation_sample_count: usize,
    held_out_sample_count: usize,
    evaluation_total_count: usize,
    held_out_total_count: usize,
    evaluation_scored_count: usize,
    held_out_scored_count: usize,
    evaluation_hold_count: usize,
    held_out_hold_count: usize,
    evaluation_screen_unit_count: usize,
    held_out_screen_unit_count: usize,
    evaluation_positive_screen_unit_count: usize,
    evaluation_negative_screen_unit_count: usize,
    held_out_positive_screen_unit_count: usize,
    held_out_negative_screen_unit_count: usize,
    evaluated_count: usize,
    held_out_evaluated_count: usize,
    hold_count: usize,
    decision_threshold: Option<f64>,
    selection_split: String,
    balanced_accuracy: Option<f64>,
    balanced_accuracy_ci95: Option<[f64; 2]>,
    evaluation_bootstrap_effective_replicates: usize,
    evaluation_confidence_interval_method: String,
    auroc: Option<f64>,
    auroc_ci95: Option<[f64; 2]>,
    hold_as_error_accuracy: Option<f64>,
    held_out_balanced_accuracy: Option<f64>,
    held_out_balanced_accuracy_ci95: Option<[f64; 2]>,
    held_out_bootstrap_effective_replicates: usize,
    held_out_confidence_interval_method: String,
    held_out_auroc: Option<f64>,
    held_out_auroc_ci95: Option<[f64; 2]>,
    held_out_hold_as_error_accuracy: Option<f64>,
    true_positive: usize,
    false_positive: usize,
    true_negative: usize,
    false_negative: usize,
    predicted_positive_count: usize,
    predicted_positive_count_including_hold: usize,
    label_counts: BTreeMap<String, usize>,
    required_independent_positive_count: usize,
    required_independent_negative_count: usize,
    required_independent_positive_screen_unit_count: usize,
    required_independent_negative_screen_unit_count: usize,
    statistical_power_status: String,
    error: Option<String>,
    #[serde(skip)]
    pairing_samples: Vec<IndependentFeatureSample>,
    #[serde(skip)]
    pairing_scores: Vec<f64>,
    #[serde(skip)]
    pairing_labels: Vec<bool>,
}

#[derive(Clone, Debug, Serialize)]
struct RealScreenPairedComparisonReport {
    cns_condition: String,
    retina_condition: String,
    evaluation_sample_count: usize,
    cns_balanced_accuracy: Option<f64>,
    retina_balanced_accuracy: Option<f64>,
    paired_balanced_accuracy_difference: Option<f64>,
    ci95: Option<[f64; 2]>,
    bootstrap_effective_replicates: usize,
    confidence_interval_method: String,
    minimum_lower_bound: f64,
    criteria_met: bool,
    rationale: String,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct RealScreenStageTimingReport {
    name: String,
    sample_count: usize,
    p50_ms: f64,
    p95_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
struct RealScreenPerformanceReport {
    sample_count: usize,
    cache_mode: String,
    cache_hit_count: usize,
    cache_miss_count: usize,
    p50_ms: f64,
    p95_ms: f64,
    stages: Vec<RealScreenStageTimingReport>,
}

#[derive(Clone, Debug, Serialize)]
struct RealScreenSplitAcceptanceReport {
    expected_record_count: usize,
    actual_record_count: usize,
    record_count_matches: bool,
    scored_record_count: usize,
    positive_count: usize,
    negative_count: usize,
    positive_screen_unit_count: usize,
    negative_screen_unit_count: usize,
    hold_count: usize,
    hold_fraction: f64,
    hold_fraction_definition: String,
    hold_fraction_meets_criteria: bool,
    class_count_meets_criteria: bool,
    screen_unit_count_meets_criteria: bool,
    accepted: bool,
}

#[derive(Clone, Debug, Serialize)]
struct RealScreenEvaluationReport {
    report_version: u32,
    status: String,
    reason: Option<String>,
    evaluation_stage: String,
    trial_evaluation_possible: bool,
    adoption_criteria_met: bool,
    calculation_status: String,
    data_sufficiency_status: String,
    data_sufficiency_criteria_met: bool,
    performance_status: String,
    performance_criteria_met: bool,
    trial_criteria: RealScreenTrialCriteria,
    adoption_criteria: RealScreenAdoptionCriteria,
    adoption_effort_estimate: String,
    manifest_sha256: String,
    input_manifest_paths: Vec<String>,
    input_manifest_sha256s: Vec<String>,
    normalized_manifest_path: String,
    normalized_manifest_sha256: String,
    independent_labels_path: Option<String>,
    independent_labels_sha256: Option<String>,
    independent_label_format: Option<String>,
    independent_label_missing_count: usize,
    independent_label_null_count: usize,
    independent_label_annotator: Option<String>,
    independent_label_question: Option<String>,
    independent_label_annotated_at: Option<String>,
    independent_label_file_modified_at_s: Option<f64>,
    independent_label_metadata_status: String,
    session_split_assignment: BTreeMap<String, String>,
    session_split_assignment_sha256: String,
    criteria_sha256: String,
    pack_manifest_sha256: String,
    record_count: usize,
    labeled_record_count: usize,
    scored_record_count: usize,
    hold_record_count: usize,
    split_counts: BTreeMap<String, usize>,
    independent_label_counts: BTreeMap<String, usize>,
    split_label_counts: BTreeMap<String, BTreeMap<String, usize>>,
    split_acceptance: BTreeMap<String, RealScreenSplitAcceptanceReport>,
    excluded_record_count: usize,
    total_record_count_within_criteria: bool,
    split_structure_valid: bool,
    split_counts_meet_criteria: bool,
    split_acceptance_meets_criteria: bool,
    hold_fraction_meets_criteria: bool,
    hold_fraction_definition: String,
    independent_annotation_status: String,
    input_contract: String,
    split_contract: String,
    learning_rates: Vec<f64>,
    l2_values: Vec<f64>,
    conditions: Vec<RealScreenConditionReport>,
    cns_vs_retina_paired_comparison: Option<RealScreenPairedComparisonReport>,
    case_memory_calibration: Option<RealScreenCaseMemoryCalibration>,
    performance: Option<RealScreenPerformanceReport>,
    rss_measurement: String,
    peak_rss_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
struct RealScreenCaseMemoryCalibration {
    distance_metric: String,
    screen_unit: String,
    standardization: String,
    same_screen_pair_count: usize,
    different_screen_pair_count: usize,
    same_screen_distance_p50: Option<f64>,
    same_screen_distance_p95: Option<f64>,
    different_screen_distance_min: Option<f64>,
    different_screen_distance_p05: Option<f64>,
    different_screen_distance_p50: Option<f64>,
    selected_threshold: Option<f64>,
    same_screen_within_threshold_fraction: Option<f64>,
    different_screen_within_threshold_fraction: Option<f64>,
    neighbors_are_same_screen_only: bool,
    exact_image_hash_primary_key: bool,
    runtime_distance_threshold: f64,
    selected_threshold_role: String,
    runtime_boundary_verified: bool,
    calibration_splits: Vec<String>,
    held_out_same_hash_pair_count: usize,
    held_out_different_hash_near_count: usize,
}

#[derive(Clone, Debug)]
struct RealScreenObservation {
    record: RealScreenRecord,
    independent_label: Option<bool>,
    current_features: Vec<f64>,
    response_features: Vec<f64>,
    shuffled_response_features: Vec<f64>,
    history_features: Vec<f64>,
    retina_features: Vec<f64>,
    distance: Option<f64>,
}

#[derive(Clone, Debug)]
struct IndependentFeatureSample {
    split: String,
    screen_unit: String,
    record_id: String,
    image_sha256: String,
    label: Option<bool>,
    features: Vec<f64>,
}

type RealScreenSelected = (Vec<f64>, f64, f64, f64, (f64, f64, f64));

#[derive(Clone, Debug)]
struct CachedResponse {
    features: Vec<f32>,
    raw_norm: f64,
    delta_norm: f64,
    scaled_delta_norm: f64,
    active_fraction: f64,
    saturation_fraction: f64,
    components: Vec<RateResponseComponentReport>,
}

#[derive(Clone, Debug)]
struct CachedStimulus {
    image: habitua_connectome::RgbImage,
    neural_input: Vec<f32>,
    response: CachedResponse,
}

#[derive(Clone, Debug, Serialize)]
struct CachedResponseCacheReport {
    response_definition: String,
    initial_state_definition: String,
    update_count: usize,
    baseline_forward_count: usize,
    unique_input_count: usize,
    cache_hit_count: usize,
    cache_miss_count: usize,
    response_feature_dimension: usize,
    response_projection_dimension: usize,
    response_groups: Vec<RateResponseGroupReport>,
    response_component_names: Vec<String>,
    baseline_elapsed_ms: f64,
    rate_forward_p50_ms: f64,
    rate_forward_p95_ms: f64,
    uncached_response_p50_ms: f64,
    uncached_response_p95_ms: f64,
    cached_feature_lookup_p50_ms: f64,
    cached_feature_lookup_p95_ms: f64,
    rate_forward_peak_rss_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
struct ThreePointPairReport {
    id: String,
    kind: ScreenFixtureKind,
    image_l2_distance: f64,
    image_cosine_similarity: f64,
    neural_input_l2_distance: f64,
    neural_input_cosine_similarity: f64,
    neural_nonzero_difference_count: usize,
    downstream_response_l2_distance: f64,
    downstream_response_cosine_similarity: f64,
    downstream_nonzero_difference_count: usize,
    left_raw_response_norm: f64,
    right_raw_response_norm: f64,
    left_delta_response_norm: f64,
    right_delta_response_norm: f64,
    left_response_norm: f64,
    right_response_norm: f64,
    left_active_fraction: f64,
    right_active_fraction: f64,
    left_saturation_fraction: f64,
    right_saturation_fraction: f64,
    left_zero_component_count: usize,
    right_zero_component_count: usize,
    left_response_zero: bool,
    right_response_zero: bool,
}

#[derive(Clone, Debug, Serialize)]
struct ThreePointAuditReport {
    pair_count: usize,
    unique_image_count: usize,
    unrelated_unique_image_count: usize,
    near_neural_zero_pair_count: usize,
    near_downstream_zero_pair_count: usize,
    information_loss_counts: BTreeMap<String, usize>,
    response_feature_dimension: usize,
    feature_separation: FeatureSeparationReport,
    learning_feature_dimension: Option<usize>,
    learning_feature_separation: Option<FeatureSeparationReport>,
    passed: bool,
    pairs: Vec<ThreePointPairReport>,
}

#[derive(Clone, Debug, Serialize)]
struct FeatureSeparationReport {
    near_pair_count: usize,
    near_l2_min: f64,
    near_l2_p50: f64,
    near_l2_max: f64,
    near_cosine_min: f64,
    near_cosine_p50: f64,
    near_cosine_max: f64,
    unrelated_pair_count: usize,
    unrelated_l2_min: f64,
    unrelated_l2_p50: f64,
    unrelated_l2_max: f64,
    unrelated_cosine_min: f64,
    unrelated_cosine_p50: f64,
    unrelated_cosine_max: f64,
    unrelated_to_near_p50_l2_ratio: f64,
    passed: bool,
}

#[derive(Clone, Debug, Serialize)]
struct CachedBehaviorStepReport {
    name: String,
    input_id: String,
    stream_id: String,
    elapsed_seconds: f64,
    brain_change_probability: f64,
    brain_no_change_probability: f64,
    automatic_change: Option<bool>,
    readiness: String,
    consensus_action: String,
    /// Individual brain reactions retained for diagnostics only.
    active_reaction_count: usize,
    /// Reactions approved by BrainRunner consensus for external callers.
    external_reaction_count: usize,
}

#[derive(Clone, Debug, Serialize)]
struct CachedRestProbeReport {
    duration_seconds: f64,
    feature_distance: Option<f64>,
    previous_absolute_difference: Option<f64>,
    elapsed_seconds: Option<f64>,
    repeat_count: usize,
    history_active: bool,
    brain_change_probability: f64,
    brain_no_change_probability: f64,
    readiness: String,
    consensus_action: String,
}

#[derive(Clone, Debug, Serialize)]
struct CachedFoodReversalReport {
    input_id: String,
    change_probability_before: f64,
    change_probability_after: f64,
    no_change_probability_before: f64,
    no_change_probability_after: f64,
    consensus_before: String,
    consensus_after: String,
    reaction_count_before: usize,
    reaction_count_after: usize,
    reaction_signature_before: Vec<String>,
    reaction_signature_after: Vec<String>,
    diagnostic_reaction_count_before: usize,
    diagnostic_reaction_count_after: usize,
    diagnostic_reaction_signature_before: Vec<String>,
    diagnostic_reaction_signature_after: Vec<String>,
    selected_feedback_reversed: bool,
    changed: bool,
    feedback_commit_applied_before_re_evaluation: bool,
    same_start_state_and_timestamp: bool,
    no_food_control_change_probability: f64,
    no_food_control_consensus: String,
    no_food_control_reaction_signature: Vec<String>,
    no_food_control_probability_delta: f64,
    other_input_side_effect_probability_delta: f64,
    other_input_side_effect_action_changed: bool,
    reverse_feedback_changed: bool,
    automatic_overwrite_checked: bool,
    automatic_overwrite_blocked: bool,
    automatic_teacher_update_count_before: u64,
    automatic_teacher_update_count_after: u64,
    feedback_event_timestamp_s: f64,
    feedback_received_at_s: f64,
    feedback_commit_timestamp_s: f64,
    feedback_revision: u64,
    food_branch_start_timestamp_ms: u64,
    no_food_branch_start_timestamp_ms: u64,
    re_evaluation_timestamp_ms: u64,
    food_branch_start_state_sha256: String,
    no_food_branch_start_state_sha256: String,
    reverse_food_branch_start_state_sha256: String,
    pre_commit_state_sha256: String,
    post_commit_state_sha256: String,
    state_hash_changed_after_commit: bool,
    case_memory_match_count_before: usize,
    case_memory_match_count_after: usize,
    case_memory_match_count_other_input: usize,
    case_memory_nearest_distance_after: Option<f64>,
    case_memory_adjustment_change_after: f64,
    case_memory_adjustment_no_change_after: f64,
}

#[derive(Clone, Debug, Serialize)]
struct CachedBehaviorReport {
    steps: Vec<CachedBehaviorStepReport>,
    rest_probes: Vec<CachedRestProbeReport>,
    rest_history_expiry_verified: bool,
    food_reversal: CachedFoodReversalReport,
    repeat_change_probability: f64,
    different_change_probability: f64,
    after_rest_change_probability: f64,
    rest_recovery_probability_delta: f64,
    hold_rate: f64,
    rest_protocol: String,
    long_repeat_count: usize,
    history_max_age_seconds: f64,
    food_branch_protocol: String,
    acceptance: CachedBehaviorAcceptanceReport,
}

#[derive(Clone, Debug, Serialize)]
struct CachedBehaviorAcceptanceReport {
    initial_readiness_hold: bool,
    repeat_change_lower_than_initial: bool,
    no_change_retention: bool,
    different_change_higher_than_repeat: bool,
    rest_recovery: bool,
    rest_history_expiry_verified: bool,
    food_action_changed: bool,
    food_reaction_changed: bool,
    long_repeat_suppression: bool,
    rest_gap_was_unobserved: bool,
    food_no_food_control: bool,
    reverse_food: bool,
    no_other_input_side_effect: bool,
    automatic_overwrite_blocked: bool,
    all_passed: bool,
}

#[derive(Clone, Debug, Serialize)]
struct CachedLearningEvaluationReport {
    report_version: u32,
    manifest_sha256: String,
    criteria_sha256: String,
    base_evaluation_manifest_sha256: String,
    fixtures_sha256: String,
    pack_manifest_sha256: String,
    seed_schedule_sha256: String,
    response_cache: CachedResponseCacheReport,
    three_point_audit: ThreePointAuditReport,
    batch_training: BatchTrainingReport,
    learning_presentations: Vec<LearningPresentationReport>,
    conditions: Vec<LearningConditionResult>,
    behavior: CachedBehaviorReport,
    acceptance: LearningAcceptanceCriteria,
    all_rate_values_finite: bool,
    peak_rss_bytes: Option<u64>,
    helper_artifact_path: String,
    helper_artifact_sha256: String,
    rss_measurement: String,
    inference_scope: String,
    learning_scope: String,
}

#[derive(Clone, Debug, Serialize)]
struct LearningScheduleEntry {
    id: String,
    kind: ScreenFixtureKind,
    pair_index: usize,
    side: String,
    timestamp_s: f64,
    image_sha256: String,
    expected_change: bool,
}

#[derive(Clone, Debug, Serialize)]
struct LearningSeedSchedule {
    criteria_sha256: String,
    base_evaluation_manifest_sha256: String,
    presentation_count: usize,
    pair_count: usize,
    input_order: String,
    unlabeled_side: String,
    distance_metric: String,
    distance_threshold_bits: u64,
    previous_absolute_difference_scale_bits: u64,
    history_elapsed_time_scale_seconds_bits: u64,
    brain_roles: Vec<LearningBrainCriteria>,
    seeds: LearningSeeds,
    entries: Vec<LearningScheduleEntry>,
    training_entries: Vec<LearningScheduleEntry>,
    tuning_entries: Vec<LearningScheduleEntry>,
    evaluation_entries: Vec<LearningScheduleEntry>,
    held_out_entries: Vec<LearningScheduleEntry>,
}

#[derive(Clone, Debug)]
struct BatchSample {
    entry: LearningScheduleEntry,
    augmented_features: Vec<f64>,
    distance: Option<f64>,
    previous_absolute_difference: Option<f64>,
    automatic_change: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
struct LearningPresentationReport {
    split: String,
    id: String,
    pair_index: usize,
    side: String,
    distance: Option<f64>,
    distance_over_threshold: Option<f64>,
    previous_absolute_difference: Option<f64>,
    previous_absolute_difference_over_scale: Option<f64>,
    automatic_change: Option<bool>,
    expected_change: bool,
    automatic_expected_label_match: Option<bool>,
    readiness: String,
    selected_feedback: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
struct FeatureStandardization {
    mean: Vec<f64>,
    scale: Vec<f64>,
    zero_variance_count: usize,
}

#[derive(Clone, Debug)]
struct FittedBatchReadout {
    learning_rate: f64,
    l2: f64,
    weights: Vec<f64>,
    bias: f64,
}

type BatchReadoutFit = (FittedBatchReadout, Vec<BatchCandidateReport>, Vec<f64>);
type SelectedBatchReadout = (FittedBatchReadout, (f64, f64, f64), Vec<f64>);

#[derive(Clone, Debug, Serialize)]
struct BatchCandidateReport {
    role: String,
    learning_rate: f64,
    l2: f64,
    tuning_balanced_accuracy: f64,
    tuning_auroc: f64,
    tuning_log_loss: f64,
    selected: bool,
}

#[derive(Clone, Debug, Serialize)]
struct PredictionDistributionReport {
    role: String,
    sample_count: usize,
    minimum: f64,
    p10: f64,
    median: f64,
    p90: f64,
    maximum: f64,
    distinct_value_count: usize,
    constant_prediction: bool,
}

#[derive(Clone, Debug, Serialize)]
struct StandardizationReport {
    dimension: usize,
    zero_variance_count: usize,
    mean_min: f64,
    mean_max: f64,
    scale_min: f64,
    scale_max: f64,
    mean_sha256: String,
    scale_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
struct TeacherRuleReport {
    threshold: f64,
    training_match_count: usize,
    training_sample_count: usize,
    tuning_match_count: usize,
    tuning_sample_count: usize,
    evaluation_match_count: usize,
    evaluation_sample_count: usize,
    held_out_match_count: usize,
    held_out_sample_count: usize,
    training_fixture_label_match_count: usize,
    tuning_fixture_label_match_count: usize,
    evaluation_fixture_label_match_count: usize,
    held_out_fixture_label_match_count: usize,
    training_baseline_insufficient_count: usize,
    tuning_baseline_insufficient_count: usize,
    evaluation_baseline_insufficient_count: usize,
    held_out_baseline_insufficient_count: usize,
    training_automatic_expected_label_match_rate: f64,
    tuning_automatic_expected_label_match_rate: f64,
    evaluation_automatic_expected_label_match_rate: f64,
    held_out_automatic_expected_label_match_rate: f64,
    all_match: bool,
}

#[derive(Clone, Debug, Serialize)]
struct BatchConditionReport {
    name: String,
    epochs: usize,
    training_sample_count: usize,
    tuning_sample_count: usize,
    evaluation_sample_count: usize,
    held_out_sample_count: usize,
    decision_threshold: f64,
    candidates: Vec<BatchCandidateReport>,
    selected_learning_rates: BTreeMap<String, f64>,
    selected_l2_values: BTreeMap<String, f64>,
    evaluation_prediction_distributions: Vec<PredictionDistributionReport>,
    held_out_prediction_distributions: Vec<PredictionDistributionReport>,
    selected_readouts: Vec<RateHelperBrain>,
}

#[derive(Clone, Debug, Serialize)]
struct BatchTrainingReport {
    response_feature_dimension: usize,
    history_feature_dimension: usize,
    augmented_feature_dimension: usize,
    train_pair_count: usize,
    tuning_pair_count: usize,
    evaluation_pair_count: usize,
    held_out_pair_count: usize,
    epochs: usize,
    learning_rates: Vec<f64>,
    l2_values: Vec<f64>,
    standardization: StandardizationReport,
    teacher_rule: TeacherRuleReport,
    decision_threshold_selection: String,
    criteria_reaction_threshold: f64,
    conditions: Vec<BatchConditionReport>,
}

#[derive(Clone, Debug)]
struct BrainScores {
    expected_change: bool,
    change_score: f64,
    brain_change_score: f64,
    brain_no_change_score: f64,
    readiness_sufficient: bool,
}

#[derive(Clone, Debug)]
struct ResponseGroup {
    report: RateResponseGroupReport,
    indices: Vec<u32>,
}

#[derive(Clone, Debug)]
struct ResponseSnapshot {
    raw: Vec<f64>,
    delta: Vec<f64>,
    scaled_delta: Vec<f64>,
    raw_norm: f64,
    delta_norm: f64,
    scaled_delta_norm: f64,
    active_fraction: f64,
    saturation_fraction: f64,
    components: Vec<RateResponseComponentReport>,
}

type CachedResponseCacheBuild = (
    BTreeMap<String, CachedStimulus>,
    CachedResponseCacheReport,
    Vec<ResponseGroup>,
);

#[derive(Clone, Debug, Serialize)]
struct FeaturePairReport {
    id: String,
    kind: ScreenFixtureKind,
    l2_distance: f64,
    cosine_similarity: f64,
    direction_distance: f64,
    left_input_l1: f64,
    right_input_l1: f64,
    input_l1_relative_difference: f64,
    left_input_l2: f64,
    right_input_l2: f64,
    input_l2_relative_difference: f64,
}

#[derive(Clone, Debug, Serialize)]
struct RateInputAuditReport {
    manifest_sha256: String,
    criteria_sha256: String,
    fixtures_sha256: String,
    pack_manifest_sha256: String,
    seed_schedule_sha256: String,
    retina_audit: habitua_connectome::RetinaAudit,
    retina_contract_passed: bool,
    pair_count: usize,
    feature_vector_count: usize,
    exact_duplicate_pair_count: usize,
    minimum_distinct_l2_distance: f64,
    maximum_input_l1_relative_difference: f64,
    maximum_input_l2_relative_difference: f64,
    pairs: Vec<FeaturePairReport>,
    passed: bool,
}

#[derive(Clone, Debug, Serialize)]
struct RateFrozenPairReport {
    id: String,
    kind: ScreenFixtureKind,
    raw_cosine_similarity: f64,
    raw_l2_distance: f64,
    delta_cosine_similarity: f64,
    delta_l2_distance: f64,
    scaled_delta_cosine_similarity: f64,
    scaled_delta_l2_distance: f64,
    cosine_similarity: f64,
    l2_distance: f64,
}

#[derive(Clone, Debug, Serialize)]
struct RateFrozenReport {
    pair_count: usize,
    simulation_count: usize,
    aa_p10_cosine: Option<f64>,
    near_p50_cosine: Option<f64>,
    ab_p90_cosine: Option<f64>,
    raw_aa_p10_cosine: Option<f64>,
    raw_near_p50_cosine: Option<f64>,
    raw_ab_p90_cosine: Option<f64>,
    delta_aa_p10_cosine: Option<f64>,
    delta_near_p50_cosine: Option<f64>,
    delta_ab_p90_cosine: Option<f64>,
    scaled_delta_aa_p10_cosine: Option<f64>,
    scaled_delta_near_p50_cosine: Option<f64>,
    scaled_delta_ab_p90_cosine: Option<f64>,
    all_response_valid: bool,
    all_activity_ok: bool,
    all_saturation_ok: bool,
    passed: bool,
    pairs: Vec<RateFrozenPairReport>,
}

#[derive(Clone, Debug, Serialize)]
struct RateResponseGroupReport {
    name: String,
    population_names: Vec<String>,
    neuron_count: usize,
    rest_l2_norm: f64,
    scale: f64,
}

#[derive(Clone, Debug, Serialize)]
struct RateResponseComponentReport {
    name: String,
    raw_norm: f64,
    delta_norm: f64,
    scaled_delta_norm: f64,
    active_fraction: f64,
    saturation_fraction: f64,
    zero_response: bool,
}

#[derive(Clone, Debug, Serialize)]
struct RateObservationReport {
    id: String,
    kind: ScreenFixtureKind,
    elapsed_ms: f64,
    response_norm: f64,
    active_readout_fraction: f64,
    raw_response_norm: f64,
    delta_response_norm: f64,
    scaled_delta_response_norm: f64,
    active_response_fraction: f64,
    saturation_fraction: f64,
    components: Vec<RateResponseComponentReport>,
    value_probability: f64,
}

#[derive(Clone, Debug, Serialize)]
struct RateHabituationReport {
    first_novelty: Option<f64>,
    repeated_novelty: Option<f64>,
    recovered_novelty: Option<f64>,
    repeated_activity_norm: f64,
    recovered_activity_norm: f64,
    value_probability_before: f64,
    value_probability_after: f64,
    value_change: f64,
    response_sufficient: bool,
}

#[derive(Clone, Debug, Serialize)]
struct RateLearningPhaseReport {
    phase: String,
    input_id: String,
    target: f64,
    probability_before: f64,
    probability_after: f64,
    loss: f64,
    updated: bool,
    gradient_nonzero_log_gain_fraction: f64,
}

#[derive(Clone, Debug, Serialize)]
struct RateControlReport {
    feedback_source_count: usize,
    feedback_shuffled_match_count: usize,
    wiring_attempts: usize,
    wiring_successes: usize,
    wiring_rejected: usize,
    wiring_original_edge_overlap: f64,
    wiring_changed_edge_count: usize,
    weight_changed_edge_count: usize,
}

#[derive(Clone, Debug, Serialize)]
struct RatePerformanceReport {
    inference_sample_count: usize,
    inference_p50_ms: f64,
    inference_p95_ms: f64,
    learning_sample_count: usize,
    learning_p50_ms: f64,
    learning_p95_ms: f64,
    peak_rss_bytes: Option<u64>,
    inference_scope: String,
    learning_scope: String,
    stages: Vec<RateStagePerformanceReport>,
    rate_operation_breakdown: RateOperationBreakdown,
    rss_measurement: String,
}

#[derive(Clone, Debug, Serialize)]
struct RateStagePerformanceReport {
    name: String,
    sample_count: usize,
    p50_ms: f64,
    p95_ms: f64,
    peak_rss_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
struct RateOperationBreakdown {
    sample_count: usize,
    csr_traversal_ms: f64,
    neuron_update_ms: f64,
    other_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
struct RateEvaluationReport {
    manifest_sha256: String,
    criteria_sha256: String,
    fixtures_sha256: String,
    pack_manifest_sha256: String,
    seed_schedule_sha256: String,
    evaluation_manifest_sha256: String,
    rest_steps: usize,
    rest_convergence_max_abs: f64,
    retina_audit: habitua_connectome::RetinaAudit,
    retina_contract_passed: bool,
    response_groups: Vec<RateResponseGroupReport>,
    numerical: RateNumericalReport,
    input_audit_passed: bool,
    frozen: RateFrozenReport,
    habituation: RateHabituationReport,
    learning: Vec<RateLearningPhaseReport>,
    controls: RateControlReport,
    performance: RatePerformanceReport,
    observations: Vec<RateObservationReport>,
}

#[derive(Clone, Debug, Serialize)]
struct RateNumericalReport {
    f32_reference_abs_tolerance: f64,
    finite_difference_abs_tolerance: f64,
    nonlinear_boundary_margin: f64,
    all_rate_values_finite: bool,
    graph_fingerprint_before_learning: String,
    graph_fingerprint_after_learning: String,
    graph_structure_unchanged: bool,
    maximum_nonzero_edge_gradient_fraction: f64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let command = arguments.next().ok_or("a subcommand is required")?;
    let values = parse_values(arguments)?;
    match command.as_str() {
        "generate-fixtures" => generate_fixtures(&values),
        "generate-learning-manifest" => generate_learning_manifest(&values),
        "capture-real-screens" => capture_real_screens(&values),
        "real-screen-evaluate" => real_screen_evaluate(&values),
        "audit" => audit(&values),
        "evaluate" => evaluate(&values),
        "learning-evaluate" => learning_evaluate(&values),
        "learning-evaluate-legacy" => learning_evaluate_legacy(&values),
        _ => Err(format!("unknown subcommand {command}").into()),
    }
}

fn generate_fixtures(values: &BTreeMap<String, String>) -> Result<(), Box<dyn Error>> {
    let criteria_path = required_path(values, "--criteria")?;
    let output_dir = required_path(values, "--output-dir")?;
    let fixtures_path = required_path(values, "--fixtures-manifest")?;
    let evaluation_manifest_path = required_path(values, "--evaluation-manifest")?;
    let pack_path = required_path(values, "--pack")?;
    let data_root = required_path(values, "--data-root")?;
    reject_unknown(
        values,
        &[
            "--criteria",
            "--output-dir",
            "--fixtures-manifest",
            "--evaluation-manifest",
            "--pack",
            "--data-root",
        ],
    )?;
    let criteria_bytes = fs::read(&criteria_path)?;
    let criteria: RateEvaluationCriteria = serde_json::from_slice(&criteria_bytes)?;
    validate_criteria(&criteria)?;
    fs::create_dir_all(&output_dir)?;
    let kinds = [
        (ScreenFixtureKind::Same, "same"),
        (ScreenFixtureKind::Near, "near"),
        (ScreenFixtureKind::Unrelated, "unrelated"),
    ];
    let mut records = Vec::new();
    for (kind, kind_name) in kinds {
        for pair_index in 0..criteria.pair_count {
            for side in ["a", "b"] {
                let fixture = ScreenFixture::generate(
                    kind,
                    pair_index,
                    side,
                    criteria.width,
                    criteria.height,
                )?;
                let id = format!("{kind_name}-{pair_index:02}-{side}");
                let path = output_dir.join(format!("{id}.ppm"));
                fixture.write_ppm(&path)?;
                records.push(FixtureRecord {
                    id,
                    kind,
                    pair_index,
                    side: side.to_owned(),
                    path: path.to_string_lossy().into_owned(),
                    width: fixture.width,
                    height: fixture.height,
                    sha256: fixture.sha256,
                });
            }
        }
    }
    let fixture_manifest = FixtureManifest {
        schema_version: RATE_EVALUATION_SCHEMA_VERSION,
        width: criteria.width,
        height: criteria.height,
        pair_count: criteria.pair_count,
        records,
    };
    write_json(&fixtures_path, &fixture_manifest)?;
    let fixtures_sha256 = sha256_bytes(&fs::read(&fixtures_path)?);
    let pack_manifest_path = pack_path.join("rate_manifest.json");
    let pack_manifest_bytes = fs::read(&pack_manifest_path)?;
    let schedule = canonical_schedule(&criteria, &fixture_manifest)?;
    let manifest = RateEvaluationManifest {
        schema_version: RATE_EVALUATION_SCHEMA_VERSION,
        canonical_data_root: data_root.to_string_lossy().into_owned(),
        canonical_pack: pack_path.to_string_lossy().into_owned(),
        canonical_pack_manifest_sha256: sha256_bytes(&pack_manifest_bytes),
        criteria_file: criteria_path
            .file_name()
            .ok_or("criteria path has no file name")?
            .to_string_lossy()
            .into_owned(),
        criteria_sha256: sha256_bytes(&criteria_bytes),
        fixtures_file: fixtures_path.to_string_lossy().into_owned(),
        fixtures_sha256,
        retina_input_types: criteria.input_types.clone(),
        retina_coordinate_rule: criteria.retina.coordinate_rule.clone(),
        retina_unknown_side_policy: criteria.retina.unknown_side_policy.clone(),
        seed_schedule_sha256: sha256_bytes(&schedule),
        seed_schedule_canonical_bytes: String::from_utf8(schedule)?,
    };
    write_json(&evaluation_manifest_path, &manifest)?;
    Ok(())
}

fn generate_learning_manifest(values: &BTreeMap<String, String>) -> Result<(), Box<dyn Error>> {
    let criteria_path = required_path(values, "--criteria")?;
    let base_manifest_path = required_path(values, "--evaluation-manifest")?;
    let output_path = required_path(values, "--output")?;
    reject_unknown(values, &["--criteria", "--evaluation-manifest", "--output"])?;
    let criteria_bytes = fs::read(&criteria_path)?;
    let criteria: LearningEvaluationCriteria = serde_json::from_slice(&criteria_bytes)?;
    validate_learning_criteria(&criteria)?;
    let base = read_context(&base_manifest_path)?;
    if base.fixtures.pair_count != criteria.pair_count
        || base.fixtures.records.len() != criteria.presentation_count
    {
        return Err("learning criteria and canonical fixtures have different sizes".into());
    }
    let base_manifest_bytes = fs::read(&base_manifest_path)?;
    let base_manifest_sha256 = sha256_bytes(&base_manifest_bytes);
    let schedule = learning_schedule(&criteria, &base_manifest_sha256, &base.fixtures)?;
    validate_learning_split_image_hashes(&schedule)?;
    let schedule_bytes = serde_json::to_vec(&schedule)?;
    let pack_manifest_path = Path::new(&base.manifest.canonical_pack).join("rate_manifest.json");
    let manifest = LearningEvaluationManifest {
        schema_version: RATE_EVALUATION_SCHEMA_VERSION,
        canonical_pack: base.manifest.canonical_pack.clone(),
        canonical_pack_manifest_sha256: base.manifest.canonical_pack_manifest_sha256.clone(),
        base_evaluation_manifest: base_manifest_path.to_string_lossy().into_owned(),
        base_evaluation_manifest_sha256: base_manifest_sha256,
        criteria_file: criteria_path.to_string_lossy().into_owned(),
        criteria_sha256: sha256_bytes(&criteria_bytes),
        fixtures_file: base.manifest.fixtures_file.clone(),
        fixtures_sha256: base.manifest.fixtures_sha256.clone(),
        seed_schedule_canonical_bytes: String::from_utf8(schedule_bytes.clone())?,
        seed_schedule_sha256: sha256_bytes(&schedule_bytes),
    };
    if sha256_bytes(&fs::read(pack_manifest_path)?) != manifest.canonical_pack_manifest_sha256 {
        return Err(
            "canonical rate pack manifest hash changed while preparing learning manifest".into(),
        );
    }
    write_json(&output_path, &manifest)
}

fn capture_real_screens(values: &BTreeMap<String, String>) -> Result<(), Box<dyn Error>> {
    let output_dir = required_path(values, "--output-dir")?;
    let manifest_path = required_path(values, "--manifest")?;
    let count = values
        .get("--count")
        .map_or(Ok(60), |value| value.parse::<usize>())?;
    let interval_seconds = values
        .get("--interval-seconds")
        .map_or(Ok(1.0), |value| value.parse::<f64>())?;
    reject_unknown(
        values,
        &[
            "--output-dir",
            "--manifest",
            "--count",
            "--interval-seconds",
        ],
    )?;
    if count == 0 || !interval_seconds.is_finite() || interval_seconds < 0.0 {
        return Err("screen capture count and interval are invalid".into());
    }
    fs::create_dir_all(&output_dir)?;
    let mut records = Vec::with_capacity(count);
    let mut capture_status = "complete".to_owned();
    for index in 0..count {
        let captured_at_s = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| format!("system clock is before UNIX epoch: {error}"))?
            .as_secs_f64();
        let id = format!("screen-{index:04}");
        let path = output_dir.join(format!("{id}.ppm"));
        let result = Command::new("/usr/sbin/screencapture")
            .args(["-x", "-t", "ppm"])
            .arg(&path)
            .output()?;
        if !result.status.success() {
            let detail = String::from_utf8_lossy(&result.stderr).trim().to_owned();
            capture_status = if detail.is_empty() {
                format!("failed: screencapture exited with {}", result.status)
            } else {
                format!("failed: {detail}")
            };
            break;
        }
        let image = habitua_connectome::RgbImage::read_ppm(&path)?;
        records.push(RealScreenRecord {
            id,
            path: path.to_string_lossy().into_owned(),
            captured_at_s,
            stream_id: "screen-capture".to_owned(),
            split: "unassigned".to_owned(),
            app: String::new(),
            title: String::new(),
            independent_label: "unlabeled".to_owned(),
            label_reason: "人が画面内容を確認して付与する".to_owned(),
            width: image.width,
            height: image.height,
            sha256: habitua_connectome::image_sha256(&image),
        });
        if index + 1 < count && interval_seconds > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(interval_seconds));
        }
    }
    write_json(
        &manifest_path,
        &RealScreenManifest {
            schema_version: RATE_EVALUATION_SCHEMA_VERSION,
            source: "macOS screencapture -x -t ppm".to_owned(),
            capture_status: capture_status.clone(),
            label_contract: independent_label_contract(),
            records,
        },
    )?;
    if capture_status != "complete" {
        return Err(capture_status.into());
    }
    Ok(())
}

fn independent_label_contract() -> IndependentLabelCriteria {
    IndependentLabelCriteria {
        no_change: "human_notification_label_no".to_owned(),
        small_change: "human_notification_label_uncertain".to_owned(),
        change: "human_notification_label_yes".to_owned(),
        hold: "unannotated_or_technical_exclusion_is_hold".to_owned(),
        positive_mapping:
            "human_yes_positive_uncertain_reported_separately_hold_is_not_removed_from_denominator"
                .to_owned(),
    }
}

fn read_real_screen_manifest(path: &Path) -> Result<(Vec<u8>, RealScreenManifest), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let stream_prefix = path.parent().and_then(Path::file_name).map_or_else(
        || "screen-manifest".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    if let Ok(mut manifest) = serde_json::from_slice::<RealScreenManifest>(&bytes) {
        normalize_real_screen_records(
            &mut manifest.records,
            &stream_prefix,
            path.parent().unwrap_or_else(|| Path::new(".")),
        );
        assign_real_screen_splits(&mut manifest.records);
        return Ok((bytes, manifest));
    }
    let mut captures = Vec::new();
    for (line_number, line) in String::from_utf8(bytes.clone())?.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let capture: CoordinatorScreenRecord = serde_json::from_str(line).map_err(|error| {
            format!("screen JSONL line {} is invalid: {error}", line_number + 1)
        })?;
        captures.push(capture);
    }
    if captures.is_empty() {
        return Err("screen manifest is neither a JSON manifest nor non-empty JSONL".into());
    }
    captures.sort_by_key(|capture| capture.index);
    let mut records = Vec::with_capacity(captures.len());
    for capture in &captures {
        if capture.path.is_empty() || capture.app.is_empty() {
            return Err(format!(
                "screen JSONL record {} has an empty path or app",
                capture.index
            )
            .into());
        }
        let image_path = resolve_path(
            path.parent().unwrap_or_else(|| Path::new(".")),
            &capture.path,
        );
        let image = read_screen_image(&image_path)?;
        records.push(RealScreenRecord {
            id: format!("shot-{:03}", capture.index),
            path: image_path.to_string_lossy().into_owned(),
            captured_at_s: parse_capture_timestamp(&capture.timestamp).ok_or_else(|| {
                format!(
                    "screen JSONL record {} has an invalid timestamp {}",
                    capture.index, capture.timestamp
                )
            })?,
            stream_id: stream_prefix.clone(),
            split: "unassigned".to_owned(),
            app: capture.app.clone(),
            title: capture.title.clone(),
            independent_label: "unlabeled".to_owned(),
            label_reason: String::new(),
            width: image.width,
            height: image.height,
            sha256: habitua_connectome::image_sha256(&image),
        });
    }
    normalize_real_screen_records(
        &mut records,
        &stream_prefix,
        path.parent().unwrap_or_else(|| Path::new(".")),
    );
    assign_real_screen_splits(&mut records);
    let capture_status = path
        .parent()
        .map(|parent| parent.join("capture.err"))
        .filter(|status_path| status_path.exists())
        .and_then(|status_path| fs::read_to_string(status_path).ok())
        .filter(|status| status.lines().any(|line| line.trim() == "CAPTURE-DONE"))
        .map_or_else(|| "in_progress".to_owned(), |_| "complete".to_owned());
    Ok((
        bytes,
        RealScreenManifest {
            schema_version: RATE_EVALUATION_SCHEMA_VERSION,
            source: "coordinator manifest.jsonl; timestamps ordered at 30 seconds".to_owned(),
            capture_status,
            label_contract: independent_label_contract(),
            records,
        },
    ))
}

fn normalize_real_screen_records(
    records: &mut [RealScreenRecord],
    stream_prefix: &str,
    root: &Path,
) {
    records.sort_by(|left, right| {
        left.captured_at_s
            .total_cmp(&right.captured_at_s)
            .then_with(|| left.id.cmp(&right.id))
    });
    for record in records.iter_mut() {
        if !Path::new(&record.path).is_absolute() {
            record.path = root.join(&record.path).to_string_lossy().into_owned();
        }
        let original_stream = if record.stream_id.is_empty() {
            "screen".to_owned()
        } else {
            record.stream_id.clone()
        };
        record.stream_id = format!("{stream_prefix}:{original_stream}");
        record.id = format!("{stream_prefix}:{}", record.id);
        if record.app.starts_with("desktop_config_entry-") {
            record.independent_label = "hold".to_owned();
            record.label_reason = "technical_exclusion:temporary_test_window".to_owned();
        } else if !record.label_reason.starts_with("human:") {
            record.independent_label = "unlabeled".to_owned();
            record.label_reason = "independent_annotation_required".to_owned();
        }
    }
}

fn assign_real_screen_splits(records: &mut [RealScreenRecord]) {
    let split_names = ["training", "tuning", "evaluation", "held_out"];
    let mut session_splits = BTreeMap::<String, String>::new();
    // The caller concatenates manifests in the declared session order. Keep
    // that order instead of sorting by the filesystem-derived stream name;
    // otherwise screens-2 can become the training session merely because
    // its name sorts first.
    for record in records.iter_mut() {
        let split = if let Some(split) = session_splits.get(&record.stream_id) {
            split.clone()
        } else {
            let split_index = match session_splits.len() {
                0 => 0,
                // The second capture session is the independent evaluation
                // session. Tuning and held-out sessions stay empty until a
                // separately annotated capture is supplied.
                1 => 2,
                index => index.min(split_names.len() - 1),
            };
            let split = split_names[split_index].to_owned();
            session_splits.insert(record.stream_id.clone(), split.clone());
            split
        };
        record.split = split;
    }
}

fn assign_real_screen_splits_from_criteria(
    records: &mut [RealScreenRecord],
    session_split_by_manifest: &BTreeMap<String, String>,
) -> Result<(), String> {
    let valid_splits = ["training", "tuning", "evaluation", "held_out"];
    let mut seen_sessions = BTreeSet::new();
    for record in records {
        let manifest_name = record
            .stream_id
            .split_once(':')
            .map(|(manifest_name, _)| manifest_name)
            .ok_or_else(|| format!("record {} has no manifest session prefix", record.id))?;
        let split = session_split_by_manifest
            .get(manifest_name)
            .ok_or_else(|| format!("session {manifest_name} has no criteria split assignment"))?;
        if !valid_splits.contains(&split.as_str()) {
            return Err(format!(
                "session {manifest_name} has unsupported criteria split {split}"
            ));
        }
        seen_sessions.insert(manifest_name.to_owned());
        record.split = split.clone();
    }
    let unknown_sessions = session_split_by_manifest
        .keys()
        .filter(|session| !seen_sessions.contains(session.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown_sessions.is_empty() {
        return Err(format!(
            "criteria assigns sessions absent from the input manifests: {unknown_sessions:?}"
        ));
    }
    Ok(())
}

fn independent_label_key(record: &RealScreenRecord) -> Result<String, String> {
    let (session, local_id) = record
        .id
        .split_once(':')
        .ok_or_else(|| format!("record {} has no session prefix", record.id))?;
    let index = local_id
        .strip_prefix("shot-")
        .ok_or_else(|| format!("record {} has no shot index", record.id))?
        .parse::<usize>()
        .map_err(|error| format!("record {} has an invalid shot index: {error}", record.id))?;
    Ok(format!("{session}:{index}"))
}

fn apply_independent_labels(
    records: &mut [RealScreenRecord],
    import: &IndependentLabelImport,
) -> Result<IndependentLabelImportStats, String> {
    let labels = &import.labels;
    let known_keys = records
        .iter()
        .map(independent_label_key)
        .collect::<Result<BTreeSet<_>, _>>()?;
    if let Some(unknown_key) = labels.keys().find(|key| !known_keys.contains(key.as_str())) {
        return Err(format!(
            "independent label {unknown_key} does not identify a record in the input manifests"
        ));
    }
    let label_keys = labels.keys().cloned().collect::<BTreeSet<_>>();
    let missing_keys = known_keys
        .difference(&label_keys)
        .cloned()
        .collect::<Vec<_>>();
    if !missing_keys.is_empty() {
        return Err(format!(
            "independent label file is missing {} record keys; first missing key: {}",
            missing_keys.len(),
            missing_keys[0]
        ));
    }
    let mut stats = IndependentLabelImportStats::default();
    for record in records {
        let key = independent_label_key(record)?;
        let label = labels
            .get(&key)
            .expect("missing independent label keys were rejected above");
        let Some(label) = label else {
            stats.null_count += 1;
            if !record.label_reason.starts_with("technical_exclusion:") {
                record.independent_label = "unlabeled".to_owned();
                record.label_reason = "independent_annotation_null".to_owned();
            }
            continue;
        };
        match label.as_str() {
            "notify" => {
                record.independent_label = "change".to_owned();
                record.label_reason = "human:notify".to_owned();
            }
            "silence" => {
                record.independent_label = "no_change".to_owned();
                record.label_reason = "human:silence".to_owned();
            }
            "hold" => {
                record.independent_label = "hold".to_owned();
                record.label_reason = "human:hold".to_owned();
            }
            "exclude" => {
                record.independent_label = "hold".to_owned();
                record.label_reason = "technical_exclusion:human_exclude".to_owned();
            }
            other => {
                return Err(format!(
                    "independent label {key} has unsupported value {other}"
                ));
            }
        }
    }
    Ok(stats)
}

fn parse_independent_labels(bytes: &[u8]) -> Result<IndependentLabelImport, Box<dyn Error>> {
    let file: IndependentLabelFile = serde_json::from_slice(bytes)?;
    let (entries, format, annotator, question, annotated_at) = match file {
        IndependentLabelFile::Page(entries) => (
            entries,
            "annotation_page_array".to_owned(),
            None,
            None,
            None,
        ),
        IndependentLabelFile::Document(document) => (
            document.annotations,
            "annotation_document".to_owned(),
            document.annotator,
            document.question,
            document.annotated_at,
        ),
    };
    let mut labels = BTreeMap::new();
    for entry in entries {
        if entry.key.is_empty() || entry.session.is_empty() {
            return Err("independent label entries require non-empty key and session".into());
        }
        let (key_session, key_index) = entry
            .key
            .split_once(':')
            .ok_or_else(|| format!("independent label {} has no session prefix", entry.key))?;
        if key_session != entry.session || key_index.is_empty() {
            return Err(format!(
                "independent label {} session does not match its key",
                entry.key
            )
            .into());
        }
        let label = match entry.label {
            IndependentLabelField::Missing => {
                return Err(format!(
                    "independent label {} is missing the label field; use explicit null for unanswered",
                    entry.key
                )
                .into());
            }
            IndependentLabelField::Null => None,
            IndependentLabelField::Text(label) => Some(label),
        };
        if labels.insert(entry.key.clone(), label).is_some() {
            return Err(format!("duplicate independent label key {}", entry.key).into());
        }
        if let Some(label) = labels.get(&entry.key).and_then(Option::as_ref)
            && !matches!(label.as_str(), "notify" | "silence" | "hold" | "exclude")
        {
            return Err(format!(
                "independent label {} has unsupported value {}",
                entry.key, label
            )
            .into());
        }
    }
    Ok(IndependentLabelImport {
        labels,
        format,
        annotator,
        question,
        annotated_at,
    })
}

fn read_independent_labels(
    path: &Path,
) -> Result<(IndependentLabelImport, String), Box<dyn Error>> {
    let bytes = fs::read(path)?;
    Ok((parse_independent_labels(&bytes)?, sha256_bytes(&bytes)))
}

fn input_manifests_are_complete(paths: &[PathBuf]) -> Result<bool, Box<dyn Error>> {
    for path in paths {
        let (_, manifest) = read_real_screen_manifest(path)?;
        if manifest.capture_status != "complete" {
            return Ok(false);
        }
    }
    Ok(true)
}

fn read_screen_image(path: &Path) -> Result<habitua_connectome::RgbImage, Box<dyn Error>> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("png") => {
            Ok(habitua_connectome::RgbImage::read_png(path)?)
        }
        _ => Ok(habitua_connectome::RgbImage::read_ppm(path)?),
    }
}

fn parse_capture_timestamp(value: &str) -> Option<f64> {
    if value.len() != 15 || value.as_bytes().get(8) != Some(&b'T') {
        return None;
    }
    let year = value.get(..4)?.parse::<i64>().ok()?;
    let month = value.get(4..6)?.parse::<i64>().ok()?;
    let day = value.get(6..8)?.parse::<i64>().ok()?;
    let hour = value.get(9..11)?.parse::<f64>().ok()?;
    let minute = value.get(11..13)?.parse::<f64>().ok()?;
    let second = value.get(13..15)?.parse::<f64>().ok()?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    (hour < 24.0 && minute < 60.0 && second < 60.0).then_some(
        days_from_civil(year, month, day) as f64 * 86_400.0
            + hour * 3_600.0
            + minute * 60.0
            + second,
    )
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year / 400
    } else {
        (adjusted_year - 399) / 400
    };
    let year_of_era = adjusted_year - era * 400;
    let month_index = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn validate_real_screen_split_image_hashes(records: &[RealScreenRecord]) -> Result<(), String> {
    let mut hashes_by_split = BTreeMap::<&str, BTreeSet<&str>>::new();
    for record in records {
        hashes_by_split
            .entry(record.split.as_str())
            .or_default()
            .insert(record.sha256.as_str());
    }
    let split_names = hashes_by_split.keys().copied().collect::<Vec<_>>();
    for (left_index, left_name) in split_names.iter().enumerate() {
        for right_name in split_names.iter().skip(left_index + 1) {
            if let Some(hash) = hashes_by_split
                .get(left_name)
                .expect("real-screen split hash set exists")
                .intersection(
                    hashes_by_split
                        .get(right_name)
                        .expect("real-screen split hash set exists"),
                )
                .next()
            {
                return Err(format!(
                    "real-screen split image hash overlap: {left_name} and {right_name} share {hash}"
                ));
            }
        }
    }
    Ok(())
}

fn validate_real_screen_independent_labels(records: &[RealScreenRecord]) -> Result<(), String> {
    for record in records {
        if !matches!(
            record.independent_label.as_str(),
            "no_change" | "small_change" | "change" | "hold" | "unlabeled"
        ) {
            return Err(format!(
                "record {} has an unsupported independent label {}",
                record.id, record.independent_label
            ));
        }
        if matches!(record.independent_label.as_str(), "hold" | "unlabeled") {
            let valid_reason = record.label_reason.starts_with("human:")
                || record.label_reason == "independent_annotation_required"
                || record.label_reason == "independent_annotation_missing_key"
                || record.label_reason == "independent_annotation_null"
                || record.label_reason.starts_with("technical_exclusion:");
            if !valid_reason {
                return Err(format!(
                    "hold record {} lacks an independent annotation or technical exclusion reason code",
                    record.id
                ));
            }
        } else if !record.label_reason.starts_with("human:") {
            return Err(format!(
                "scored record {} is not marked as a human independent annotation",
                record.id
            ));
        }
    }
    Ok(())
}

fn validate_real_screen_split_counts(
    records: &[RealScreenRecord],
    criteria: &RealScreenTrialCriteria,
) -> Result<(), String> {
    let mut actual = BTreeMap::<String, usize>::new();
    for record in records {
        *actual.entry(record.split.clone()).or_default() += 1;
    }
    for split in criteria.expected_record_counts_by_split.keys() {
        actual.entry(split.clone()).or_default();
    }
    if actual != criteria.expected_record_counts_by_split {
        return Err(format!(
            "real screen split counts do not match criteria: expected {:?}, actual {:?}",
            criteria.expected_record_counts_by_split, actual
        ));
    }
    Ok(())
}

fn validate_real_screen_split_structure(records: &[RealScreenRecord]) -> Result<(), String> {
    let valid_splits = ["training", "tuning", "evaluation", "held_out"];
    if records.is_empty() {
        return Err("real screen manifest contains no records".to_owned());
    }
    if records.iter().any(|record| {
        !valid_splits.contains(&record.split.as_str())
            || record.stream_id.is_empty()
            || !record.captured_at_s.is_finite()
    }) {
        return Err("real screen records have an invalid session split or timestamp".to_owned());
    }
    let mut ids = BTreeSet::new();
    if let Some(record) = records.iter().find(|record| !ids.insert(&record.id)) {
        return Err(format!("duplicate real screen record id: {}", record.id));
    }
    let mut hashes_by_split = BTreeMap::<&str, BTreeSet<&str>>::new();
    for record in records {
        hashes_by_split
            .entry(record.split.as_str())
            .or_default()
            .insert(record.sha256.as_str());
    }
    let split_names = hashes_by_split.keys().copied().collect::<Vec<_>>();
    for (left_index, left_name) in split_names.iter().enumerate() {
        for right_name in split_names.iter().skip(left_index + 1) {
            if hashes_by_split
                .get(left_name)
                .expect("real-screen split exists")
                .intersection(
                    hashes_by_split
                        .get(right_name)
                        .expect("real-screen split exists"),
                )
                .next()
                .is_some()
            {
                return Err(format!(
                    "real screen split image hashes overlap: {left_name} and {right_name}"
                ));
            }
        }
    }
    Ok(())
}

fn real_screen_history_scope(record: &RealScreenRecord) -> String {
    format!("{}:{}", record.stream_id, record.split)
}

fn hold_record_count(records: &[RealScreenRecord]) -> usize {
    records
        .iter()
        .filter(|record| matches!(record.independent_label.as_str(), "hold" | "unlabeled"))
        .count()
}

fn real_screen_class_screen_unit_count(
    records: &[RealScreenRecord],
    split: &str,
    positive: bool,
) -> usize {
    records
        .iter()
        .filter(|record| {
            record.split == split
                && match record.independent_label.as_str() {
                    "change" | "small_change" => positive,
                    "no_change" => !positive,
                    _ => false,
                }
        })
        .map(|record| record.stream_id.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

#[allow(clippy::too_many_arguments)]
fn real_screen_data_sufficiency_status(
    evaluation_record_count: usize,
    positive_count: usize,
    negative_count: usize,
    positive_screen_unit_count: usize,
    negative_screen_unit_count: usize,
    evaluation_session_count: usize,
    hold_fraction: f64,
    total_count_within_criteria: bool,
    split_structure_valid: bool,
    criteria: &RealScreenAdoptionCriteria,
) -> (&'static str, bool) {
    let checks = [
        (split_structure_valid, "invalid_split_structure"),
        (
            total_count_within_criteria,
            "total_record_count_out_of_range",
        ),
        (
            evaluation_record_count >= criteria.minimum_evaluation_record_count,
            "insufficient_evaluation_record_count",
        ),
        (
            positive_count >= criteria.minimum_independent_positive_count_for_claim,
            "insufficient_independent_positive_count",
        ),
        (
            negative_count >= criteria.minimum_independent_negative_count_for_claim,
            "insufficient_independent_negative_count",
        ),
        (
            positive_screen_unit_count
                >= criteria.minimum_independent_positive_screen_unit_count_for_claim,
            "insufficient_positive_screen_unit_count",
        ),
        (
            negative_screen_unit_count
                >= criteria.minimum_independent_negative_screen_unit_count_for_claim,
            "insufficient_negative_screen_unit_count",
        ),
        (
            evaluation_session_count >= criteria.minimum_evaluation_session_count,
            "insufficient_evaluation_session_count",
        ),
        (
            hold_fraction <= criteria.maximum_hold_fraction,
            "independent_label_unknown_fraction_too_high",
        ),
    ];
    checks
        .iter()
        .find_map(|(passed, status)| (!passed).then_some((*status, false)))
        .unwrap_or(("data_sufficient_for_adoption", true))
}

fn real_screen_evaluate(values: &BTreeMap<String, String>) -> Result<(), Box<dyn Error>> {
    let learning_manifest_path = required_path(values, "--learning-manifest")?;
    let real_manifest_path = required_path(values, "--real-manifest")?;
    let output = required_path(values, "--output")?;
    let second_manifest_path = values
        .get("--real-manifest-2")
        .map(PathBuf::from)
        .or_else(|| {
            real_manifest_path
                .parent()
                .and_then(Path::parent)
                .map(|root| root.join("screens-2/manifest.jsonl"))
                .filter(|path| path.exists())
        });
    let labels_were_explicit = values.contains_key("--independent-labels");
    let independent_labels_path = values
        .get("--independent-labels")
        .map(PathBuf::from)
        .or_else(|| {
            real_manifest_path
                .parent()
                .and_then(Path::parent)
                .map(|root| root.join("independent-labels.json"))
                .filter(|path| path.exists())
        });
    reject_unknown(
        values,
        &[
            "--learning-manifest",
            "--real-manifest",
            "--real-manifest-2",
            "--independent-labels",
            "--output",
        ],
    )?;
    let context = read_learning_context(&learning_manifest_path)?;
    let mut input_manifest_paths = vec![real_manifest_path.clone()];
    if let Some(path) = second_manifest_path
        && path != real_manifest_path
    {
        input_manifest_paths.push(path);
    }
    let mut input_manifest_sha256s = Vec::with_capacity(input_manifest_paths.len());
    let mut raw_manifest_bytes = Vec::new();
    let mut input_manifests = Vec::with_capacity(input_manifest_paths.len());
    for path in &input_manifest_paths {
        let (bytes, manifest) = read_real_screen_manifest(path)?;
        input_manifest_sha256s.push(sha256_bytes(&bytes));
        raw_manifest_bytes.extend_from_slice(&bytes);
        raw_manifest_bytes.extend_from_slice(b"\n--manifest-boundary--\n");
        input_manifests.push(manifest);
    }
    let mut real_records = input_manifests
        .into_iter()
        .flat_map(|manifest| manifest.records)
        .collect::<Vec<_>>();
    let mut independent_label_import = None;
    let mut independent_label_import_stats = IndependentLabelImportStats::default();
    if let Some(labels_path) = &independent_labels_path {
        let (import, _) = read_independent_labels(labels_path)?;
        independent_label_import_stats = apply_independent_labels(&mut real_records, &import)?;
        independent_label_import = Some(import);
    } else if labels_were_explicit {
        return Err("explicit independent-labels path does not exist".into());
    }
    if independent_label_import.is_none() {
        independent_label_import_stats.missing_count = real_records
            .iter()
            .filter(|record| !record.label_reason.starts_with("technical_exclusion:"))
            .count();
    }
    assign_real_screen_splits_from_criteria(
        &mut real_records,
        &context.criteria.real_screen.trial.session_split_by_manifest,
    )?;
    let real_manifest = RealScreenManifest {
        schema_version: RATE_EVALUATION_SCHEMA_VERSION,
        source: "coordinator manifest(s); independent notification labels are human supplied; app/title transitions are diagnostic only"
            .to_owned(),
        capture_status: if input_manifests_are_complete(&input_manifest_paths)? {
            "complete".to_owned()
        } else {
            "in_progress".to_owned()
        },
        label_contract: independent_label_contract(),
        records: real_records,
    };
    let manifest_sha256 = sha256_bytes(&raw_manifest_bytes);
    let (independent_labels_path, independent_labels_sha256) =
        if let Some(path) = independent_labels_path {
            let bytes = fs::read(&path)?;
            (
                Some(path.to_string_lossy().into_owned()),
                Some(sha256_bytes(&bytes)),
            )
        } else {
            (None, None)
        };
    let independent_label_format = independent_label_import
        .as_ref()
        .map(|import| import.format.clone());
    let independent_label_annotator = independent_label_import
        .as_ref()
        .and_then(|import| import.annotator.clone());
    let independent_label_question = independent_label_import.as_ref().map(|import| {
        import
            .question
            .clone()
            .unwrap_or_else(|| INDEPENDENT_LABEL_PAGE_QUESTION.to_owned())
    });
    let independent_label_annotated_at = independent_label_import
        .as_ref()
        .and_then(|import| import.annotated_at.clone());
    let independent_label_file_modified_at_s = independent_labels_path
        .as_ref()
        .and_then(|path| fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs_f64());
    let independent_label_metadata_status = independent_label_import.as_ref().map_or_else(
        || "not_available".to_owned(),
        |import| {
            if import.annotator.is_some()
                || import.question.is_some()
                || import.annotated_at.is_some()
            {
                "provided_in_annotation_document".to_owned()
            } else {
                "annotation_page_array_without_metadata".to_owned()
            }
        },
    );
    let session_split_assignment = context
        .criteria
        .real_screen
        .trial
        .session_split_by_manifest
        .clone();
    let session_split_assignment_sha256 =
        sha256_bytes(&serde_json::to_vec(&session_split_assignment)?);
    let normalized_manifest_path = output.with_extension("normalized.json");
    write_json(&normalized_manifest_path, &real_manifest)?;
    let normalized_manifest_sha256 = sha256_bytes(&fs::read(&normalized_manifest_path)?);
    let mut split_counts = BTreeMap::new();
    let mut independent_label_counts = BTreeMap::new();
    let mut split_label_counts = BTreeMap::<String, BTreeMap<String, usize>>::new();
    for record in &real_manifest.records {
        *split_counts.entry(record.split.clone()).or_insert(0) += 1;
        *independent_label_counts
            .entry(record.independent_label.clone())
            .or_insert(0) += 1;
        *split_label_counts
            .entry(record.split.clone())
            .or_default()
            .entry(record.independent_label.clone())
            .or_insert(0) += 1;
    }
    let split_acceptance = context
        .criteria
        .real_screen
        .trial
        .expected_record_counts_by_split
        .keys()
        .map(|split| {
            let labels = split_label_counts.get(split);
            let actual_record_count = split_counts.get(split).copied().unwrap_or(0);
            let expected_record_count = context
                .criteria
                .real_screen
                .trial
                .expected_record_counts_by_split
                .get(split)
                .copied()
                .unwrap_or(0);
            let scored_record_count = labels
                .into_iter()
                .flat_map(|counts| counts.iter())
                .filter(|(label, _)| {
                    matches!(label.as_str(), "no_change" | "small_change" | "change")
                })
                .map(|(_, count)| *count)
                .sum::<usize>();
            let positive_count = labels
                .map(|counts| {
                    counts.get("small_change").copied().unwrap_or(0)
                        + counts.get("change").copied().unwrap_or(0)
                })
                .unwrap_or(0);
            let negative_count = labels
                .and_then(|counts| counts.get("no_change").copied())
                .unwrap_or(0);
            let positive_screen_unit_count =
                real_screen_class_screen_unit_count(&real_manifest.records, split, true);
            let negative_screen_unit_count =
                real_screen_class_screen_unit_count(&real_manifest.records, split, false);
            let hold_count = labels
                .map(|counts| {
                    counts.get("hold").copied().unwrap_or(0)
                        + counts.get("unlabeled").copied().unwrap_or(0)
                })
                .unwrap_or(0);
            let hold_fraction = hold_count as f64 / actual_record_count.max(1) as f64;
            let hold_fraction_meets_criteria =
                hold_fraction <= context.criteria.real_screen.trial.maximum_hold_fraction;
            let class_count_meets_criteria = if expected_record_count == 0 {
                true
            } else if matches!(split.as_str(), "evaluation" | "held_out") {
                positive_count
                    >= context
                        .criteria
                        .real_screen
                        .adoption
                        .minimum_independent_positive_count_for_claim
                    && negative_count
                        >= context
                            .criteria
                            .real_screen
                            .adoption
                            .minimum_independent_negative_count_for_claim
            } else {
                true
            };
            let screen_unit_count_meets_criteria = if expected_record_count == 0 {
                true
            } else if matches!(split.as_str(), "evaluation" | "held_out") {
                positive_screen_unit_count
                    >= context
                        .criteria
                        .real_screen
                        .adoption
                        .minimum_independent_positive_screen_unit_count_for_claim
                    && negative_screen_unit_count
                        >= context
                            .criteria
                            .real_screen
                            .adoption
                            .minimum_independent_negative_screen_unit_count_for_claim
            } else {
                true
            };
            (
                split.clone(),
                RealScreenSplitAcceptanceReport {
                    expected_record_count,
                    actual_record_count,
                    record_count_matches: expected_record_count == actual_record_count,
                    scored_record_count,
                    positive_count,
                    negative_count,
                    positive_screen_unit_count,
                    negative_screen_unit_count,
                    hold_count,
                    hold_fraction,
                    hold_fraction_definition:
                        "independent label unknown rate: null/missing/unresolved hold or technical exclusion; not helper readiness/action hold"
                            .to_owned(),
                    hold_fraction_meets_criteria,
                    class_count_meets_criteria,
                    screen_unit_count_meets_criteria,
                    accepted: expected_record_count == actual_record_count
                        && hold_fraction_meets_criteria
                        && class_count_meets_criteria
                        && screen_unit_count_meets_criteria,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    let split_counts_meet_criteria = validate_real_screen_split_counts(
        &real_manifest.records,
        &context.criteria.real_screen.trial,
    )
    .is_ok();
    let split_acceptance_meets_criteria = split_acceptance.values().all(|entry| entry.accepted);
    let excluded_record_count = real_manifest
        .records
        .iter()
        .filter(|record| record.label_reason.starts_with("technical_exclusion:"))
        .count();
    let independent_annotation_status = if real_manifest
        .records
        .iter()
        .any(|record| record.label_reason.starts_with("human:"))
    {
        "human_annotations_present"
    } else {
        "trial_pending_human_annotations"
    };
    let independent_label_unknown_count = hold_record_count(&real_manifest.records);
    let independent_label_unknown_fraction =
        independent_label_unknown_count as f64 / real_manifest.records.len().max(1) as f64;
    let adoption = &context.criteria.real_screen.adoption;
    let hold_fraction_meets_criteria =
        independent_label_unknown_fraction <= adoption.maximum_hold_fraction;
    let total_count_within_criteria = (context
        .criteria
        .real_screen
        .trial
        .minimum_total_record_count
        ..=context
            .criteria
            .real_screen
            .trial
            .maximum_total_record_count)
        .contains(&real_manifest.records.len());
    let split_structure_valid =
        validate_real_screen_split_structure(&real_manifest.records).is_ok();
    let has_independent_annotations = real_manifest
        .records
        .iter()
        .any(|record| record.label_reason.starts_with("human:"));
    let evaluation_records = real_manifest
        .records
        .iter()
        .filter(|record| record.split == "evaluation")
        .collect::<Vec<_>>();
    let evaluation_positive_count = evaluation_records
        .iter()
        .filter(|record| matches!(record.independent_label.as_str(), "small_change" | "change"))
        .count();
    let evaluation_negative_count = evaluation_records
        .iter()
        .filter(|record| record.independent_label == "no_change")
        .count();
    let evaluation_positive_screen_unit_count =
        real_screen_class_screen_unit_count(&real_manifest.records, "evaluation", true);
    let evaluation_negative_screen_unit_count =
        real_screen_class_screen_unit_count(&real_manifest.records, "evaluation", false);
    let evaluation_session_count = evaluation_records
        .iter()
        .map(|record| record.stream_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let evaluation_hold_count = evaluation_records
        .iter()
        .filter(|record| matches!(record.independent_label.as_str(), "hold" | "unlabeled"))
        .count();
    let evaluation_hold_fraction =
        evaluation_hold_count as f64 / evaluation_records.len().max(1) as f64;
    let (data_sufficiency_status, data_sufficiency_criteria_met) =
        real_screen_data_sufficiency_status(
            evaluation_records.len(),
            evaluation_positive_count,
            evaluation_negative_count,
            evaluation_positive_screen_unit_count,
            evaluation_negative_screen_unit_count,
            evaluation_session_count,
            evaluation_hold_fraction,
            total_count_within_criteria,
            split_structure_valid,
            adoption,
        );
    let adoption_criteria_met = false;
    let trial_evaluation_possible = has_independent_annotations
        && split_structure_valid
        && evaluation_positive_count > 0
        && evaluation_negative_count > 0
        && real_manifest
            .records
            .iter()
            .filter(|record| record.split == "training")
            .any(|record| record.independent_label == "no_change")
        && real_manifest
            .records
            .iter()
            .filter(|record| record.split == "training")
            .any(|record| matches!(record.independent_label.as_str(), "small_change" | "change"));
    let evaluation_stage = if trial_evaluation_possible {
        "trial"
    } else {
        "pending"
    };
    let additional_positive = adoption
        .minimum_independent_positive_count_for_claim
        .saturating_sub(evaluation_positive_count);
    let additional_negative = adoption
        .minimum_independent_negative_count_for_claim
        .saturating_sub(evaluation_negative_count);
    let additional_sessions = adoption
        .minimum_evaluation_session_count
        .saturating_sub(evaluation_session_count);
    let adoption_effort_estimate = format!(
        "追加の評価セッション最低{additional_sessions}、追加注釈最低{additional_positive}陽性+{additional_negative}陰性={total_annotations}件。現在の評価 split の実測値から計算し、ラベル到着後に再計算する",
        total_annotations = additional_positive.saturating_add(additional_negative),
    );
    let base_report = |status: &str, reason: Option<String>| {
        RealScreenEvaluationReport {
        report_version: 3,
        status: status.to_owned(),
        reason,
        evaluation_stage: evaluation_stage.to_owned(),
        trial_evaluation_possible,
        adoption_criteria_met,
        calculation_status: "not_started".to_owned(),
        data_sufficiency_status: data_sufficiency_status.to_owned(),
        data_sufficiency_criteria_met,
        performance_status: "not_evaluated".to_owned(),
        performance_criteria_met: false,
        trial_criteria: context.criteria.real_screen.trial.clone(),
        adoption_criteria: context.criteria.real_screen.adoption.clone(),
        adoption_effort_estimate: adoption_effort_estimate.clone(),
        manifest_sha256: manifest_sha256.clone(),
        input_manifest_paths: input_manifest_paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        input_manifest_sha256s: input_manifest_sha256s.clone(),
        normalized_manifest_path: normalized_manifest_path.to_string_lossy().into_owned(),
        normalized_manifest_sha256: normalized_manifest_sha256.clone(),
        independent_labels_path: independent_labels_path.clone(),
        independent_labels_sha256: independent_labels_sha256.clone(),
        independent_label_format: independent_label_format.clone(),
        independent_label_missing_count: independent_label_import_stats.missing_count,
        independent_label_null_count: independent_label_import_stats.null_count,
        independent_label_annotator: independent_label_annotator.clone(),
        independent_label_question: independent_label_question.clone(),
        independent_label_annotated_at: independent_label_annotated_at.clone(),
        independent_label_file_modified_at_s,
        independent_label_metadata_status: independent_label_metadata_status.clone(),
        session_split_assignment: session_split_assignment.clone(),
        session_split_assignment_sha256: session_split_assignment_sha256.clone(),
        criteria_sha256: context.manifest.criteria_sha256.clone(),
        pack_manifest_sha256: context.manifest.canonical_pack_manifest_sha256.clone(),
        record_count: real_manifest.records.len(),
        labeled_record_count: real_manifest
            .records
            .iter()
            .filter(|record| {
                matches!(
                    record.independent_label.as_str(),
                    "no_change" | "small_change" | "change"
                )
            })
            .count(),
        scored_record_count: real_manifest
            .records
            .iter()
            .filter(|record| matches!(record.independent_label.as_str(), "no_change" | "small_change" | "change"))
            .count(),
        hold_record_count: real_manifest
            .records
            .iter()
            .filter(|record| matches!(record.independent_label.as_str(), "hold" | "unlabeled"))
            .count(),
        split_counts: split_counts.clone(),
        independent_label_counts: independent_label_counts.clone(),
        split_label_counts: split_label_counts.clone(),
        split_acceptance: split_acceptance.clone(),
        excluded_record_count,
        total_record_count_within_criteria: total_count_within_criteria,
        split_structure_valid,
        split_counts_meet_criteria,
        split_acceptance_meets_criteria,
        hold_fraction_meets_criteria,
        hold_fraction_definition: "independent label unknown rate (null, missing, unresolved hold, or technical exclusion); helper readiness/action hold is not included in this offline label metric".to_owned(),
        independent_annotation_status: independent_annotation_status.to_owned(),
        input_contract: "real PNG/PPM -> fixed bilinear resize -> RetinaMap -> frozen CNS response; no response feature is reused between conditions".to_owned(),
        split_contract: "splits are assigned to whole capture sessions; history scope is stream_id plus split; training statistics and tuning threshold are not shared with evaluation or held_out".to_owned(),
        learning_rates: context.criteria.batch_learning_rates.clone(),
        l2_values: context.criteria.batch_l2_values.clone(),
        conditions: Vec::new(),
        cns_vs_retina_paired_comparison: None,
        case_memory_calibration: None,
        performance: None,
        rss_measurement: "/usr/bin/time -l の外部計測を併用し、内部には getrusage の累積 high-water を記録する".to_owned(),
        peak_rss_bytes: process_peak_rss_bytes(),
    }
    };
    let blocked_reason = if real_manifest.schema_version != RATE_EVALUATION_SCHEMA_VERSION {
        Some("real screen manifest schema version is unsupported".to_owned())
    } else if let Err(error) = validate_real_screen_split_image_hashes(&real_manifest.records) {
        Some(error)
    } else if let Err(error) = validate_real_screen_split_structure(&real_manifest.records) {
        Some(error)
    } else if real_manifest.capture_status != "complete" {
        Some(format!(
            "screen capture status is not complete: {}",
            real_manifest.capture_status
        ))
    } else if real_manifest.records.len()
        < context
            .criteria
            .real_screen
            .trial
            .minimum_total_record_count
        || real_manifest.records.len()
            > context
                .criteria
                .real_screen
                .trial
                .maximum_total_record_count
    {
        Some(format!(
            "独立ラベル評価の総レコード数がcriteria範囲外: {}〜{}",
            context
                .criteria
                .real_screen
                .trial
                .minimum_total_record_count,
            context
                .criteria
                .real_screen
                .trial
                .maximum_total_record_count
        ))
    } else if real_manifest.label_contract != context.criteria.labels.independent {
        Some("real screen label contract differs from criteria".to_owned())
    } else if real_manifest.records.iter().any(|record| {
        !matches!(
            record.independent_label.as_str(),
            "no_change" | "small_change" | "change" | "hold" | "unlabeled"
        ) || record.split == "unassigned"
            || record.stream_id.is_empty()
            || !record.captured_at_s.is_finite()
            || record.path.is_empty()
            || record.label_reason.is_empty()
    }) {
        Some("real screen records require an assigned split, timestamp, path, and independent label state".to_owned())
    } else if let Err(error) = validate_real_screen_independent_labels(&real_manifest.records) {
        Some(error)
    } else {
        let mut ids = BTreeSet::new();
        real_manifest
            .records
            .iter()
            .find(|record| !ids.insert(record.id.clone()))
            .map(|record| format!("duplicate real screen record id: {}", record.id))
    };
    if let Some(reason) = blocked_reason {
        return write_json(&output, &base_report("blocked", Some(reason)));
    }

    let graph = Arc::new(RateGraph::load(&context.manifest.canonical_pack)?);
    let retina = RetinaMap::from_graph(&graph, retina_config(&context.base.criteria))?;
    let parameters = RateParameters::initial(&graph);
    let (rest_activity, _) = final_activity_with_parameters_profile(
        &graph,
        &parameters,
        &vec![0.0; graph.neuron_count()],
        context.criteria.rate_steps_per_observation,
        None,
    )?;
    let groups = build_response_groups(&graph, &rest_activity, &context.base.criteria.frozen)?;
    let response_dimension = groups.len() * (4 + context.criteria.response_projection_dimension);
    let mut records = real_manifest.records.clone();
    records.sort_by(|left, right| {
        left.stream_id
            .cmp(&right.stream_id)
            .then_with(|| left.captured_at_s.total_cmp(&right.captured_at_s))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut response_history = BTreeMap::<String, Vec<FrozenReadoutHistoryEntry>>::new();
    let mut retina_history = BTreeMap::<String, Vec<FrozenReadoutHistoryEntry>>::new();
    let mut observations = Vec::with_capacity(records.len());
    let mut real_response_cache = BTreeMap::<String, CachedResponse>::new();
    let mut real_case_candidates = Vec::<(String, Vec<f64>)>::new();
    let mut real_timing = BTreeMap::<String, Vec<f64>>::new();
    let mut real_cache_hit_count = 0_usize;
    let mut real_cache_miss_count = 0_usize;
    for record in records {
        let total_started = Instant::now();
        let image_started = Instant::now();
        let image_path = resolve_path(Path::new("."), &record.path);
        let image = read_screen_image(&image_path)?;
        if habitua_connectome::image_sha256(&image) != record.sha256 {
            return Err(format!("real screen {} hash mismatch", record.id).into());
        }
        real_timing
            .entry("image_read".to_owned())
            .or_default()
            .push(image_started.elapsed().as_secs_f64() * 1_000.0);
        let retina_started = Instant::now();
        let image = image.resize_bilinear(retina.config.width, retina.config.height)?;
        let neural_input = retina
            .encode(&image, graph.neuron_count())?
            .input
            .into_iter()
            .map(|value| value as f32)
            .collect::<Vec<_>>();
        real_timing
            .entry("retina_mapping".to_owned())
            .or_default()
            .push(retina_started.elapsed().as_secs_f64() * 1_000.0);
        let input = neural_input
            .iter()
            .map(|value| f64::from(*value))
            .collect::<Vec<_>>();
        let cns_started = Instant::now();
        let response = if let Some(cached) = real_response_cache.get(&record.sha256).cloned() {
            real_cache_hit_count += 1;
            cached
        } else {
            real_cache_miss_count += 1;
            let (activity, _) = final_activity_with_parameters_profile(
                &graph,
                &parameters,
                &input,
                context.criteria.rate_steps_per_observation,
                None,
            )?;
            let response = cached_response_summary(
                &activity,
                &rest_activity,
                &groups,
                parameters.hmax,
                context.base.criteria.frozen.zero_response_epsilon,
                context.criteria.response_projection_dimension,
            );
            real_response_cache.insert(record.sha256.clone(), response.clone());
            response
        };
        real_timing
            .entry("cns_forward".to_owned())
            .or_default()
            .push(cns_started.elapsed().as_secs_f64() * 1_000.0);
        let readout_started = Instant::now();
        let history_scope = real_screen_history_scope(&record);
        let stream_response_history = response_history.entry(history_scope.clone()).or_default();
        let (history_values, current_features) = frozen_readout_augmented_features_with_expiry(
            &response.features,
            stream_response_history,
            record.captured_at_s,
            context.criteria.recent_observations,
            context.criteria.distance_threshold as f32,
            context.criteria.previous_absolute_difference_scale as f32,
            context.criteria.history_elapsed_time_scale_seconds as f32,
            context.criteria.behavior.history_max_age_seconds as f32,
        );
        let retina_projection =
            retina_input_projection(&neural_input, &retina, response.features.len());
        let stream_retina_history = retina_history.entry(history_scope).or_default();
        let (_, retina_augmented_features) = frozen_readout_augmented_features_with_expiry(
            &retina_projection,
            stream_retina_history,
            record.captured_at_s,
            context.criteria.recent_observations,
            context.criteria.distance_threshold as f32,
            context.criteria.previous_absolute_difference_scale as f32,
            context.criteria.history_elapsed_time_scale_seconds as f32,
            context.criteria.behavior.history_max_age_seconds as f32,
        );
        let label = match record.independent_label.as_str() {
            "no_change" => Some(false),
            "small_change" | "change" => Some(true),
            "hold" | "unlabeled" => None,
            _ => return Err(format!("unknown independent label for {}", record.id).into()),
        };
        let current_features = current_features
            .iter()
            .map(|value| f64::from(*value))
            .collect::<Vec<_>>();
        let history_features = current_features
            .get(response_dimension..)
            .unwrap_or_default()
            .to_vec();
        let response_features = response
            .features
            .iter()
            .map(|value| f64::from(*value))
            .collect::<Vec<_>>();
        real_timing
            .entry("readout".to_owned())
            .or_default()
            .push(readout_started.elapsed().as_secs_f64() * 1_000.0);
        let case_lookup_started = Instant::now();
        let _case_memory_match_count = runtime_case_memory_lookup_count(
            &record.sha256,
            &current_features,
            &real_case_candidates,
            context.criteria.case_memory.distance_threshold,
        );
        real_timing
            .entry("case_lookup".to_owned())
            .or_default()
            .push(case_lookup_started.elapsed().as_secs_f64() * 1_000.0);
        observations.push(RealScreenObservation {
            record: record.clone(),
            independent_label: label,
            current_features,
            response_features,
            shuffled_response_features: Vec::new(),
            history_features,
            retina_features: retina_augmented_features
                .into_iter()
                .map(f64::from)
                .collect(),
            distance: history_values.distance.map(f64::from),
        });
        stream_response_history.push(FrozenReadoutHistoryEntry {
            features: response.features,
            timestamp_s: record.captured_at_s,
        });
        stream_retina_history.push(FrozenReadoutHistoryEntry {
            features: retina_projection,
            timestamp_s: record.captured_at_s,
        });
        while stream_response_history.len() > context.criteria.recent_observations {
            stream_response_history.remove(0);
        }
        while stream_retina_history.len() > context.criteria.recent_observations {
            stream_retina_history.remove(0);
        }
        let candidate_features = observations
            .last()
            .map(|observation| observation.current_features.clone())
            .unwrap_or_default();
        real_case_candidates.push((record.sha256.clone(), candidate_features));
        real_timing
            .entry("total".to_owned())
            .or_default()
            .push(total_started.elapsed().as_secs_f64() * 1_000.0);
    }
    let shuffled_indices = deterministic_permutation(observations.len(), 0x9e37_79b9_7f4a_7c15);
    let shuffled_sources = observations
        .iter()
        .map(|observation| observation.response_features.clone())
        .collect::<Vec<_>>();
    for (index, observation) in observations.iter_mut().enumerate() {
        observation.shuffled_response_features = shuffled_sources[shuffled_indices[index]].clone();
    }
    let mut report = base_report(
        if trial_evaluation_possible {
            "trial"
        } else {
            "trial_pending"
        },
        Some(
            "性能・データ充足・計算完了を分離して判定するため、最終状態は条件計算後に確定する"
                .to_owned(),
        ),
    );
    report.case_memory_calibration = Some(real_screen_case_memory_calibration(
        &observations
            .iter()
            .filter(|observation| {
                observation.record.split == "training" || observation.record.split == "tuning"
            })
            .cloned()
            .collect::<Vec<_>>(),
        context.criteria.case_memory.distance_threshold,
        &observations,
    ));
    report.conditions = vec![
        real_screen_condition_report(
            "current_response_plus_history",
            "CNS response 80 + shared history 4",
            &observations,
            |observation| &observation.current_features,
            &context.criteria,
        )?,
        real_screen_condition_report(
            "history_only",
            "shared history 4 computed from CNS response",
            &observations,
            |observation| &observation.history_features,
            &context.criteria,
        )?,
        real_screen_condition_report(
            "cns_response_only",
            "CNS response 80 without history features",
            &observations,
            |observation| &observation.response_features,
            &context.criteria,
        )?,
        real_screen_condition_report(
            "retina_input_plus_history",
            "RetinaMap input projection 80 + history 4; CNS forward omitted",
            &observations,
            |observation| &observation.retina_features,
            &context.criteria,
        )?,
        real_screen_condition_report(
            "cns_response_shuffled",
            "deterministically permuted CNS response 80; shared split and labels",
            &observations,
            |observation| &observation.shuffled_response_features,
            &context.criteria,
        )?,
        real_screen_direct_condition_report(&observations, &context.criteria),
    ];
    let primary_condition = report
        .conditions
        .iter()
        .find(|condition| condition.name == adoption.performance_condition);
    let cns_condition = report
        .conditions
        .iter()
        .find(|condition| condition.name == "current_response_plus_history")
        .ok_or("current response condition is missing")?;
    let retina_condition = report
        .conditions
        .iter()
        .find(|condition| condition.name == "retina_input_plus_history")
        .ok_or("retina input condition is missing")?;
    let paired_comparison = real_screen_paired_comparison(
        cns_condition,
        retina_condition,
        &context.criteria.real_screen.trial,
        adoption,
    )?;
    report.cns_vs_retina_paired_comparison = Some(paired_comparison);
    let (performance_status, performance_criteria_met, adoption_criteria_met) =
        real_screen_adoption_decision(
            data_sufficiency_criteria_met,
            primary_condition.and_then(|condition| condition.balanced_accuracy),
            primary_condition.and_then(|condition| condition.auroc),
            report.cns_vs_retina_paired_comparison.as_ref(),
            adoption,
        );
    report.calculation_status = "complete".to_owned();
    report.data_sufficiency_status = data_sufficiency_status.to_owned();
    report.data_sufficiency_criteria_met = data_sufficiency_criteria_met;
    report.performance_status = performance_status.to_owned();
    report.performance_criteria_met = performance_criteria_met;
    report.adoption_criteria_met = adoption_criteria_met;
    report.evaluation_stage = if report.adoption_criteria_met {
        "adoption"
    } else if trial_evaluation_possible {
        "trial"
    } else {
        "pending"
    }
    .to_owned();
    report.status = if report.adoption_criteria_met {
        "complete"
    } else if trial_evaluation_possible {
        "trial"
    } else {
        "trial_pending"
    }
    .to_owned();
    report.reason = if report.adoption_criteria_met {
        None
    } else if !data_sufficiency_criteria_met {
        Some(format!(
            "データ充足条件未達: {data_sufficiency_status}。性能判定とは分離して記録した"
        ))
    } else if !performance_criteria_met {
        Some(format!(
            "性能条件未達または未算出: {performance_status}。データ充足とは分離して記録した"
        ))
    } else {
        Some("計算は完了したが、本採用条件は未成立".to_owned())
    };
    report.performance = Some(real_screen_performance_report(
        &real_timing,
        real_cache_hit_count,
        real_cache_miss_count,
    ));
    report.peak_rss_bytes = process_peak_rss_bytes();
    write_json(&output, &report)
}

fn retina_input_projection(input: &[f32], retina: &RetinaMap, dimension: usize) -> Vec<f32> {
    let mut projection = vec![0.0; dimension.max(1)];
    let mut counts = vec![0_usize; projection.len()];
    for (position, entry) in retina.entries.iter().enumerate() {
        let bucket = position * projection.len() / retina.entries.len().max(1);
        projection[bucket] += input[entry.neuron_index as usize];
        counts[bucket] += 1;
    }
    for (value, count) in projection.iter_mut().zip(counts) {
        *value /= count.max(1) as f32;
    }
    projection
}

fn deterministic_permutation(length: usize, seed: u64) -> Vec<usize> {
    let mut indices = (0..length).collect::<Vec<_>>();
    let mut state = seed.max(1);
    for index in (1..length).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let swap = (state as usize) % (index + 1);
        indices.swap(index, swap);
    }
    indices
}

fn runtime_case_memory_lookup_count(
    image_sha256: &str,
    _features: &[f64],
    candidates: &[(String, Vec<f64>)],
    _threshold: f64,
) -> usize {
    candidates
        .iter()
        .filter(|(candidate_hash, _candidate_features)| candidate_hash == image_sha256)
        .count()
}

fn l2_distance_f64(left: &[f64], right: &[f64]) -> f64 {
    if left.len() != right.len() {
        return f64::INFINITY;
    }
    left.iter()
        .zip(right)
        .map(|(left, right)| (left - right).powi(2))
        .sum::<f64>()
        .sqrt()
}

fn real_screen_performance_report(
    timings: &BTreeMap<String, Vec<f64>>,
    cache_hit_count: usize,
    cache_miss_count: usize,
) -> RealScreenPerformanceReport {
    let sample_count = timings.get("total").map_or(0, Vec::len);
    let stages = [
        "image_read",
        "retina_mapping",
        "cns_forward",
        "readout",
        "case_lookup",
    ]
    .into_iter()
    .map(|name| {
        let values = timings.get(name).map_or(&[][..], Vec::as_slice);
        RealScreenStageTimingReport {
            name: name.to_owned(),
            sample_count: values.len(),
            p50_ms: percentile(values, 0.50),
            p95_ms: percentile(values, 0.95),
        }
    })
    .collect();
    let total = timings.get("total").map_or(&[][..], Vec::as_slice);
    RealScreenPerformanceReport {
        sample_count,
        cache_mode: "exact_image_sha256_response_cache".to_owned(),
        cache_hit_count,
        cache_miss_count,
        p50_ms: percentile(total, 0.50),
        p95_ms: percentile(total, 0.95),
        stages,
    }
}

fn real_screen_case_memory_calibration(
    observations: &[RealScreenObservation],
    configured_threshold: f64,
    all_observations: &[RealScreenObservation],
) -> RealScreenCaseMemoryCalibration {
    let dimension = observations
        .first()
        .map_or(0, |observation| observation.current_features.len());
    let mut mean = vec![0.0; dimension];
    for observation in observations {
        for (slot, value) in mean.iter_mut().zip(&observation.current_features) {
            *slot += *value;
        }
    }
    for slot in &mut mean {
        *slot /= observations.len().max(1) as f64;
    }
    let mut scale = vec![1.0; dimension];
    for (index, slot) in scale.iter_mut().enumerate() {
        let variance = observations
            .iter()
            .map(|observation| (observation.current_features[index] - mean[index]).powi(2))
            .sum::<f64>()
            / observations.len().max(1) as f64;
        let candidate = variance.sqrt();
        if candidate.is_finite() && candidate > 1.0e-12 {
            *slot = candidate;
        }
    }
    let standardized = |observation: &RealScreenObservation| {
        observation
            .current_features
            .iter()
            .zip(&mean)
            .zip(&scale)
            .map(|((value, mean), scale)| (*value - *mean) / *scale)
            .collect::<Vec<_>>()
    };
    let standardized_features = observations.iter().map(standardized).collect::<Vec<_>>();
    let mut same_distances = Vec::new();
    let mut different_distances = Vec::new();
    for (left_index, left) in observations.iter().enumerate() {
        for (right_index, right) in observations.iter().enumerate().skip(left_index + 1) {
            let distance = standardized_features[left_index]
                .iter()
                .zip(&standardized_features[right_index])
                .map(|(left, right)| (left - right).powi(2))
                .sum::<f64>()
                .sqrt();
            let same_screen = left.record.sha256 == right.record.sha256;
            if same_screen {
                same_distances.push(distance);
            } else {
                different_distances.push(distance);
            }
        }
    }
    let same_p95 = percentile_option(&same_distances, 0.95);
    let different_min = percentile_option(&different_distances, 0.0);
    let different_p05 = percentile_option(&different_distances, 0.05);
    let selected_threshold = configured_threshold
        .is_finite()
        .then_some(configured_threshold)
        .filter(|threshold| *threshold > 0.0);
    let same_within = selected_threshold.map(|threshold| {
        same_distances
            .iter()
            .filter(|distance| **distance <= threshold)
            .count() as f64
            / same_distances.len().max(1) as f64
    });
    let different_within = selected_threshold.map(|threshold| {
        different_distances
            .iter()
            .filter(|distance| **distance <= threshold)
            .count() as f64
            / different_distances.len().max(1) as f64
    });
    let held_out = all_observations
        .iter()
        .filter(|observation| observation.record.split == "held_out")
        .collect::<Vec<_>>();
    let held_out_same_hash_pair_count = held_out
        .iter()
        .enumerate()
        .flat_map(|(left_index, left)| {
            held_out
                .iter()
                .skip(left_index + 1)
                .map(move |right| (left, right))
        })
        .filter(|(left, right)| left.record.sha256 == right.record.sha256)
        .count();
    let held_out_different_hash_near_count = selected_threshold.map_or(0, |threshold| {
        let held_out_standardized = held_out
            .iter()
            .map(|observation| standardized(observation))
            .collect::<Vec<_>>();
        held_out_standardized
            .iter()
            .enumerate()
            .flat_map(|(left_index, left)| {
                held_out_standardized
                    .iter()
                    .skip(left_index + 1)
                    .map(move |right| (left, right))
            })
            .filter(|(left, right)| l2_distance_f64(left, right) <= threshold)
            .count()
    });
    let runtime_boundary_verified = selected_threshold.is_some_and(|_threshold| {
        let candidates = all_observations
            .iter()
            .map(|observation| (observation.record.sha256.clone(), standardized(observation)))
            .collect::<Vec<_>>();
        let mut exact_hash_counts = BTreeMap::<&str, usize>::new();
        for (hash, _) in &candidates {
            *exact_hash_counts.entry(hash.as_str()).or_default() += 1;
        }
        all_observations.iter().all(|observation| {
            runtime_case_memory_lookup_count(
                &observation.record.sha256,
                &standardized(observation),
                &candidates,
                0.0,
            ) == exact_hash_counts
                .get(observation.record.sha256.as_str())
                .copied()
                .unwrap_or(0)
        })
    });
    RealScreenCaseMemoryCalibration {
        distance_metric: "l2_on_standardized_current_response_plus_history_features".to_owned(),
        screen_unit: "exact image SHA-256; distance distributions are diagnostic only".to_owned(),
        standardization: "training and tuning real-screen observations only".to_owned(),
        same_screen_pair_count: same_distances.len(),
        different_screen_pair_count: different_distances.len(),
        same_screen_distance_p50: percentile_option(&same_distances, 0.50),
        same_screen_distance_p95: same_p95,
        different_screen_distance_min: different_min,
        different_screen_distance_p05: different_p05,
        different_screen_distance_p50: percentile_option(&different_distances, 0.50),
        selected_threshold,
        same_screen_within_threshold_fraction: same_within,
        different_screen_within_threshold_fraction: different_within,
        neighbors_are_same_screen_only: different_within.is_some_and(|fraction| fraction == 0.0),
        exact_image_hash_primary_key: true,
        runtime_distance_threshold: configured_threshold,
        selected_threshold_role:
            "runtime_uses_the_fixed_criteria_threshold_only_as_a_secondary_diagnostic; exact_image_sha256_is_the_only_case_memory_match_key"
                .to_owned(),
        runtime_boundary_verified,
        calibration_splits: vec!["training".to_owned(), "tuning".to_owned()],
        held_out_same_hash_pair_count,
        held_out_different_hash_near_count,
    }
}

fn real_screen_condition_report<F>(
    name: &str,
    feature_definition: &str,
    observations: &[RealScreenObservation],
    feature: F,
    criteria: &LearningEvaluationCriteria,
) -> Result<RealScreenConditionReport, Box<dyn Error>>
where
    F: Fn(&RealScreenObservation) -> &[f64],
{
    let samples = observations
        .iter()
        .map(|observation| IndependentFeatureSample {
            split: observation.record.split.clone(),
            screen_unit: observation.record.stream_id.clone(),
            record_id: observation.record.id.clone(),
            image_sha256: observation.record.sha256.clone(),
            label: observation.independent_label,
            features: feature(observation).to_vec(),
        })
        .collect::<Vec<_>>();
    let training = samples
        .iter()
        .filter(|sample| sample.split == "training" && sample.label.is_some())
        .collect::<Vec<_>>();
    let tuning = samples
        .iter()
        .filter(|sample| sample.split == "tuning" && sample.label.is_some())
        .collect::<Vec<_>>();
    let evaluation = samples
        .iter()
        .filter(|sample| sample.split == "evaluation" && sample.label.is_some())
        .collect::<Vec<_>>();
    let evaluation_all = samples
        .iter()
        .filter(|sample| sample.split == "evaluation")
        .collect::<Vec<_>>();
    let held_out = samples
        .iter()
        .filter(|sample| sample.split == "held_out" && sample.label.is_some())
        .collect::<Vec<_>>();
    let held_out_all = samples
        .iter()
        .filter(|sample| sample.split == "held_out")
        .collect::<Vec<_>>();
    let selection = if tuning.is_empty()
        && criteria
            .real_screen
            .trial
            .expected_record_counts_by_split
            .get("tuning")
            .copied()
            .unwrap_or(0)
            == 0
    {
        &training
    } else {
        &tuning
    };
    let held_out_required = criteria
        .real_screen
        .trial
        .expected_record_counts_by_split
        .get("held_out")
        .copied()
        .unwrap_or(0)
        > 0;
    if training.is_empty()
        || selection.is_empty()
        || evaluation.is_empty()
        || (held_out_required && held_out.is_empty())
    {
        return Ok(unscored_real_screen_condition_report(
            name,
            feature_definition,
            &samples,
            observations,
            format!("real screen condition {name} lacks independent labels in a required split"),
            criteria,
        ));
    }
    let standardization = fit_value_standardization(
        &training
            .iter()
            .map(|sample| sample.features.clone())
            .collect::<Vec<_>>(),
    )?;
    let training_features = standardize_value_samples(&training, &standardization);
    let selection_features = standardize_value_samples(selection, &standardization);
    let training_labels = training
        .iter()
        .map(|sample| sample.label.unwrap())
        .collect::<Vec<_>>();
    let selection_labels = selection
        .iter()
        .map(|sample| sample.label.unwrap())
        .collect::<Vec<_>>();
    let mut selected: Option<RealScreenSelected> = None;
    for learning_rate in &criteria.batch_learning_rates {
        for l2 in &criteria.batch_l2_values {
            let (weights, bias) = fit_batch_parameters(
                &training_features,
                &training_labels,
                *learning_rate,
                *l2,
                criteria.batch_epochs,
            );
            let scores = selection_features
                .iter()
                .map(|features| predict_batch_parameters(&weights, bias, features))
                .collect::<Vec<_>>();
            let score = (
                balanced_accuracy_at_threshold(
                    &scores,
                    &selection_labels,
                    criteria.readout_reaction_threshold,
                ),
                auroc(&scores, &selection_labels),
                -batch_log_loss(&scores, &selection_labels),
            );
            if selected
                .as_ref()
                .is_none_or(|(_, _, _, _, selected_score)| score > *selected_score)
            {
                selected = Some((weights, bias, *learning_rate, *l2, score));
            }
        }
    }
    let (weights, bias, _learning_rate, _l2, _) = selected.ok_or("real screen grid is empty")?;
    let selection_scores = selection_features
        .iter()
        .map(|features| predict_batch_parameters(&weights, bias, features))
        .collect::<Vec<_>>();
    let threshold = select_binary_decision_threshold(
        &selection_scores,
        &selection_labels,
        criteria.readout_reaction_threshold,
    );
    let evaluation_features = standardize_value_samples(&evaluation, &standardization);
    let evaluation_labels = evaluation
        .iter()
        .map(|sample| sample.label.unwrap())
        .collect::<Vec<_>>();
    let evaluation_scores = evaluation_features
        .iter()
        .map(|features| predict_batch_parameters(&weights, bias, features))
        .collect::<Vec<_>>();
    let evaluation_all_features = standardize_value_samples(&evaluation_all, &standardization);
    let evaluation_all_scores = evaluation_all_features
        .iter()
        .map(|features| predict_batch_parameters(&weights, bias, features))
        .collect::<Vec<_>>();
    let held_out_features = standardize_value_samples(&held_out, &standardization);
    let held_out_labels = held_out
        .iter()
        .map(|sample| sample.label.unwrap())
        .collect::<Vec<_>>();
    let held_out_scores = held_out_features
        .iter()
        .map(|features| predict_batch_parameters(&weights, bias, features))
        .collect::<Vec<_>>();
    let held_out_all_features = standardize_value_samples(&held_out_all, &standardization);
    let held_out_all_scores = held_out_all_features
        .iter()
        .map(|features| predict_batch_parameters(&weights, bias, features))
        .collect::<Vec<_>>();
    let (true_positive, false_positive, true_negative, false_negative) =
        confusion_matrix(&evaluation_scores, &evaluation_labels, threshold);
    let evaluation_hold_count = evaluation_all.len().saturating_sub(evaluation.len());
    let held_out_hold_count = held_out_all.len().saturating_sub(held_out.len());
    let evaluation_ci = real_screen_metric_confidence_intervals(
        &evaluation,
        &evaluation_scores,
        &evaluation_labels,
        threshold,
        &criteria.real_screen.trial,
    );
    let evaluation_confidence_interval_method = real_screen_confidence_interval_method(&evaluation);
    let held_out_has_both_classes = has_both_boolean_classes(&held_out_labels);
    let held_out_ci = held_out_has_both_classes
        .then(|| {
            real_screen_metric_confidence_intervals(
                &held_out,
                &held_out_scores,
                &held_out_labels,
                threshold,
                &criteria.real_screen.trial,
            )
        })
        .flatten();
    let held_out_confidence_interval_method = real_screen_confidence_interval_method(&held_out);
    let evaluation_positive_screen_unit_count =
        independent_sample_class_screen_unit_count(&evaluation, true);
    let evaluation_negative_screen_unit_count =
        independent_sample_class_screen_unit_count(&evaluation, false);
    let evaluation_session_count = evaluation
        .iter()
        .map(|sample| sample.screen_unit.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let evaluation_hold_fraction =
        evaluation_hold_count as f64 / evaluation_all.len().max(1) as f64;
    let (statistical_power_status, _) = real_screen_data_sufficiency_status(
        evaluation.len(),
        evaluation_labels.iter().filter(|label| **label).count(),
        evaluation_labels.iter().filter(|label| !**label).count(),
        evaluation_positive_screen_unit_count,
        evaluation_negative_screen_unit_count,
        evaluation_session_count,
        evaluation_hold_fraction,
        true,
        true,
        &criteria.real_screen.adoption,
    );
    let mut label_counts = BTreeMap::new();
    for observation in observations {
        *label_counts
            .entry(observation.record.independent_label.clone())
            .or_insert(0) += 1;
    }
    Ok(RealScreenConditionReport {
        name: name.to_owned(),
        feature_definition: feature_definition.to_owned(),
        feature_dimension: samples.first().map_or(0, |sample| sample.features.len()),
        training_sample_count: training.len(),
        tuning_sample_count: tuning.len(),
        evaluation_sample_count: evaluation.len(),
        held_out_sample_count: held_out.len(),
        evaluation_total_count: evaluation_all.len(),
        held_out_total_count: held_out_all.len(),
        evaluation_scored_count: evaluation.len(),
        held_out_scored_count: held_out.len(),
        evaluation_hold_count,
        held_out_hold_count,
        evaluation_screen_unit_count: independent_screen_unit_count(&evaluation_all),
        held_out_screen_unit_count: independent_screen_unit_count(&held_out_all),
        evaluation_positive_screen_unit_count,
        evaluation_negative_screen_unit_count,
        held_out_positive_screen_unit_count: independent_sample_class_screen_unit_count(
            &held_out, true,
        ),
        held_out_negative_screen_unit_count: independent_sample_class_screen_unit_count(
            &held_out, false,
        ),
        evaluated_count: evaluation.len(),
        held_out_evaluated_count: held_out.len(),
        hold_count: observations
            .iter()
            .filter(|observation| observation.independent_label.is_none())
            .count(),
        decision_threshold: Some(threshold),
        selection_split: if tuning.is_empty() {
            "training".to_owned()
        } else {
            "tuning".to_owned()
        },
        balanced_accuracy: Some(balanced_accuracy_at_threshold(
            &evaluation_scores,
            &evaluation_labels,
            threshold,
        )),
        balanced_accuracy_ci95: evaluation_ci.as_ref().map(|interval| interval.0),
        evaluation_bootstrap_effective_replicates: evaluation_ci
            .as_ref()
            .map_or(0, |interval| interval.2),
        evaluation_confidence_interval_method: evaluation_confidence_interval_method.to_owned(),
        auroc: Some(auroc(&evaluation_scores, &evaluation_labels)),
        auroc_ci95: evaluation_ci.as_ref().map(|interval| interval.1),
        hold_as_error_accuracy: Some(accuracy_with_hold_as_error(
            &evaluation_all_scores,
            &evaluation_all,
            threshold,
        )),
        held_out_balanced_accuracy: held_out_has_both_classes
            .then(|| balanced_accuracy_at_threshold(&held_out_scores, &held_out_labels, threshold)),
        held_out_balanced_accuracy_ci95: held_out_ci.as_ref().map(|interval| interval.0),
        held_out_bootstrap_effective_replicates: held_out_ci
            .as_ref()
            .map_or(0, |interval| interval.2),
        held_out_confidence_interval_method: held_out_confidence_interval_method.to_owned(),
        held_out_auroc: held_out_has_both_classes
            .then(|| auroc(&held_out_scores, &held_out_labels)),
        held_out_auroc_ci95: held_out_ci.as_ref().map(|interval| interval.1),
        held_out_hold_as_error_accuracy: (!held_out_all.is_empty())
            .then(|| accuracy_with_hold_as_error(&held_out_all_scores, &held_out_all, threshold)),
        true_positive,
        false_positive,
        true_negative,
        false_negative,
        predicted_positive_count: evaluation_scores
            .iter()
            .filter(|score| **score >= threshold)
            .count(),
        predicted_positive_count_including_hold: evaluation_all_scores
            .iter()
            .filter(|score| **score >= threshold)
            .count(),
        label_counts,
        required_independent_positive_count: criteria
            .real_screen
            .adoption
            .minimum_independent_positive_count_for_claim,
        required_independent_negative_count: criteria
            .real_screen
            .adoption
            .minimum_independent_negative_count_for_claim,
        required_independent_positive_screen_unit_count: criteria
            .real_screen
            .adoption
            .minimum_independent_positive_screen_unit_count_for_claim,
        required_independent_negative_screen_unit_count: criteria
            .real_screen
            .adoption
            .minimum_independent_negative_screen_unit_count_for_claim,
        statistical_power_status: statistical_power_status.to_owned(),
        error: None,
        pairing_samples: evaluation.iter().map(|sample| (*sample).clone()).collect(),
        pairing_scores: evaluation_scores,
        pairing_labels: evaluation_labels,
    })
}

fn unscored_real_screen_condition_report(
    name: &str,
    feature_definition: &str,
    samples: &[IndependentFeatureSample],
    observations: &[RealScreenObservation],
    error: String,
    criteria: &LearningEvaluationCriteria,
) -> RealScreenConditionReport {
    let count_for = |split: &str| {
        samples
            .iter()
            .filter(|sample| sample.split == split)
            .count()
    };
    let scored_for = |split: &str| {
        samples
            .iter()
            .filter(|sample| sample.split == split && sample.label.is_some())
            .count()
    };
    let label_counts = observations
        .iter()
        .fold(BTreeMap::new(), |mut counts, observation| {
            *counts
                .entry(observation.record.independent_label.clone())
                .or_insert(0) += 1;
            counts
        });
    let evaluation_count = count_for("evaluation");
    let held_out_count = count_for("held_out");
    let evaluation_scored_count = scored_for("evaluation");
    let held_out_scored_count = scored_for("held_out");
    RealScreenConditionReport {
        name: name.to_owned(),
        feature_definition: feature_definition.to_owned(),
        feature_dimension: samples.first().map_or(0, |sample| sample.features.len()),
        training_sample_count: scored_for("training"),
        tuning_sample_count: scored_for("tuning"),
        evaluation_sample_count: evaluation_scored_count,
        held_out_sample_count: held_out_scored_count,
        evaluation_total_count: evaluation_count,
        held_out_total_count: held_out_count,
        evaluation_scored_count,
        held_out_scored_count,
        evaluation_hold_count: evaluation_count.saturating_sub(evaluation_scored_count),
        held_out_hold_count: held_out_count.saturating_sub(held_out_scored_count),
        evaluation_screen_unit_count: samples
            .iter()
            .filter(|sample| sample.split == "evaluation")
            .map(|sample| sample.screen_unit.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        held_out_screen_unit_count: samples
            .iter()
            .filter(|sample| sample.split == "held_out")
            .map(|sample| sample.screen_unit.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        evaluation_positive_screen_unit_count: independent_sample_class_screen_unit_count(
            &samples
                .iter()
                .filter(|sample| sample.split == "evaluation")
                .collect::<Vec<_>>(),
            true,
        ),
        evaluation_negative_screen_unit_count: independent_sample_class_screen_unit_count(
            &samples
                .iter()
                .filter(|sample| sample.split == "evaluation")
                .collect::<Vec<_>>(),
            false,
        ),
        held_out_positive_screen_unit_count: independent_sample_class_screen_unit_count(
            &samples
                .iter()
                .filter(|sample| sample.split == "held_out")
                .collect::<Vec<_>>(),
            true,
        ),
        held_out_negative_screen_unit_count: independent_sample_class_screen_unit_count(
            &samples
                .iter()
                .filter(|sample| sample.split == "held_out")
                .collect::<Vec<_>>(),
            false,
        ),
        evaluated_count: evaluation_scored_count,
        held_out_evaluated_count: held_out_scored_count,
        hold_count: samples
            .iter()
            .filter(|sample| sample.label.is_none())
            .count(),
        decision_threshold: None,
        selection_split: "none".to_owned(),
        balanced_accuracy: None,
        balanced_accuracy_ci95: None,
        evaluation_bootstrap_effective_replicates: 0,
        evaluation_confidence_interval_method: "unavailable".to_owned(),
        auroc: None,
        auroc_ci95: None,
        hold_as_error_accuracy: None,
        held_out_balanced_accuracy: None,
        held_out_balanced_accuracy_ci95: None,
        held_out_bootstrap_effective_replicates: 0,
        held_out_confidence_interval_method: "unavailable".to_owned(),
        held_out_auroc: None,
        held_out_auroc_ci95: None,
        held_out_hold_as_error_accuracy: None,
        true_positive: 0,
        false_positive: 0,
        true_negative: 0,
        false_negative: 0,
        predicted_positive_count: 0,
        predicted_positive_count_including_hold: 0,
        label_counts,
        required_independent_positive_count: criteria
            .real_screen
            .adoption
            .minimum_independent_positive_count_for_claim,
        required_independent_negative_count: criteria
            .real_screen
            .adoption
            .minimum_independent_negative_count_for_claim,
        required_independent_positive_screen_unit_count: criteria
            .real_screen
            .adoption
            .minimum_independent_positive_screen_unit_count_for_claim,
        required_independent_negative_screen_unit_count: criteria
            .real_screen
            .adoption
            .minimum_independent_negative_screen_unit_count_for_claim,
        statistical_power_status: "insufficient_independent_labels".to_owned(),
        error: Some(error),
        pairing_samples: Vec::new(),
        pairing_scores: Vec::new(),
        pairing_labels: Vec::new(),
    }
}

fn real_screen_direct_condition_report(
    observations: &[RealScreenObservation],
    criteria: &LearningEvaluationCriteria,
) -> RealScreenConditionReport {
    let evaluation = observations
        .iter()
        .filter(|observation| {
            observation.record.split == "evaluation" && observation.independent_label.is_some()
        })
        .collect::<Vec<_>>();
    let evaluation_all = observations
        .iter()
        .filter(|observation| observation.record.split == "evaluation")
        .collect::<Vec<_>>();
    let scores = evaluation
        .iter()
        .map(|observation| observation.distance.unwrap_or(0.0) / criteria.distance_threshold)
        .collect::<Vec<_>>();
    let labels = evaluation
        .iter()
        .map(|observation| observation.independent_label.unwrap())
        .collect::<Vec<_>>();
    let held_out = observations
        .iter()
        .filter(|observation| {
            observation.record.split == "held_out" && observation.independent_label.is_some()
        })
        .collect::<Vec<_>>();
    let held_out_all = observations
        .iter()
        .filter(|observation| observation.record.split == "held_out")
        .collect::<Vec<_>>();
    let held_out_scores = held_out
        .iter()
        .map(|observation| observation.distance.unwrap_or(0.0) / criteria.distance_threshold)
        .collect::<Vec<_>>();
    let held_out_labels = held_out
        .iter()
        .map(|observation| observation.independent_label.unwrap())
        .collect::<Vec<_>>();
    let held_out_required = criteria
        .real_screen
        .trial
        .expected_record_counts_by_split
        .get("held_out")
        .copied()
        .unwrap_or(0)
        > 0;
    if evaluation.is_empty() || (held_out_required && held_out.is_empty()) {
        let samples = observations
            .iter()
            .map(|observation| IndependentFeatureSample {
                split: observation.record.split.clone(),
                screen_unit: observation.record.stream_id.clone(),
                record_id: observation.record.id.clone(),
                image_sha256: observation.record.sha256.clone(),
                label: observation.independent_label,
                features: vec![observation.distance.unwrap_or(0.0)],
            })
            .collect::<Vec<_>>();
        return unscored_real_screen_condition_report(
            "direct_history_distance_rule",
            "distance > distance_threshold from CNS response history",
            &samples,
            observations,
            "direct rule requires independent labels in evaluation and configured held_out splits"
                .to_owned(),
            criteria,
        );
    }
    let evaluation_all_scores = evaluation_all
        .iter()
        .map(|observation| observation.distance.unwrap_or(0.0) / criteria.distance_threshold)
        .collect::<Vec<_>>();
    let held_out_all_scores = held_out_all
        .iter()
        .map(|observation| observation.distance.unwrap_or(0.0) / criteria.distance_threshold)
        .collect::<Vec<_>>();
    let (true_positive, false_positive, true_negative, false_negative) =
        confusion_matrix(&scores, &labels, 1.0);
    let evaluation_ci_samples = evaluation
        .iter()
        .map(|observation| IndependentFeatureSample {
            split: "evaluation".to_owned(),
            screen_unit: observation.record.stream_id.clone(),
            record_id: observation.record.id.clone(),
            image_sha256: observation.record.sha256.clone(),
            label: observation.independent_label,
            features: vec![0.0],
        })
        .collect::<Vec<_>>();
    let evaluation_ci_refs = evaluation_ci_samples.iter().collect::<Vec<_>>();
    let held_out_ci_samples = held_out
        .iter()
        .map(|observation| IndependentFeatureSample {
            split: "held_out".to_owned(),
            screen_unit: observation.record.stream_id.clone(),
            record_id: observation.record.id.clone(),
            image_sha256: observation.record.sha256.clone(),
            label: observation.independent_label,
            features: vec![0.0],
        })
        .collect::<Vec<_>>();
    let held_out_ci_refs = held_out_ci_samples.iter().collect::<Vec<_>>();
    let evaluation_ci = real_screen_metric_confidence_intervals(
        &evaluation_ci_refs,
        &scores,
        &labels,
        1.0,
        &criteria.real_screen.trial,
    );
    let evaluation_confidence_interval_method =
        real_screen_confidence_interval_method(&evaluation_ci_refs);
    let held_out_has_both_classes = has_both_boolean_classes(&held_out_labels);
    let held_out_ci = held_out_has_both_classes
        .then(|| {
            real_screen_metric_confidence_intervals(
                &held_out_ci_refs,
                &held_out_scores,
                &held_out_labels,
                1.0,
                &criteria.real_screen.trial,
            )
        })
        .flatten();
    let held_out_confidence_interval_method =
        real_screen_confidence_interval_method(&held_out_ci_refs);
    let evaluation_positive_screen_unit_count = evaluation
        .iter()
        .filter(|observation| observation.independent_label == Some(true))
        .map(|observation| observation.record.stream_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let evaluation_negative_screen_unit_count = evaluation
        .iter()
        .filter(|observation| observation.independent_label == Some(false))
        .map(|observation| observation.record.stream_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let evaluation_session_count = evaluation
        .iter()
        .map(|observation| observation.record.stream_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let evaluation_hold_fraction = evaluation_all.len().saturating_sub(evaluation.len()) as f64
        / evaluation_all.len().max(1) as f64;
    let (statistical_power_status, _) = real_screen_data_sufficiency_status(
        evaluation.len(),
        labels.iter().filter(|label| **label).count(),
        labels.iter().filter(|label| !**label).count(),
        evaluation_positive_screen_unit_count,
        evaluation_negative_screen_unit_count,
        evaluation_session_count,
        evaluation_hold_fraction,
        true,
        true,
        &criteria.real_screen.adoption,
    );
    let mut label_counts = BTreeMap::new();
    for observation in observations {
        *label_counts
            .entry(observation.record.independent_label.clone())
            .or_insert(0) += 1;
    }
    RealScreenConditionReport {
        name: "direct_history_distance_rule".to_owned(),
        feature_definition: "distance > distance_threshold from CNS response history".to_owned(),
        feature_dimension: 1,
        training_sample_count: observations
            .iter()
            .filter(|observation| {
                observation.record.split == "training" && observation.independent_label.is_some()
            })
            .count(),
        tuning_sample_count: observations
            .iter()
            .filter(|observation| {
                observation.record.split == "tuning" && observation.independent_label.is_some()
            })
            .count(),
        evaluation_sample_count: evaluation.len(),
        held_out_sample_count: held_out.len(),
        evaluation_total_count: evaluation_all.len(),
        held_out_total_count: held_out_all.len(),
        evaluation_scored_count: evaluation.len(),
        held_out_scored_count: held_out.len(),
        evaluation_hold_count: evaluation_all.len().saturating_sub(evaluation.len()),
        held_out_hold_count: held_out_all.len().saturating_sub(held_out.len()),
        evaluation_screen_unit_count: evaluation
            .iter()
            .map(|observation| observation.record.stream_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        held_out_screen_unit_count: held_out
            .iter()
            .map(|observation| observation.record.stream_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        evaluation_positive_screen_unit_count,
        evaluation_negative_screen_unit_count,
        held_out_positive_screen_unit_count: held_out
            .iter()
            .filter(|observation| observation.independent_label == Some(true))
            .map(|observation| observation.record.stream_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        held_out_negative_screen_unit_count: held_out
            .iter()
            .filter(|observation| observation.independent_label == Some(false))
            .map(|observation| observation.record.stream_id.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        evaluated_count: evaluation.len(),
        held_out_evaluated_count: held_out.len(),
        hold_count: observations
            .iter()
            .filter(|observation| observation.independent_label.is_none())
            .count(),
        decision_threshold: Some(1.0),
        selection_split: "criteria_distance_threshold".to_owned(),
        balanced_accuracy: Some(balanced_accuracy_at_threshold(&scores, &labels, 1.0)),
        balanced_accuracy_ci95: evaluation_ci.as_ref().map(|interval| interval.0),
        evaluation_bootstrap_effective_replicates: evaluation_ci
            .as_ref()
            .map_or(0, |interval| interval.2),
        evaluation_confidence_interval_method: evaluation_confidence_interval_method.to_owned(),
        auroc: Some(auroc(&scores, &labels)),
        auroc_ci95: evaluation_ci.as_ref().map(|interval| interval.1),
        hold_as_error_accuracy: Some(accuracy_with_hold_as_error_observations(
            &evaluation_all_scores,
            &evaluation_all,
            1.0,
        )),
        held_out_balanced_accuracy: held_out_has_both_classes
            .then(|| balanced_accuracy_at_threshold(&held_out_scores, &held_out_labels, 1.0)),
        held_out_balanced_accuracy_ci95: held_out_ci.as_ref().map(|interval| interval.0),
        held_out_bootstrap_effective_replicates: held_out_ci
            .as_ref()
            .map_or(0, |interval| interval.2),
        held_out_confidence_interval_method: held_out_confidence_interval_method.to_owned(),
        held_out_auroc: held_out_has_both_classes
            .then(|| auroc(&held_out_scores, &held_out_labels)),
        held_out_auroc_ci95: held_out_ci.as_ref().map(|interval| interval.1),
        held_out_hold_as_error_accuracy: (!held_out_all.is_empty()).then(|| {
            accuracy_with_hold_as_error_observations(&held_out_all_scores, &held_out_all, 1.0)
        }),
        true_positive,
        false_positive,
        true_negative,
        false_negative,
        predicted_positive_count: scores.iter().filter(|score| **score >= 1.0).count(),
        predicted_positive_count_including_hold: evaluation_all_scores
            .iter()
            .filter(|score| **score >= 1.0)
            .count(),
        label_counts,
        required_independent_positive_count: criteria
            .real_screen
            .adoption
            .minimum_independent_positive_count_for_claim,
        required_independent_negative_count: criteria
            .real_screen
            .adoption
            .minimum_independent_negative_count_for_claim,
        required_independent_positive_screen_unit_count: criteria
            .real_screen
            .adoption
            .minimum_independent_positive_screen_unit_count_for_claim,
        required_independent_negative_screen_unit_count: criteria
            .real_screen
            .adoption
            .minimum_independent_negative_screen_unit_count_for_claim,
        statistical_power_status: statistical_power_status.to_owned(),
        error: None,
        pairing_samples: evaluation_ci_samples,
        pairing_scores: scores,
        pairing_labels: labels,
    }
}

fn real_screen_paired_comparison(
    cns: &RealScreenConditionReport,
    retina: &RealScreenConditionReport,
    trial: &RealScreenTrialCriteria,
    adoption: &RealScreenAdoptionCriteria,
) -> Result<RealScreenPairedComparisonReport, Box<dyn Error>> {
    if cns.pairing_samples.len() != retina.pairing_samples.len()
        || cns.pairing_scores.len() != retina.pairing_scores.len()
        || cns.pairing_labels.len() != retina.pairing_labels.len()
        || cns.pairing_samples.len() != cns.pairing_scores.len()
    {
        return Err("paired CNS and retina evaluation samples have different lengths".into());
    }
    if !paired_feature_samples_match(&cns.pairing_samples, &retina.pairing_samples) {
        return Err(
            "paired CNS and retina samples do not have identical record IDs, image hashes, session labels, and labels"
                .into(),
        );
    }
    let samples = &cns.pairing_samples;
    let sample_count = samples.len();
    let method = real_screen_confidence_interval_method(&samples.iter().collect::<Vec<_>>());
    let cns_balanced_accuracy = cns.balanced_accuracy;
    let retina_balanced_accuracy = retina.balanced_accuracy;
    let paired_balanced_accuracy_difference = cns_balanced_accuracy
        .zip(retina_balanced_accuracy)
        .map(|(cns, retina)| cns - retina);
    let mut ci_values = Vec::new();
    if sample_count > 0 && method != "unavailable" {
        let mut clusters = BTreeMap::<&str, Vec<usize>>::new();
        for (index, sample) in samples.iter().enumerate() {
            clusters
                .entry(sample.screen_unit.as_str())
                .or_default()
                .push(index);
        }
        let clusters = clusters.into_values().collect::<Vec<_>>();
        let cluster_bootstrap = method == "screen_unit_cluster_bootstrap";
        let mut rng = StdRng::seed_from_u64(trial.bootstrap_seed);
        for _ in 0..trial.bootstrap_replicates {
            let indices =
                bootstrap_sample_indices(&clusters, sample_count, cluster_bootstrap, &mut rng);
            let labels = indices
                .iter()
                .map(|index| cns.pairing_labels[*index])
                .collect::<Vec<_>>();
            if !has_both_boolean_classes(&labels) {
                continue;
            }
            let cns_scores = indices
                .iter()
                .map(|index| cns.pairing_scores[*index])
                .collect::<Vec<_>>();
            let retina_scores = indices
                .iter()
                .map(|index| retina.pairing_scores[*index])
                .collect::<Vec<_>>();
            let cns_accuracy = balanced_accuracy_at_threshold(
                &cns_scores,
                &labels,
                cns.decision_threshold.unwrap_or(0.5),
            );
            let retina_accuracy = balanced_accuracy_at_threshold(
                &retina_scores,
                &labels,
                retina.decision_threshold.unwrap_or(0.5),
            );
            ci_values.push(cns_accuracy - retina_accuracy);
        }
    }
    let ci95 = (!ci_values.is_empty()).then(|| {
        [
            percentile_option(&ci_values, 0.025).expect("non-empty paired bootstrap values"),
            percentile_option(&ci_values, 0.975).expect("non-empty paired bootstrap values"),
        ]
    });
    let criteria_met = method == "screen_unit_cluster_bootstrap"
        && ci95.is_some_and(|interval| {
            interval[0] >= adoption.minimum_paired_balanced_accuracy_difference_lower_bound
        });
    let error = if sample_count == 0 {
        Some("paired evaluation has no independently labeled samples".to_owned())
    } else if method == "unavailable" {
        Some("paired evaluation lacks both label classes".to_owned())
    } else if ci95.is_none() {
        Some("paired bootstrap produced no replicates with both label classes".to_owned())
    } else {
        None
    };
    Ok(RealScreenPairedComparisonReport {
        cns_condition: cns.name.clone(),
        retina_condition: retina.name.clone(),
        evaluation_sample_count: sample_count,
        cns_balanced_accuracy,
        retina_balanced_accuracy,
        paired_balanced_accuracy_difference,
        ci95,
        bootstrap_effective_replicates: ci_values.len(),
        confidence_interval_method: method.to_owned(),
        minimum_lower_bound: adoption.minimum_paired_balanced_accuracy_difference_lower_bound,
        criteria_met,
        rationale: adoption.paired_difference_rationale.clone(),
        error,
    })
}

fn paired_feature_samples_match(
    left: &[IndependentFeatureSample],
    right: &[IndependentFeatureSample],
) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.split == right.split
                && left.screen_unit == right.screen_unit
                && left.record_id == right.record_id
                && left.image_sha256 == right.image_sha256
                && left.label == right.label
        })
}

fn real_screen_performance_criteria_met(
    balanced_accuracy: Option<f64>,
    auroc: Option<f64>,
    paired_lower_bound: Option<f64>,
    paired_method: Option<&str>,
    criteria: &RealScreenAdoptionCriteria,
) -> bool {
    balanced_accuracy.is_some_and(|value| value >= criteria.minimum_balanced_accuracy)
        && auroc.is_some_and(|value| value >= criteria.minimum_auroc)
        && paired_method == Some("screen_unit_cluster_bootstrap")
        && paired_lower_bound.is_some_and(|value| {
            value >= criteria.minimum_paired_balanced_accuracy_difference_lower_bound
        })
}

fn real_screen_performance_status(
    balanced_accuracy: Option<f64>,
    auroc: Option<f64>,
    paired: Option<&RealScreenPairedComparisonReport>,
    criteria_met: bool,
) -> &'static str {
    if balanced_accuracy.is_none() || auroc.is_none() {
        "not_available"
    } else if paired.is_none_or(|comparison| comparison.ci95.is_none()) {
        "not_available_paired_comparison"
    } else if criteria_met {
        "met"
    } else {
        "not_met"
    }
}

fn real_screen_adoption_decision(
    data_sufficient: bool,
    balanced_accuracy: Option<f64>,
    auroc: Option<f64>,
    paired: Option<&RealScreenPairedComparisonReport>,
    criteria: &RealScreenAdoptionCriteria,
) -> (String, bool, bool) {
    let performance_met = real_screen_performance_criteria_met(
        balanced_accuracy,
        auroc,
        paired.and_then(|comparison| comparison.ci95.map(|interval| interval[0])),
        paired.map(|comparison| comparison.confidence_interval_method.as_str()),
        criteria,
    );
    let performance_status =
        real_screen_performance_status(balanced_accuracy, auroc, paired, performance_met);
    let adoption_met = real_screen_adoption_criteria_met(data_sufficient, performance_met);
    (performance_status.to_owned(), performance_met, adoption_met)
}

fn real_screen_adoption_criteria_met(data_sufficient: bool, performance_met: bool) -> bool {
    data_sufficient && performance_met
}

fn fit_value_standardization(
    values: &[Vec<f64>],
) -> Result<FeatureStandardization, Box<dyn Error>> {
    let first = values.first().ok_or("real screen training set is empty")?;
    let dimension = first.len();
    let mut mean = vec![0.0; dimension];
    for value in values {
        if value.len() != dimension {
            return Err("real screen feature dimensions differ".into());
        }
        for (slot, current) in mean.iter_mut().zip(value) {
            *slot += *current;
        }
    }
    for slot in &mut mean {
        *slot /= values.len() as f64;
    }
    let mut variance = vec![0.0; dimension];
    for value in values {
        for ((slot, current), average) in variance.iter_mut().zip(value).zip(&mean) {
            let difference = *current - *average;
            *slot += difference * difference;
        }
    }
    let scale = variance
        .into_iter()
        .map(|value| {
            let scale = (value / values.len() as f64).sqrt();
            if scale.is_finite() && scale > 1.0e-12 {
                scale
            } else {
                1.0
            }
        })
        .collect();
    Ok(FeatureStandardization {
        mean,
        scale,
        zero_variance_count: 0,
    })
}

fn accuracy_with_hold_as_error(
    scores: &[f64],
    samples: &[&IndependentFeatureSample],
    threshold: f64,
) -> f64 {
    let correct = scores
        .iter()
        .zip(samples)
        .filter(|(score, sample)| {
            sample
                .label
                .is_some_and(|label| (**score >= threshold) == label)
        })
        .count();
    correct as f64 / scores.len().max(1) as f64
}

fn accuracy_with_hold_as_error_observations(
    scores: &[f64],
    observations: &[&RealScreenObservation],
    threshold: f64,
) -> f64 {
    let correct = scores
        .iter()
        .zip(observations)
        .filter(|(score, observation)| {
            observation
                .independent_label
                .is_some_and(|label| (**score >= threshold) == label)
        })
        .count();
    correct as f64 / scores.len().max(1) as f64
}

fn independent_screen_unit_count(samples: &[&IndependentFeatureSample]) -> usize {
    samples
        .iter()
        .map(|sample| sample.screen_unit.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

fn independent_sample_class_screen_unit_count(
    samples: &[&IndependentFeatureSample],
    positive: bool,
) -> usize {
    samples
        .iter()
        .filter(|sample| sample.label == Some(positive))
        .map(|sample| sample.screen_unit.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

fn has_both_boolean_classes(labels: &[bool]) -> bool {
    labels.iter().any(|label| *label) && labels.iter().any(|label| !*label)
}

#[cfg(test)]
fn required_independent_class_count(criteria: &RealScreenTrialCriteria) -> usize {
    let z = 1.96_f64;
    let half_width = criteria.ba_ci_half_width_target;
    ((z * z * 0.25) / (2.0 * half_width * half_width)).ceil() as usize
}

fn real_screen_confidence_interval_method(samples: &[&IndependentFeatureSample]) -> &'static str {
    let mut clusters = BTreeSet::new();
    let mut positive_clusters = BTreeSet::new();
    let mut negative_clusters = BTreeSet::new();
    for sample in samples {
        clusters.insert(sample.screen_unit.as_str());
        if sample.label == Some(true) {
            positive_clusters.insert(sample.screen_unit.as_str());
        } else if sample.label == Some(false) {
            negative_clusters.insert(sample.screen_unit.as_str());
        }
    }
    if clusters.len() >= 2 && positive_clusters.len() >= 2 && negative_clusters.len() >= 2 {
        "screen_unit_cluster_bootstrap"
    } else if samples.iter().any(|sample| sample.label == Some(true))
        && samples.iter().any(|sample| sample.label == Some(false))
    {
        "record_bootstrap_trial_only"
    } else {
        "unavailable"
    }
}

fn real_screen_metric_confidence_intervals(
    samples: &[&IndependentFeatureSample],
    scores: &[f64],
    labels: &[bool],
    threshold: f64,
    criteria: &RealScreenTrialCriteria,
) -> Option<([f64; 2], [f64; 2], usize)> {
    if samples.is_empty()
        || samples.len() != scores.len()
        || scores.len() != labels.len()
        || criteria.bootstrap_replicates == 0
    {
        return None;
    }
    let mut clusters = BTreeMap::<&str, Vec<usize>>::new();
    for (index, sample) in samples.iter().enumerate() {
        clusters
            .entry(sample.screen_unit.as_str())
            .or_default()
            .push(index);
    }
    let clusters = clusters.into_values().collect::<Vec<_>>();
    let cluster_bootstrap =
        real_screen_confidence_interval_method(samples) == "screen_unit_cluster_bootstrap";
    if !cluster_bootstrap && !has_both_boolean_classes(labels) {
        return None;
    }
    let mut rng = StdRng::seed_from_u64(criteria.bootstrap_seed);
    let mut balanced_accuracy_values = Vec::with_capacity(criteria.bootstrap_replicates);
    let mut auroc_values = Vec::with_capacity(criteria.bootstrap_replicates);
    for _ in 0..criteria.bootstrap_replicates {
        let indices =
            bootstrap_sample_indices(&clusters, samples.len(), cluster_bootstrap, &mut rng);
        let bootstrap_scores = indices
            .iter()
            .map(|index| scores[*index])
            .collect::<Vec<_>>();
        let bootstrap_labels = indices
            .iter()
            .map(|index| labels[*index])
            .collect::<Vec<_>>();
        if bootstrap_labels.iter().any(|label| *label)
            && bootstrap_labels.iter().any(|label| !*label)
        {
            balanced_accuracy_values.push(balanced_accuracy_at_threshold(
                &bootstrap_scores,
                &bootstrap_labels,
                threshold,
            ));
            auroc_values.push(auroc(&bootstrap_scores, &bootstrap_labels));
        }
    }
    if balanced_accuracy_values.is_empty() {
        return None;
    }
    Some((
        [
            percentile_option(&balanced_accuracy_values, 0.025)?,
            percentile_option(&balanced_accuracy_values, 0.975)?,
        ],
        [
            percentile_option(&auroc_values, 0.025)?,
            percentile_option(&auroc_values, 0.975)?,
        ],
        balanced_accuracy_values.len(),
    ))
}

fn bootstrap_sample_indices(
    clusters: &[Vec<usize>],
    sample_count: usize,
    cluster_bootstrap: bool,
    rng: &mut StdRng,
) -> Vec<usize> {
    debug_assert!(!real_screen_bootstrap_sampler_name(cluster_bootstrap).is_empty());
    if cluster_bootstrap {
        bootstrap_cluster_indices_with_sampler(clusters, |cluster_count| {
            rng.random_range(0..cluster_count)
        })
    } else {
        (0..sample_count)
            .map(|_| rng.random_range(0..sample_count))
            .collect()
    }
}

fn bootstrap_cluster_indices_with_sampler<F>(
    clusters: &[Vec<usize>],
    mut select_cluster: F,
) -> Vec<usize>
where
    F: FnMut(usize) -> usize,
{
    if clusters.is_empty() {
        return Vec::new();
    }
    let mut indices = Vec::new();
    for _ in 0..clusters.len() {
        let selected = select_cluster(clusters.len());
        assert!(
            selected < clusters.len(),
            "bootstrap sampler selected an invalid cluster"
        );
        indices.extend(&clusters[selected]);
    }
    indices
}

fn real_screen_bootstrap_sampler_name(cluster_bootstrap: bool) -> &'static str {
    if cluster_bootstrap {
        "StdRng::random_range_uniform_cluster_sampling"
    } else {
        "StdRng::random_range_uniform_record_sampling"
    }
}

#[cfg(test)]
fn binomial_99_9_wilson_interval(successes: usize, trials: usize) -> Option<[f64; 2]> {
    if trials == 0 || successes > trials {
        return None;
    }
    let n = trials as f64;
    let p = successes as f64 / n;
    let z = 3.290_526_731_491_925_5_f64;
    let denominator = 1.0 + z * z / n;
    let center = (p + z * z / (2.0 * n)) / denominator;
    let half_width = z * ((p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt()) / denominator;
    Some([
        (center - half_width).max(0.0),
        (center + half_width).min(1.0),
    ])
}

fn standardize_value_samples(
    samples: &[&IndependentFeatureSample],
    standardization: &FeatureStandardization,
) -> Vec<Vec<f64>> {
    samples
        .iter()
        .map(|sample| {
            sample
                .features
                .iter()
                .zip(&standardization.mean)
                .zip(&standardization.scale)
                .map(|((value, mean), scale)| (*value - *mean) / *scale)
                .collect()
        })
        .collect()
}

fn select_binary_decision_threshold(scores: &[f64], labels: &[bool], fallback: f64) -> f64 {
    let mut candidates = vec![0.0, 1.0, fallback];
    candidates.extend(scores.iter().copied().filter(|score| score.is_finite()));
    candidates.sort_by(f64::total_cmp);
    candidates.dedup_by(|left, right| (*left - *right).abs() <= 1.0e-12);
    let midpoints = candidates
        .windows(2)
        .map(|window| (window[0] + window[1]) * 0.5)
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    candidates.extend(midpoints);
    candidates
        .into_iter()
        .max_by(|left, right| {
            let left_score = (
                balanced_accuracy_at_threshold(scores, labels, *left),
                sensitivity_specificity(scores, labels, *left).0
                    + sensitivity_specificity(scores, labels, *left).1
                    - 1.0,
                -(left - fallback).abs(),
            );
            let right_score = (
                balanced_accuracy_at_threshold(scores, labels, *right),
                sensitivity_specificity(scores, labels, *right).0
                    + sensitivity_specificity(scores, labels, *right).1
                    - 1.0,
                -(right - fallback).abs(),
            );
            left_score
                .partial_cmp(&right_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(fallback)
}

fn read_learning_context(path: &Path) -> Result<LearningEvaluationContext, Box<dyn Error>> {
    let manifest_bytes = fs::read(path)?;
    let manifest: LearningEvaluationManifest = serde_json::from_slice(&manifest_bytes)?;
    if manifest.schema_version != RATE_EVALUATION_SCHEMA_VERSION {
        return Err("unsupported learning evaluation manifest schema version".into());
    }
    let base_manifest_path = resolve_path(
        path.parent().unwrap_or_else(|| Path::new(".")),
        &manifest.base_evaluation_manifest,
    );
    let base = read_context(&base_manifest_path)?;
    if sha256_bytes(&fs::read(&base_manifest_path)?) != manifest.base_evaluation_manifest_sha256 {
        return Err("learning base evaluation manifest hash mismatch".into());
    }
    let criteria_path = resolve_path(
        path.parent().unwrap_or_else(|| Path::new(".")),
        &manifest.criteria_file,
    );
    let criteria_bytes = fs::read(criteria_path)?;
    let criteria: LearningEvaluationCriteria = serde_json::from_slice(&criteria_bytes)?;
    validate_learning_criteria(&criteria)?;
    if sha256_bytes(&criteria_bytes) != manifest.criteria_sha256 {
        return Err("learning evaluation criteria hash mismatch".into());
    }
    if base.manifest.canonical_pack != manifest.canonical_pack
        || base.manifest.canonical_pack_manifest_sha256 != manifest.canonical_pack_manifest_sha256
        || base.manifest.fixtures_file != manifest.fixtures_file
        || base.manifest.fixtures_sha256 != manifest.fixtures_sha256
    {
        return Err("learning manifest and base manifest disagree on canonical inputs".into());
    }
    if base.criteria.frozen.steps != criteria.rate_steps_per_observation
        || base.criteria.training.projection_dimension != criteria.projection_dimension
    {
        return Err(
            "learning criteria and rate evaluation criteria disagree on model shape".into(),
        );
    }
    let pack_manifest_path = Path::new(&manifest.canonical_pack).join("rate_manifest.json");
    if sha256_bytes(&fs::read(pack_manifest_path)?) != manifest.canonical_pack_manifest_sha256 {
        return Err("learning evaluation pack manifest hash mismatch".into());
    }
    let schedule = learning_schedule(
        &criteria,
        &manifest.base_evaluation_manifest_sha256,
        &base.fixtures,
    )?;
    validate_learning_split_image_hashes(&schedule)?;
    let schedule_bytes = serde_json::to_vec(&schedule)?;
    if schedule_bytes != manifest.seed_schedule_canonical_bytes.as_bytes()
        || sha256_bytes(&schedule_bytes) != manifest.seed_schedule_sha256
    {
        return Err("learning evaluation seed schedule mismatch".into());
    }
    Ok(LearningEvaluationContext {
        manifest,
        manifest_sha256: sha256_bytes(&manifest_bytes),
        criteria,
        base,
    })
}

fn validate_learning_criteria(criteria: &LearningEvaluationCriteria) -> Result<(), Box<dyn Error>> {
    let real_screen_trial = &criteria.real_screen.trial;
    let real_screen_adoption = &criteria.real_screen.adoption;
    if criteria.schema_version != RATE_EVALUATION_SCHEMA_VERSION
        || criteria.presentation_count != criteria.pair_count.saturating_mul(6)
        || criteria.labeled_presentation_count != criteria.presentation_count
        || criteria.pair_count == 0
        || criteria.train_pair_count == 0
        || criteria.tuning_pair_count == 0
        || criteria.evaluation_pair_count == 0
        || criteria
            .train_pair_count
            .saturating_add(criteria.tuning_pair_count)
            .saturating_add(criteria.evaluation_pair_count)
            != criteria.pair_count
        || criteria.held_out_pair_count == 0
        || criteria.input_order != "pair_index_then_kind_then_a_then_b"
        || criteria.unlabeled_side != "none"
        || criteria.labels.automatic_teacher != "same_is_no_change_near_and_unrelated_are_change"
        || criteria.labels.independent.no_change != "human_notification_label_no"
        || criteria.labels.independent.small_change != "human_notification_label_uncertain"
        || criteria.labels.independent.change != "human_notification_label_yes"
        || criteria.labels.independent.hold != "unannotated_or_technical_exclusion_is_hold"
        || criteria.labels.independent.positive_mapping
            != "human_yes_positive_uncertain_reported_separately_hold_is_not_removed_from_denominator"
        || criteria.distance_metric != "minimum_normalized_rms_to_recent_history"
        || !criteria.previous_absolute_difference_scale.is_finite()
        || criteria.previous_absolute_difference_scale <= 0.0
        || !criteria.history_elapsed_time_scale_seconds.is_finite()
        || criteria.history_elapsed_time_scale_seconds <= 0.0
        || !matches!(criteria.training_mode.as_str(), "full" | "readout_only")
        || criteria.recent_observations == 0
        || criteria.rate_steps_per_observation == 0
        || criteria.projection_dimension == 0
        || !(8..=32).contains(&criteria.response_projection_dimension)
        || criteria.readout_feature_definition
            != "group_mean_rms_abs_active_scaled_delta_fixed_projection"
        || !criteria.readout_l2_regularization.is_finite()
        || criteria.readout_l2_regularization < 0.0
        || !criteria.readout_learning_rate.is_finite()
        || criteria.readout_learning_rate <= 0.0
        || !criteria.readout_reaction_threshold.is_finite()
        || !(0.0..=1.0).contains(&criteria.readout_reaction_threshold)
        || criteria.decision_threshold_selection
            != "tuning_split_mean_balanced_accuracy_then_mean_youden_then_criteria_distance"
        || criteria.batch_epochs == 0
        || criteria.batch_epochs > 10_000
        || criteria.batch_learning_rates.is_empty()
        || criteria
            .batch_learning_rates
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
        || criteria.batch_l2_values.is_empty()
        || criteria
            .batch_l2_values
            .iter()
            .any(|value| !value.is_finite() || *value < 0.0)
        || !criteria.minimum_near_neural_l2.is_finite()
        || criteria.minimum_near_neural_l2 <= 0.0
        || criteria.minimum_unrelated_image_count < criteria.pair_count
        || !criteria.feature_separation.minimum_pair_l2.is_finite()
        || criteria.feature_separation.minimum_pair_l2 <= 0.0
        || !criteria
            .feature_separation
            .minimum_unrelated_to_near_p50_ratio
            .is_finite()
        || criteria
            .feature_separation
            .minimum_unrelated_to_near_p50_ratio
            <= 1.0
        || criteria.behavior.initial_input_id.is_empty()
        || criteria.behavior.repeat_input_id.is_empty()
        || criteria.behavior.different_input_id.is_empty()
        || criteria.behavior.reversal_input_id.is_empty()
        || !criteria.behavior.rest_duration_s.is_finite()
        || criteria.behavior.rest_duration_s <= 0.0
        || !criteria.behavior.reversal_strength.is_finite()
        || !(0.0..=1.0).contains(&criteria.behavior.reversal_strength)
        || criteria.behavior.rest_protocol != "advance_timestamp_without_observation"
        || criteria.behavior.long_repeat_count < 10
        || !criteria.behavior.history_max_age_seconds.is_finite()
        || criteria.behavior.history_max_age_seconds <= 0.0
        || criteria.behavior.rest_duration_s <= criteria.behavior.history_max_age_seconds
        || criteria.behavior.food_branch_protocol
            != "same_start_time_branch_food_and_no_food_then_re_evaluate"
        || !criteria
            .behavior_acceptance
            .minimum_probability_margin
            .is_finite()
        || criteria.behavior_acceptance.minimum_probability_margin < 0.0
        || !criteria
            .behavior_acceptance
            .minimum_rest_recovery_delta
            .is_finite()
        || criteria.behavior_acceptance.minimum_rest_recovery_delta < 0.0
        || criteria.behavior_acceptance.rest_recovery_rule
            != "after_rest_baseline_reset_and_external_hold"
        || !criteria
            .behavior_acceptance
            .maximum_other_input_side_effect_delta
            .is_finite()
        || criteria
            .behavior_acceptance
            .maximum_other_input_side_effect_delta
            < 0.0
        || (criteria.behavior_acceptance.require_reverse_food_change
            && criteria.behavior.reversal_strength <= 0.0)
        || !criteria.distance_threshold.is_finite()
        || criteria.distance_threshold <= 0.0
        || criteria.case_memory.distance_metric != "l2"
        || criteria.case_memory.match_policy != "exact_image_sha256"
        || !criteria.case_memory.distance_threshold.is_finite()
        || criteria.case_memory.distance_threshold <= 0.0
        || !criteria.case_memory.time_constant_seconds.is_finite()
        || criteria.case_memory.time_constant_seconds <= 0.0
        || !criteria.case_memory.logit_scale.is_finite()
        || criteria.case_memory.logit_scale < 0.0
        || criteria.case_memory.max_cases == 0
        || real_screen_trial.bootstrap_replicates == 0
        || real_screen_trial.bootstrap_replicates > 100_000
        || !real_screen_trial.ba_ci_half_width_target.is_finite()
        || real_screen_trial.ba_ci_half_width_target <= 0.0
        || real_screen_trial.ba_ci_half_width_target >= 0.5
        || real_screen_trial.required_class_count_method
            != "conservative_95pct_ba_half_width_z_squared_p_max_over_2_half_width_squared"
        || real_screen_trial.confidence_interval_method
            != "screen_unit_cluster_bootstrap_for_adoption_record_bootstrap_for_trial_only"
        || real_screen_trial.minimum_total_record_count == 0
        || real_screen_trial.maximum_total_record_count
            < real_screen_trial.minimum_total_record_count
        || real_screen_adoption.minimum_evaluation_record_count == 0
        || real_screen_adoption.minimum_evaluation_session_count < 2
        || real_screen_adoption.minimum_evaluation_record_count
            < real_screen_adoption
                .minimum_independent_positive_count_for_claim
                .saturating_add(real_screen_adoption.minimum_independent_negative_count_for_claim)
        || real_screen_adoption.minimum_independent_positive_count_for_claim == 0
        || real_screen_adoption.minimum_independent_negative_count_for_claim == 0
        || real_screen_adoption.minimum_independent_positive_screen_unit_count_for_claim < 2
        || real_screen_adoption.minimum_independent_negative_screen_unit_count_for_claim < 2
        || !real_screen_adoption.maximum_hold_fraction.is_finite()
        || !(0.0..=1.0).contains(&real_screen_adoption.maximum_hold_fraction)
        || real_screen_adoption.performance_condition != "current_response_plus_history"
        || !real_screen_adoption.minimum_balanced_accuracy.is_finite()
        || !(0.0..=1.0).contains(&real_screen_adoption.minimum_balanced_accuracy)
        || !real_screen_adoption.minimum_auroc.is_finite()
        || !(0.0..=1.0).contains(&real_screen_adoption.minimum_auroc)
        || !real_screen_adoption
            .minimum_paired_balanced_accuracy_difference_lower_bound
            .is_finite()
        || !(0.0..=1.0)
            .contains(&real_screen_adoption.minimum_paired_balanced_accuracy_difference_lower_bound)
        || real_screen_adoption.paired_difference_rationale.is_empty()
        || !real_screen_trial.maximum_hold_fraction.is_finite()
        || !(0.0..=1.0).contains(&real_screen_trial.maximum_hold_fraction)
        || real_screen_trial.expected_record_counts_by_split.is_empty()
        || real_screen_trial.session_split_by_manifest.is_empty()
        || real_screen_trial
            .session_split_by_manifest
            .values()
            .any(|split| {
                !matches!(
                    split.as_str(),
                    "training" | "tuning" | "evaluation" | "held_out"
                )
            })
        || real_screen_trial.split_assignment
            != "capture_session_order_training_evaluation_then_tuning_held_out"
        || real_screen_trial.history_boundary != "stream_id_plus_split"
    {
        return Err("learning criteria has an invalid fixed protocol".into());
    }
    if criteria.brain_roles.len() != 2
        || criteria.brain_roles.iter().any(|brain| {
            brain.id.is_empty() || !matches!(brain.role.as_str(), "change" | "no_change")
        })
        || criteria.brain_roles[0].id == criteria.brain_roles[1].id
        || criteria
            .brain_roles
            .iter()
            .map(|brain| brain.role.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            != ["change", "no_change"].into_iter().collect()
    {
        return Err("learning criteria must define one change and one no_change brain".into());
    }
    let acceptance = &criteria.acceptance;
    if !acceptance.minimum_balanced_accuracy.is_finite()
        || !acceptance.minimum_auroc.is_finite()
        || !acceptance.minimum_consensus_accuracy.is_finite()
        || !acceptance.minimum_unused_screen_accuracy.is_finite()
        || !acceptance.consensus_margin.is_finite()
        || !(0.0..=1.0).contains(&acceptance.minimum_balanced_accuracy)
        || !(0.0..=1.0).contains(&acceptance.minimum_auroc)
        || !(0.0..=1.0).contains(&acceptance.minimum_consensus_accuracy)
        || !(0.0..=1.0).contains(&acceptance.minimum_unused_screen_accuracy)
        || !(0.0..0.5).contains(&acceptance.consensus_margin)
        || !acceptance.saturation_is_reported_not_silently_ignored
    {
        return Err("learning acceptance criteria are invalid".into());
    }
    let resources = &criteria.resource_contract;
    if resources.maximum_peak_rss_bytes != 14 * 1024 * 1024 * 1024
        || resources.inference_target_peak_rss_bytes != 4 * 1024 * 1024 * 1024
        || resources.measurement != "/usr/bin/time -l"
        || resources.build_jobs != 4
    {
        return Err("learning resource contract is not the fixed contract".into());
    }
    for feedback in &criteria.human_feedback {
        if feedback.input_id.is_empty()
            || feedback.event_id.is_empty()
            || !matches!(feedback.kind.as_str(), "reward" | "punish")
            || !feedback.strength.is_finite()
            || !(0.0..=1.0).contains(&feedback.strength)
        {
            return Err("learning human feedback contains an invalid event".into());
        }
    }
    Ok(())
}

fn learning_schedule(
    criteria: &LearningEvaluationCriteria,
    base_manifest_sha256: &str,
    fixtures: &FixtureManifest,
) -> Result<LearningSeedSchedule, Box<dyn Error>> {
    let criteria_sha256 = sha256_bytes(&serde_json::to_vec(criteria)?);
    let mut entries = Vec::with_capacity(criteria.presentation_count);
    for pair_index in 0..criteria.pair_count {
        for (kind, _) in [
            (ScreenFixtureKind::Same, "same"),
            (ScreenFixtureKind::Near, "near"),
            (ScreenFixtureKind::Unrelated, "unrelated"),
        ] {
            for side in ["a", "b"] {
                let record = fixtures
                    .records
                    .iter()
                    .find(|record| {
                        record.kind == kind
                            && record.pair_index == pair_index
                            && record.side == side
                    })
                    .ok_or_else(|| {
                        format!("missing learning fixture {kind:?}-{pair_index}-{side}")
                    })?;
                entries.push(LearningScheduleEntry {
                    id: record.id.clone(),
                    kind,
                    pair_index,
                    side: side.to_owned(),
                    timestamp_s: entries.len() as f64,
                    image_sha256: record.sha256.clone(),
                    expected_change: kind != ScreenFixtureKind::Same,
                });
            }
        }
    }
    let training_entry_count = criteria.train_pair_count * 6;
    let tuning_entry_count = criteria.tuning_pair_count * 6;
    let evaluation_entry_count = criteria.evaluation_pair_count * 6;
    let training_entries = entries[..training_entry_count].to_vec();
    let tuning_entries =
        entries[training_entry_count..training_entry_count + tuning_entry_count].to_vec();
    let evaluation_entries = entries[training_entry_count + tuning_entry_count
        ..training_entry_count + tuning_entry_count + evaluation_entry_count]
        .to_vec();
    let mut held_out_entries = Vec::with_capacity(criteria.held_out_pair_count * 6);
    for pair_offset in 0..criteria.held_out_pair_count {
        let pair_index = criteria.pair_count + pair_offset;
        for (kind, kind_name) in [
            (ScreenFixtureKind::Same, "same"),
            (ScreenFixtureKind::Near, "near"),
            (ScreenFixtureKind::Unrelated, "unrelated"),
        ] {
            for side in ["a", "b"] {
                let fixture = ScreenFixture::generate(
                    kind,
                    pair_index,
                    side,
                    fixtures.width,
                    fixtures.height,
                )?;
                held_out_entries.push(LearningScheduleEntry {
                    id: format!("heldout-{kind_name}-{pair_offset:02}-{side}"),
                    kind,
                    pair_index,
                    side: side.to_owned(),
                    timestamp_s: (criteria.presentation_count + held_out_entries.len()) as f64,
                    image_sha256: fixture.sha256,
                    expected_change: kind != ScreenFixtureKind::Same,
                });
            }
        }
    }
    let schedule = LearningSeedSchedule {
        criteria_sha256,
        base_evaluation_manifest_sha256: base_manifest_sha256.to_owned(),
        presentation_count: criteria.presentation_count,
        pair_count: criteria.pair_count,
        input_order: criteria.input_order.clone(),
        unlabeled_side: criteria.unlabeled_side.clone(),
        distance_metric: criteria.distance_metric.clone(),
        distance_threshold_bits: criteria.distance_threshold.to_bits(),
        previous_absolute_difference_scale_bits: criteria
            .previous_absolute_difference_scale
            .to_bits(),
        history_elapsed_time_scale_seconds_bits: criteria
            .history_elapsed_time_scale_seconds
            .to_bits(),
        brain_roles: criteria.brain_roles.clone(),
        seeds: criteria.seeds.clone(),
        entries,
        training_entries,
        tuning_entries,
        evaluation_entries,
        held_out_entries,
    };
    validate_learning_split_image_hashes(&schedule)?;
    Ok(schedule)
}

fn validate_learning_split_image_hashes(
    schedule: &LearningSeedSchedule,
) -> Result<(), Box<dyn Error>> {
    let splits = [
        ("training", &schedule.training_entries),
        ("tuning", &schedule.tuning_entries),
        ("evaluation", &schedule.evaluation_entries),
        ("held_out", &schedule.held_out_entries),
    ];
    let mut hashes = BTreeMap::<&str, BTreeSet<&str>>::new();
    for (name, entries) in splits {
        let set = hashes.entry(name).or_default();
        for entry in entries {
            set.insert(entry.image_sha256.as_str());
        }
    }
    let split_names = ["training", "tuning", "evaluation", "held_out"];
    for (left_index, left_name) in split_names.iter().enumerate() {
        for right_name in split_names.iter().skip(left_index + 1) {
            if let Some(hash) = hashes
                .get(*left_name)
                .expect("split hash set exists")
                .intersection(hashes.get(*right_name).expect("split hash set exists"))
                .next()
            {
                return Err(format!(
                    "learning split image hash overlap: {left_name} and {right_name} share {hash}"
                )
                .into());
            }
        }
    }
    Ok(())
}

fn audit(values: &BTreeMap<String, String>) -> Result<(), Box<dyn Error>> {
    let manifest_path = required_path(values, "--evaluation-manifest")?;
    let output = required_path(values, "--output")?;
    reject_unknown(values, &["--evaluation-manifest", "--output"])?;
    let context = read_context(&manifest_path)?;
    let graph = RateGraph::load(&context.manifest.canonical_pack)?;
    let retina = RetinaMap::from_graph(&graph, retina_config(&context.criteria))?;
    let retina_contract_passed = retina_contract_passed(&retina.audit, &context.criteria.retina);
    let mut pairs = Vec::new();
    let mut exact_duplicate_pair_count = 0;
    let mut minimum_distinct_l2_distance = f64::INFINITY;
    let mut maximum_input_l1_relative_difference = 0.0_f64;
    let mut maximum_input_l2_relative_difference = 0.0_f64;
    for pair_index in 0..context.fixtures.pair_count {
        for (kind, name) in [
            (ScreenFixtureKind::Same, "same"),
            (ScreenFixtureKind::Near, "near"),
            (ScreenFixtureKind::Unrelated, "unrelated"),
        ] {
            let left = fixture_for(&context.fixtures, kind, pair_index, "a", name)?;
            let right = fixture_for(&context.fixtures, kind, pair_index, "b", name)?;
            let left_image = left.image()?;
            let right_image = right.image()?;
            let (l2, cosine, direction) =
                image_feature_metrics(&left_image.pixels, &right_image.pixels);
            let left_input = retina.encode(&left_image, graph.neuron_count())?.input;
            let right_input = retina.encode(&right_image, graph.neuron_count())?.input;
            let left_input_l1 = l1_norm(&left_input);
            let right_input_l1 = l1_norm(&right_input);
            let input_l1_relative_difference = relative_difference(left_input_l1, right_input_l1);
            let left_input_l2 = l2_norm(&left_input);
            let right_input_l2 = l2_norm(&right_input);
            let input_l2_relative_difference = relative_difference(left_input_l2, right_input_l2);
            maximum_input_l1_relative_difference =
                maximum_input_l1_relative_difference.max(input_l1_relative_difference);
            maximum_input_l2_relative_difference =
                maximum_input_l2_relative_difference.max(input_l2_relative_difference);
            if l2 <= context.criteria.audit.same_l2_max {
                exact_duplicate_pair_count += 1;
            } else {
                minimum_distinct_l2_distance = minimum_distinct_l2_distance.min(l2);
            }
            pairs.push(FeaturePairReport {
                id: format!("{name}-{pair_index:02}"),
                kind,
                l2_distance: l2,
                cosine_similarity: cosine,
                direction_distance: direction,
                left_input_l1,
                right_input_l1,
                input_l1_relative_difference,
                left_input_l2,
                right_input_l2,
                input_l2_relative_difference,
            });
        }
    }
    let near_min = pairs
        .iter()
        .filter(|pair| pair.kind == ScreenFixtureKind::Near)
        .map(|pair| pair.direction_distance)
        .fold(f64::INFINITY, f64::min);
    let unrelated_min = pairs
        .iter()
        .filter(|pair| pair.kind == ScreenFixtureKind::Unrelated)
        .map(|pair| pair.direction_distance)
        .fold(f64::INFINITY, f64::min);
    let passed = input_audit_decision(
        &context.criteria.audit,
        exact_duplicate_pair_count,
        near_min,
        unrelated_min,
        retina_contract_passed,
    );
    let report = RateInputAuditReport {
        manifest_sha256: context.manifest_sha256,
        criteria_sha256: context.manifest.criteria_sha256,
        fixtures_sha256: context.manifest.fixtures_sha256,
        pack_manifest_sha256: context.manifest.canonical_pack_manifest_sha256,
        seed_schedule_sha256: context.manifest.seed_schedule_sha256,
        retina_audit: retina.audit,
        retina_contract_passed,
        pair_count: pairs.len(),
        feature_vector_count: context.fixtures.records.len(),
        exact_duplicate_pair_count,
        minimum_distinct_l2_distance,
        maximum_input_l1_relative_difference,
        maximum_input_l2_relative_difference,
        pairs,
        passed,
    };
    write_json(&output, &report)
}

fn evaluate(values: &BTreeMap<String, String>) -> Result<(), Box<dyn Error>> {
    let manifest_path = required_path(values, "--evaluation-manifest")?;
    let output = required_path(values, "--output")?;
    let peak_rss_bytes = values
        .get("--peak-rss-bytes")
        .map(|value| value.parse::<u64>())
        .transpose()?;
    reject_unknown(
        values,
        &["--evaluation-manifest", "--output", "--peak-rss-bytes"],
    )?;
    let context = read_context(&manifest_path)?;
    let pack_load_started = Instant::now();
    let graph = RateGraph::load(&context.manifest.canonical_pack)?;
    let pack_load_ms = pack_load_started.elapsed().as_secs_f64() * 1_000.0;
    let pack_load_rss = process_peak_rss_bytes();
    let pack_manifest_sha256 = sha256_bytes(&fs::read(
        Path::new(&context.manifest.canonical_pack).join("rate_manifest.json"),
    )?);
    let graph_fingerprint_before_learning = graph.fingerprint();
    let retina_build_started = Instant::now();
    let retina = RetinaMap::from_graph(&graph, retina_config(&context.criteria))?;
    let retina_build_ms = retina_build_started.elapsed().as_secs_f64() * 1_000.0;
    let retina_build_rss = process_peak_rss_bytes();
    let retina_contract_passed = retina_contract_passed(&retina.audit, &context.criteria.retina);
    let readout_indices = graph
        .population(&context.criteria.readout_population)
        .map_err(|error| Box::<dyn Error>::from(error.to_string()))?
        .to_vec();
    let learning_config = RateLearningConfig {
        steps_per_observation: context.criteria.frozen.steps,
        ..RateLearningConfig::default()
    };
    let mut state = RateLearningState::new(
        &graph,
        readout_indices.clone(),
        context.criteria.training.projection_dimension,
        context.criteria.training.seed,
        learning_config,
    )?;
    let rest_started = Instant::now();
    let (rest_activity, rest_convergence_max_abs) = state.rest_state(&graph)?;
    let rest_ms = rest_started.elapsed().as_secs_f64() * 1_000.0;
    let rest_rss = process_peak_rss_bytes();
    let response_groups = build_response_groups(&graph, &rest_activity, &context.criteria.frozen)?;
    let response_group_reports = response_groups
        .iter()
        .map(|group| group.report.clone())
        .collect::<Vec<_>>();
    let mut observations = Vec::new();
    let mut responses: BTreeMap<String, ResponseSnapshot> = BTreeMap::new();
    let mut inference_times = Vec::new();
    let mut fixture_decode_times = Vec::new();
    let mut retina_encode_times = Vec::new();
    let mut forward_readout_times = Vec::new();
    let mut response_times = Vec::new();
    let mut all_rate_values_finite = true;
    let mut first_activity = None;
    let mut first_input = None;
    for record in &context.fixtures.records {
        let total_started = Instant::now();
        let fixture_started = Instant::now();
        let fixture = fixture_from_record(record)?;
        let image = fixture.image()?;
        fixture_decode_times.push(fixture_started.elapsed().as_secs_f64() * 1_000.0);
        let encode_started = Instant::now();
        let input = retina.encode(&image, graph.neuron_count())?;
        retina_encode_times.push(encode_started.elapsed().as_secs_f64() * 1_000.0);
        let forward_started = Instant::now();
        let (activity, readout) = state.predict_final(&graph, &input.input)?;
        all_rate_values_finite &= activity.iter().all(|value| value.is_finite());
        forward_readout_times.push(forward_started.elapsed().as_secs_f64() * 1_000.0);
        let response_started = Instant::now();
        let snapshot = response_snapshot(
            &activity,
            &rest_activity,
            &response_groups,
            state.parameters.hmax,
            context.criteria.frozen.zero_response_epsilon,
        );
        response_times.push(response_started.elapsed().as_secs_f64() * 1_000.0);
        let elapsed_ms = total_started.elapsed().as_secs_f64() * 1_000.0;
        inference_times.push(elapsed_ms);
        if first_activity.is_none() && record.id == "same-00-a" {
            first_activity = Some(activity.clone());
            first_input = Some(input.input.clone());
        }
        responses.insert(record.id.clone(), snapshot.clone());
        observations.push(RateObservationReport {
            id: record.id.clone(),
            kind: record.kind,
            elapsed_ms,
            response_norm: snapshot.raw_norm,
            active_readout_fraction: snapshot.active_fraction,
            raw_response_norm: snapshot.raw_norm,
            delta_response_norm: snapshot.delta_norm,
            scaled_delta_response_norm: snapshot.scaled_delta_norm,
            active_response_fraction: snapshot.active_fraction,
            saturation_fraction: snapshot.saturation_fraction,
            components: snapshot.components,
            value_probability: readout.probability,
        });
    }
    let inference_rss = process_peak_rss_bytes();
    let frozen = frozen_report(&context, &responses, &observations);
    let first_activity = first_activity.ok_or("same-00-a was not generated")?;
    let first_input = first_input.ok_or("same-00-a input was not generated")?;
    let (_, rate_timing) = final_activity_with_parameters_profile(
        &graph,
        &state.parameters,
        &first_input,
        context.criteria.frozen.steps,
        Some(&rest_activity),
    )?;
    let mut habituation =
        HabituationState::new(graph.neuron_count(), HabituationConfig::default())?;
    let rest = vec![0.0; graph.neuron_count()];
    let scales = vec![1.0; graph.neuron_count()];
    let first_value = state.predict_final(&graph, &first_input)?.1.probability;
    let first_habituation = habituation.observe(&first_activity, &rest, &scales, 0.0, 1.0)?;
    let repeated_habituation = habituation.observe(&first_activity, &rest, &scales, 0.0, 1.0)?;
    let recovered_habituation = habituation.observe(&first_activity, &rest, &scales, 600.0, 0.0)?;
    let second_value = state.predict_final(&graph, &first_input)?.1.probability;
    let habituation_report = RateHabituationReport {
        first_novelty: first_habituation.novelty_score,
        repeated_novelty: repeated_habituation.novelty_score,
        recovered_novelty: recovered_habituation.novelty_score,
        repeated_activity_norm: l2_norm(&repeated_habituation.habituated_activity),
        recovered_activity_norm: l2_norm(&recovered_habituation.habituated_activity),
        value_probability_before: first_value,
        value_probability_after: second_value,
        value_change: (second_value - first_value).abs(),
        response_sufficient: first_habituation.response_sufficient,
    };
    let mut learning = Vec::new();
    let mut learning_times = Vec::new();
    let mut maximum_nonzero_edge_gradient_fraction: f64 = 0.0;
    for (phase, event) in [
        (
            "reward",
            FeedbackEvent::new("reward-00", "same-00-a", FeedbackKind::Reward, 1.0, 0.0)?,
        ),
        (
            "punish",
            FeedbackEvent::new(
                "punish-00",
                "unrelated-00-a",
                FeedbackKind::Punish,
                1.0,
                0.0,
            )?,
        ),
        (
            "reversal-reward",
            FeedbackEvent::new(
                "reversal-reward-00",
                "unrelated-00-a",
                FeedbackKind::Reward,
                1.0,
                1.0,
            )?,
        ),
        (
            "reversal-punish",
            FeedbackEvent::new(
                "reversal-punish-00",
                "same-00-a",
                FeedbackKind::Punish,
                1.0,
                1.0,
            )?,
        ),
    ] {
        let input = fixture_input(&context, &retina, &graph, &event.input_id)?;
        let probability_before = state.predict_final(&graph, &input)?.1.probability;
        let started = Instant::now();
        let update = state.train_observation(&graph, &input, &event, TrainingMode::Full)?;
        maximum_nonzero_edge_gradient_fraction =
            maximum_nonzero_edge_gradient_fraction.max(update.gradient_nonzero_log_gain_fraction);
        learning_times.push(started.elapsed().as_secs_f64() * 1_000.0);
        let probability_after = state.predict_final(&graph, &input)?.1.probability;
        learning.push(RateLearningPhaseReport {
            phase: phase.to_owned(),
            input_id: event.input_id,
            target: update.target,
            probability_before,
            probability_after,
            loss: update.loss,
            updated: update.updated,
            gradient_nonzero_log_gain_fraction: update.gradient_nonzero_log_gain_fraction,
        });
    }
    let learning_rss = process_peak_rss_bytes();
    let graph_fingerprint_after_learning = graph.fingerprint();
    drop(state);
    let mut feedback = FeedbackStore::default();
    for (index, record) in context.fixtures.records.iter().enumerate() {
        let kind = match record.kind {
            ScreenFixtureKind::Same => FeedbackKind::Reward,
            ScreenFixtureKind::Near | ScreenFixtureKind::Unrelated => FeedbackKind::Punish,
        };
        feedback.insert(FeedbackEvent::new(
            format!("control-{index:03}"),
            record.id.clone(),
            kind,
            1.0,
            index as f64,
        )?)?;
    }
    let shuffled_feedback =
        shuffle_feedback(&feedback, context.criteria.training.feedback_shuffle_seed);
    let controls_started = Instant::now();
    let wiring = shuffle_wiring(
        &graph,
        context.criteria.training.feedback_shuffle_seed,
        context.criteria.training.wiring_swaps,
    )?;
    let wiring_stats = wiring.stats.clone();
    drop(wiring);
    let weights = shuffle_weights(&graph, context.criteria.training.weight_shuffle_seed)?;
    let weight_stats = weights.stats.clone();
    drop(weights);
    let controls_ms = controls_started.elapsed().as_secs_f64() * 1_000.0;
    let controls_rss = process_peak_rss_bytes();
    let controls = RateControlReport {
        feedback_source_count: shuffled_feedback.source_event_count,
        feedback_shuffled_match_count: shuffled_feedback.matched_event_count,
        wiring_attempts: wiring_stats.attempts,
        wiring_successes: wiring_stats.successes,
        wiring_rejected: wiring_stats.rejected,
        wiring_original_edge_overlap: wiring_stats.original_edge_overlap,
        wiring_changed_edge_count: wiring_stats.changed_edge_count,
        weight_changed_edge_count: weight_stats.changed_edge_count,
    };
    let final_rss = process_peak_rss_bytes();
    let measured_peak_rss = peak_rss_bytes.or(final_rss);
    let performance = RatePerformanceReport {
        inference_sample_count: inference_times.len(),
        inference_p50_ms: percentile(&inference_times, 0.50),
        inference_p95_ms: percentile(&inference_times, 0.95),
        learning_sample_count: learning_times.len(),
        learning_p50_ms: percentile(&learning_times, 0.50),
        learning_p95_ms: percentile(&learning_times, 0.95),
        peak_rss_bytes: measured_peak_rss,
        inference_scope: "72 generated fixture decodes, RetinaMap encodes, cached rest state, 32 rate updates, readout, and response construction; pack load and map construction are separate stages".to_owned(),
        learning_scope: "four labeled observations, including the train-time forward, RateBackward, gradient clipping, and one Adam update per event; pack load excluded".to_owned(),
        stages: vec![
            stage_report("pack_load", vec![pack_load_ms], pack_load_rss),
            stage_report("retina_map_build", vec![retina_build_ms], retina_build_rss),
            stage_report("rest_state", vec![rest_ms], rest_rss),
            stage_report("fixture_decode", fixture_decode_times, inference_rss),
            stage_report("retina_encode", retina_encode_times, inference_rss),
            stage_report("rate_forward_readout", forward_readout_times, inference_rss),
            stage_report("response_construction", response_times, inference_rss),
            stage_report("inference_total", inference_times.clone(), inference_rss),
            stage_report("learning_total", learning_times.clone(), learning_rss),
            stage_report("controls", vec![controls_ms], controls_rss),
        ],
        rate_operation_breakdown: RateOperationBreakdown {
            sample_count: 1,
            csr_traversal_ms: rate_timing.csr_traversal_ms,
            neuron_update_ms: rate_timing.neuron_update_ms,
            other_ms: rate_timing.other_ms,
        },
        rss_measurement: "macOS getrusage(RUSAGE_SELF).ru_maxrss のプロセス累積 high-water を各段階の終了時に記録した。stage ごとの値はその時点までの上限であり、段階内の瞬間値ではない。外部 --peak-rss-bytes が指定された場合は全体値として併記した。".to_owned(),
    };
    let input_audit_passed = input_audit_passed(&context)? && retina_contract_passed;
    let report = RateEvaluationReport {
        manifest_sha256: context.manifest_sha256.clone(),
        criteria_sha256: context.manifest.criteria_sha256,
        fixtures_sha256: context.manifest.fixtures_sha256,
        pack_manifest_sha256,
        seed_schedule_sha256: context.manifest.seed_schedule_sha256,
        evaluation_manifest_sha256: context.manifest_sha256.clone(),
        rest_steps: RateLearningState::rest_steps(),
        rest_convergence_max_abs,
        retina_audit: retina.audit,
        retina_contract_passed,
        input_audit_passed,
        response_groups: response_group_reports,
        numerical: RateNumericalReport {
            f32_reference_abs_tolerance: context.criteria.numerical.f32_reference_abs_tolerance,
            finite_difference_abs_tolerance: context
                .criteria
                .numerical
                .finite_difference_abs_tolerance,
            nonlinear_boundary_margin: context.criteria.numerical.nonlinear_boundary_margin,
            all_rate_values_finite,
            graph_structure_unchanged: graph_fingerprint_before_learning
                == graph_fingerprint_after_learning,
            graph_fingerprint_before_learning,
            graph_fingerprint_after_learning,
            maximum_nonzero_edge_gradient_fraction,
        },
        frozen,
        habituation: habituation_report,
        learning,
        controls,
        performance,
        observations,
    };
    write_json(&output, &report)
}

fn learning_evaluate(values: &BTreeMap<String, String>) -> Result<(), Box<dyn Error>> {
    let manifest_path = required_path(values, "--learning-manifest")?;
    let output = required_path(values, "--output")?;
    let artifact_output = values
        .get("--artifact-output")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            output
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("malecns-frozen-response-learning-artifact.json")
        });
    let peak_rss_bytes = values
        .get("--peak-rss-bytes")
        .map(|value| value.parse::<u64>())
        .transpose()?;
    reject_unknown(
        values,
        &[
            "--learning-manifest",
            "--output",
            "--artifact-output",
            "--peak-rss-bytes",
        ],
    )?;
    let context = read_learning_context(&manifest_path)?;
    let graph = Arc::new(RateGraph::load(&context.manifest.canonical_pack)?);
    let retina = RetinaMap::from_graph(&graph, retina_config(&context.base.criteria))?;
    let schedule = learning_schedule(
        &context.criteria,
        &context.manifest.base_evaluation_manifest_sha256,
        &context.base.fixtures,
    )?;
    let mut cache_entries = schedule.entries.clone();
    cache_entries.extend(schedule.held_out_entries.clone());
    let parameters = RateParameters::initial(&graph);
    let (cache, cache_report, groups) =
        build_cached_response_cache(&graph, &parameters, &retina, &context, &cache_entries)?;
    let mut three_point_audit = cached_three_point_audit(
        &context,
        &schedule.entries,
        &cache,
        context.criteria.minimum_near_neural_l2,
        context.criteria.minimum_unrelated_image_count,
    )?;
    let response_feature_dimension =
        groups.len() * (4 + context.criteria.response_projection_dimension);
    if response_feature_dimension == 0 {
        return Err("cached response feature dimension is zero".into());
    }
    let training_samples =
        build_batch_samples(&schedule.training_entries, &cache, &context.criteria)?;
    let tuning_samples = build_batch_samples(&schedule.tuning_entries, &cache, &context.criteria)?;
    let evaluation_samples =
        build_batch_samples(&schedule.evaluation_entries, &cache, &context.criteria)?;
    let held_out_samples =
        build_batch_samples(&schedule.held_out_entries, &cache, &context.criteria)?;
    let learning_feature_separation = learning_feature_separation(
        &[&training_samples, &tuning_samples, &evaluation_samples],
        context.criteria.pair_count,
        &context.criteria.feature_separation,
    )?;
    three_point_audit.learning_feature_dimension = training_samples
        .first()
        .map(|sample| sample.augmented_features.len());
    three_point_audit.learning_feature_separation = Some(learning_feature_separation.clone());
    three_point_audit.passed &= learning_feature_separation.passed;
    let standardization = fit_feature_standardization(&training_samples)?;
    let teacher_rule = teacher_rule_report(
        &[
            &training_samples,
            &tuning_samples,
            &evaluation_samples,
            &held_out_samples,
        ],
        context.criteria.distance_threshold,
        response_feature_dimension,
    );

    let learned_feedback = FrozenReadoutFeedback::default();
    let (learned, behavior, learned_batch) =
        run_cached_readout_condition(CachedReadoutCondition {
            name: "learned_readout",
            cache: &cache,
            feature_dimension: response_feature_dimension,
            training_entries: &schedule.training_entries,
            evaluation_entries: &schedule.evaluation_entries,
            held_out_entries: &schedule.held_out_entries,
            training_samples: &training_samples,
            tuning_samples: &tuning_samples,
            evaluation_samples: &evaluation_samples,
            held_out_samples: &held_out_samples,
            standardization: &standardization,
            criteria: &context.criteria,
            training_mode: TrainingMode::ReadoutOnly,
            feedback: learned_feedback.clone(),
            seed_base: context.criteria.seeds.base,
            shuffle_seed: None,
            decision_threshold: None,
            behavior: Some(&context.criteria.behavior),
        })?;
    let decision_threshold = learned_batch.decision_threshold;
    let (frozen, _, frozen_batch) = run_cached_readout_condition(CachedReadoutCondition {
        name: "readout_update_disabled",
        cache: &cache,
        feature_dimension: response_feature_dimension,
        training_entries: &schedule.training_entries,
        evaluation_entries: &schedule.evaluation_entries,
        held_out_entries: &schedule.held_out_entries,
        training_samples: &training_samples,
        tuning_samples: &tuning_samples,
        evaluation_samples: &evaluation_samples,
        held_out_samples: &held_out_samples,
        standardization: &standardization,
        criteria: &context.criteria,
        training_mode: TrainingMode::Frozen,
        feedback: FrozenReadoutFeedback::default(),
        seed_base: context.criteria.seeds.base,
        shuffle_seed: None,
        decision_threshold: Some(decision_threshold),
        behavior: None,
    })?;
    let (reward_shuffled, _, reward_shuffled_batch) =
        run_cached_readout_condition(CachedReadoutCondition {
            name: "reward_shuffled",
            cache: &cache,
            feature_dimension: response_feature_dimension,
            training_entries: &schedule.training_entries,
            evaluation_entries: &schedule.evaluation_entries,
            held_out_entries: &schedule.held_out_entries,
            training_samples: &training_samples,
            tuning_samples: &tuning_samples,
            evaluation_samples: &evaluation_samples,
            held_out_samples: &held_out_samples,
            standardization: &standardization,
            criteria: &context.criteria,
            training_mode: TrainingMode::ReadoutOnly,
            feedback: FrozenReadoutFeedback::default(),
            seed_base: context.criteria.seeds.reward_shuffle,
            shuffle_seed: Some(context.criteria.seeds.reward_shuffle),
            decision_threshold: Some(decision_threshold),
            behavior: None,
        })?;
    let direct_teacher = direct_teacher_condition(
        &evaluation_samples,
        &held_out_samples,
        &context.criteria,
        decision_threshold,
    );
    let learning_presentations = learning_presentation_reports(
        &[
            ("training", &training_samples),
            ("tuning", &tuning_samples),
            ("evaluation", &evaluation_samples),
            ("held_out", &held_out_samples),
        ],
        &context.criteria,
    );
    let batch_training = BatchTrainingReport {
        response_feature_dimension,
        history_feature_dimension: 4,
        augmented_feature_dimension: training_samples
            .first()
            .map_or(0, |sample| sample.augmented_features.len()),
        train_pair_count: context.criteria.train_pair_count,
        tuning_pair_count: context.criteria.tuning_pair_count,
        evaluation_pair_count: context.criteria.evaluation_pair_count,
        held_out_pair_count: context.criteria.held_out_pair_count,
        epochs: context.criteria.batch_epochs,
        learning_rates: context.criteria.batch_learning_rates.clone(),
        l2_values: context.criteria.batch_l2_values.clone(),
        standardization: standardization_report(&standardization),
        teacher_rule,
        decision_threshold_selection: context.criteria.decision_threshold_selection.clone(),
        criteria_reaction_threshold: context.criteria.readout_reaction_threshold,
        conditions: vec![learned_batch, frozen_batch, reward_shuffled_batch],
    };
    let artifact = RateHelperArtifact {
        observation_schema: habitua_connectome::RATE_HELPER_OBSERVATION_SCHEMA.to_owned(),
        artifact_version: 1,
        pack_path: context.manifest.canonical_pack.clone(),
        pack_manifest_sha256: context.manifest.canonical_pack_manifest_sha256.clone(),
        graph_fingerprint: graph.fingerprint(),
        retina_map: retina,
        rate_steps_per_observation: context.criteria.rate_steps_per_observation,
        hmax: parameters.hmax,
        response_projection_dimension: context.criteria.response_projection_dimension,
        history_recent_observations: context.criteria.recent_observations,
        history_distance_threshold: context.criteria.distance_threshold,
        history_previous_absolute_difference_scale: context
            .criteria
            .previous_absolute_difference_scale,
        history_elapsed_time_scale_seconds: context.criteria.history_elapsed_time_scale_seconds,
        history_max_age_seconds: context.criteria.behavior.history_max_age_seconds,
        zero_response_epsilon: context.base.criteria.frozen.zero_response_epsilon,
        response_groups: groups
            .iter()
            .map(|group| RateHelperResponseGroup {
                name: group.report.name.clone(),
                neuron_indices: group.indices.clone(),
                scale: group.report.scale,
            })
            .collect(),
        brains: batch_training
            .conditions
            .iter()
            .find(|condition| condition.name == "learned_readout")
            .map(|condition| condition.selected_readouts.clone())
            .ok_or("learned readout artifact is missing")?,
        consensus_margin: context.criteria.acceptance.consensus_margin,
        case_memory: RateHelperCaseMemory {
            match_policy: "exact_image_sha256".to_owned(),
            time_constant_seconds: context.criteria.case_memory.time_constant_seconds,
            logit_scale: context.criteria.case_memory.logit_scale,
            max_cases: context.criteria.case_memory.max_cases,
        },
    };
    artifact.validate_shape(Some(&graph))?;
    artifact.write(&artifact_output)?;
    let artifact_sha256 = sha256_bytes(&fs::read(&artifact_output)?);
    let report = CachedLearningEvaluationReport {
        report_version: 3,
        manifest_sha256: context.manifest_sha256,
        criteria_sha256: context.manifest.criteria_sha256.clone(),
        base_evaluation_manifest_sha256: context.manifest.base_evaluation_manifest_sha256,
        fixtures_sha256: context.manifest.fixtures_sha256,
        pack_manifest_sha256: context.manifest.canonical_pack_manifest_sha256,
        seed_schedule_sha256: context.manifest.seed_schedule_sha256,
        response_cache: cache_report,
        three_point_audit,
        batch_training,
        learning_presentations,
        conditions: vec![learned, frozen, reward_shuffled, direct_teacher],
        behavior,
        acceptance: context.criteria.acceptance,
        all_rate_values_finite: true,
        peak_rss_bytes: peak_rss_bytes.or_else(process_peak_rss_bytes),
        helper_artifact_path: artifact_output.to_string_lossy().into_owned(),
        helper_artifact_sha256: artifact_sha256,
        rss_measurement: "macOS getrusage(RUSAGE_SELF).ru_maxrss の累積 high-water。RateGraph は一度だけ読み込み、全CNS応答は入力hashごとに一度だけ計算し、2つの読み出しが同じキャッシュを共有する。外部 --peak-rss-bytes があれば全体値として併記する。".to_owned(),
        inference_scope: "入力画像の復元、RetinaMap、無入力対照の一回計算、未キャッシュ画像の全CNS前向き計算、固定尺度の下流応答特徴抽出、キャッシュ済み特徴の2読み出し評価".to_owned(),
        learning_scope: "学習画面だけで平均・分散を計算して標準化した cached response features と4個の履歴特徴を使い、固定エポックのバッチ勾配法と学習率・L2 格子探索で正則化ロジスティック読み出しを学習する。RateBackward、回路パラメータ更新、配線シャッフル、重みシャッフルは主経路に含めない".to_owned(),
    };
    write_json(&output, &report)
}

fn build_cached_response_cache(
    graph: &RateGraph,
    parameters: &RateParameters,
    retina: &RetinaMap,
    context: &LearningEvaluationContext,
    entries: &[LearningScheduleEntry],
) -> Result<CachedResponseCacheBuild, Box<dyn Error>> {
    let zero_input = vec![0.0; graph.neuron_count()];
    let baseline_started = Instant::now();
    let (baseline_activity, _) = final_activity_with_parameters_profile(
        graph,
        parameters,
        &zero_input,
        context.criteria.rate_steps_per_observation,
        None,
    )?;
    let baseline_elapsed_ms = baseline_started.elapsed().as_secs_f64() * 1_000.0;
    if baseline_activity.iter().any(|value| !value.is_finite()) {
        return Err("the no-input baseline contains a non-finite rate".into());
    }
    let groups = build_response_groups(graph, &baseline_activity, &context.base.criteria.frozen)?;
    let mut cache = BTreeMap::new();
    let mut forward_times = Vec::new();
    let mut uncached_response_times = Vec::new();
    let mut cached_feature_lookup_times = Vec::new();
    let mut cache_hit_count = 0;
    for entry in entries {
        let cache_lookup_started = Instant::now();
        let cache_hit = cache.contains_key(&entry.image_sha256);
        let cache_lookup_ms = cache_lookup_started.elapsed().as_secs_f64() * 1_000.0;
        if cache_hit {
            cache_hit_count += 1;
            cached_feature_lookup_times.push(cache_lookup_ms);
            continue;
        }
        let uncached_started = Instant::now();
        let fixture = ScreenFixture::generate(
            entry.kind,
            entry.pair_index,
            &entry.side,
            context.base.fixtures.width,
            context.base.fixtures.height,
        )?;
        if fixture.sha256 != entry.image_sha256 {
            return Err(format!("cached fixture {} hash mismatch", entry.id).into());
        }
        let image = fixture.image()?;
        let neural_input = retina
            .encode(&image, graph.neuron_count())?
            .input
            .into_iter()
            .map(|value| value as f32)
            .collect::<Vec<_>>();
        let input = neural_input
            .iter()
            .map(|value| f64::from(*value))
            .collect::<Vec<_>>();
        let started = Instant::now();
        let (activity, _) = final_activity_with_parameters_profile(
            graph,
            parameters,
            &input,
            context.criteria.rate_steps_per_observation,
            None,
        )?;
        forward_times.push(started.elapsed().as_secs_f64() * 1_000.0);
        if activity.iter().any(|value| !value.is_finite()) {
            return Err(format!("cached response {} contains a non-finite rate", entry.id).into());
        }
        let response = cached_response_summary(
            &activity,
            &baseline_activity,
            &groups,
            parameters.hmax,
            context.base.criteria.frozen.zero_response_epsilon,
            context.criteria.response_projection_dimension,
        );
        if response.features.iter().any(|value| !value.is_finite()) {
            return Err(format!("cached response {} has a non-finite feature", entry.id).into());
        }
        cache.insert(
            entry.image_sha256.clone(),
            CachedStimulus {
                image,
                neural_input,
                response,
            },
        );
        uncached_response_times.push(uncached_started.elapsed().as_secs_f64() * 1_000.0);
    }
    let response_feature_dimension =
        groups.len() * (4 + context.criteria.response_projection_dimension);
    let cache_report = CachedResponseCacheReport {
        response_definition: "F_K(u(x);h0)-F_K(0;h0)".to_owned(),
        initial_state_definition: "h0=zeros; input and no-input use the same K updates".to_owned(),
        update_count: context.criteria.rate_steps_per_observation,
        baseline_forward_count: 1,
        unique_input_count: cache.len(),
        cache_hit_count,
        cache_miss_count: cache.len(),
        response_feature_dimension,
        response_projection_dimension: context.criteria.response_projection_dimension,
        response_groups: groups.iter().map(|group| group.report.clone()).collect(),
        response_component_names: vec![
            "mean_scaled_delta".to_owned(),
            "rms_scaled_delta".to_owned(),
            "absolute_mean_scaled_delta".to_owned(),
            "active_fraction".to_owned(),
        ],
        baseline_elapsed_ms,
        rate_forward_p50_ms: percentile(&forward_times, 0.50),
        rate_forward_p95_ms: percentile(&forward_times, 0.95),
        uncached_response_p50_ms: percentile(&uncached_response_times, 0.50),
        uncached_response_p95_ms: percentile(&uncached_response_times, 0.95),
        cached_feature_lookup_p50_ms: percentile(&cached_feature_lookup_times, 0.50),
        cached_feature_lookup_p95_ms: percentile(&cached_feature_lookup_times, 0.95),
        rate_forward_peak_rss_bytes: process_peak_rss_bytes(),
    };
    Ok((cache, cache_report, groups))
}

fn cached_response_summary(
    activity: &[f64],
    baseline: &[f64],
    groups: &[ResponseGroup],
    hmax: f64,
    zero_response_epsilon: f64,
    projection_dimension: usize,
) -> CachedResponse {
    let mut raw_values = Vec::new();
    let mut delta_values = Vec::new();
    let mut scaled_values = Vec::new();
    let mut features = Vec::with_capacity(groups.len() * (4 + projection_dimension));
    let mut components = Vec::with_capacity(groups.len());
    for group in groups {
        let scale = group.report.scale;
        let mut raw_squared = 0.0;
        let mut delta_squared = 0.0;
        let mut scaled_squared = 0.0;
        let mut scaled_sum = 0.0;
        let mut scaled_absolute_sum = 0.0;
        let mut active_count = 0;
        let mut saturation_count = 0;
        for index in &group.indices {
            let value = activity[*index as usize];
            let delta = value - baseline[*index as usize];
            let scaled = delta / scale;
            raw_values.push(value);
            delta_values.push(delta);
            scaled_values.push(scaled);
            raw_squared += value * value;
            delta_squared += delta * delta;
            scaled_squared += scaled * scaled;
            scaled_sum += scaled;
            scaled_absolute_sum += scaled.abs();
            active_count += usize::from(delta.abs() > zero_response_epsilon);
            saturation_count += usize::from(
                value <= zero_response_epsilon || value >= hmax - zero_response_epsilon,
            );
        }
        let count = group.indices.len().max(1) as f64;
        let scaled_rms = (scaled_squared / count).sqrt();
        features.extend([
            (scaled_sum / count) as f32,
            scaled_rms as f32,
            (scaled_absolute_sum / count) as f32,
            active_count as f32 / count as f32,
        ]);
        for bucket in 0..projection_dimension {
            let start = bucket * group.indices.len() / projection_dimension;
            let end = (bucket + 1) * group.indices.len() / projection_dimension;
            let bucket_sum = group.indices[start..end]
                .iter()
                .map(|index| (activity[*index as usize] - baseline[*index as usize]) / scale)
                .sum::<f64>();
            let bucket_count = end.saturating_sub(start).max(1) as f64;
            features.push((bucket_sum / bucket_count) as f32);
        }
        components.push(RateResponseComponentReport {
            name: group.report.name.clone(),
            raw_norm: raw_squared.sqrt(),
            delta_norm: delta_squared.sqrt(),
            scaled_delta_norm: scaled_squared.sqrt(),
            active_fraction: active_count as f64 / count,
            saturation_fraction: saturation_count as f64 / count,
            zero_response: scaled_rms <= zero_response_epsilon,
        });
    }
    CachedResponse {
        features,
        raw_norm: l2_norm(&raw_values),
        delta_norm: l2_norm(&delta_values),
        scaled_delta_norm: l2_norm(&scaled_values),
        active_fraction: delta_values
            .iter()
            .filter(|value| value.abs() > zero_response_epsilon)
            .count() as f64
            / delta_values.len().max(1) as f64,
        saturation_fraction: raw_values
            .iter()
            .filter(|value| {
                **value <= zero_response_epsilon || **value >= hmax - zero_response_epsilon
            })
            .count() as f64
            / raw_values.len().max(1) as f64,
        components,
    }
}

fn cached_three_point_audit(
    context: &LearningEvaluationContext,
    entries: &[LearningScheduleEntry],
    cache: &BTreeMap<String, CachedStimulus>,
    minimum_near_neural_l2: f64,
    minimum_unrelated_image_count: usize,
) -> Result<ThreePointAuditReport, Box<dyn Error>> {
    let lookup = |kind: ScreenFixtureKind, pair_index: usize, side: &str| {
        entries
            .iter()
            .find(|entry| {
                entry.kind == kind && entry.pair_index == pair_index && entry.side == side
            })
            .ok_or_else(|| format!("missing audit entry {kind:?}-{pair_index}-{side}"))
    };
    let mut pairs = Vec::new();
    let mut near_neural_zero_pair_count = 0;
    let mut near_downstream_zero_pair_count = 0;
    let mut all_images = BTreeSet::new();
    let mut unrelated_images = BTreeSet::new();
    for entry in entries {
        all_images.insert(entry.image_sha256.clone());
        if entry.kind == ScreenFixtureKind::Unrelated {
            unrelated_images.insert(entry.image_sha256.clone());
        }
    }
    for pair_index in 0..context.base.fixtures.pair_count {
        for (kind, name) in [
            (ScreenFixtureKind::Same, "same"),
            (ScreenFixtureKind::Near, "near"),
            (ScreenFixtureKind::Unrelated, "unrelated"),
        ] {
            let left = lookup(kind, pair_index, "a")?;
            let right = lookup(kind, pair_index, "b")?;
            let left = cache
                .get(&left.image_sha256)
                .ok_or_else(|| format!("missing cached left response for {name}-{pair_index}"))?;
            let right = cache
                .get(&right.image_sha256)
                .ok_or_else(|| format!("missing cached right response for {name}-{pair_index}"))?;
            let (image_l2, image_cosine, _) =
                image_feature_metrics(&left.image.pixels, &right.image.pixels);
            let (neural_l2, neural_cosine, _) =
                feature_metrics_f32(&left.neural_input, &right.neural_input);
            let (response_l2, response_cosine, _) =
                feature_metrics_f32(&left.response.features, &right.response.features);
            let neural_nonzero_difference_count = left
                .neural_input
                .iter()
                .zip(&right.neural_input)
                .filter(|(left, right)| (**left - **right).abs() > 1.0e-6)
                .count();
            let downstream_nonzero_difference_count = left
                .response
                .features
                .iter()
                .zip(&right.response.features)
                .filter(|(left, right)| (**left - **right).abs() > 1.0e-6)
                .count();
            if kind == ScreenFixtureKind::Near && neural_l2 < minimum_near_neural_l2 {
                near_neural_zero_pair_count += 1;
            }
            if kind == ScreenFixtureKind::Near
                && response_l2 <= context.base.criteria.frozen.zero_response_epsilon
            {
                near_downstream_zero_pair_count += 1;
            }
            pairs.push(ThreePointPairReport {
                id: format!("{name}-{pair_index:02}"),
                kind,
                image_l2_distance: image_l2,
                image_cosine_similarity: image_cosine,
                neural_input_l2_distance: neural_l2,
                neural_input_cosine_similarity: neural_cosine,
                neural_nonzero_difference_count,
                downstream_response_l2_distance: response_l2,
                downstream_response_cosine_similarity: response_cosine,
                downstream_nonzero_difference_count,
                left_raw_response_norm: left.response.raw_norm,
                right_raw_response_norm: right.response.raw_norm,
                left_delta_response_norm: left.response.delta_norm,
                right_delta_response_norm: right.response.delta_norm,
                left_response_norm: left.response.scaled_delta_norm,
                right_response_norm: right.response.scaled_delta_norm,
                left_active_fraction: left.response.active_fraction,
                right_active_fraction: right.response.active_fraction,
                left_saturation_fraction: left.response.saturation_fraction,
                right_saturation_fraction: right.response.saturation_fraction,
                left_zero_component_count: left
                    .response
                    .components
                    .iter()
                    .filter(|component| component.zero_response)
                    .count(),
                right_zero_component_count: right
                    .response
                    .components
                    .iter()
                    .filter(|component| component.zero_response)
                    .count(),
                left_response_zero: left.response.scaled_delta_norm
                    <= context.base.criteria.frozen.zero_response_epsilon,
                right_response_zero: right.response.scaled_delta_norm
                    <= context.base.criteria.frozen.zero_response_epsilon,
            });
        }
    }
    let same_pairs_ok = pairs
        .iter()
        .filter(|pair| pair.kind == ScreenFixtureKind::Same)
        .all(|pair| {
            pair.image_l2_distance <= context.base.criteria.audit.same_l2_max
                && pair.neural_input_l2_distance <= context.base.criteria.audit.same_l2_max
                && pair.downstream_response_l2_distance <= context.base.criteria.audit.same_l2_max
        });
    let near_pairs_ok = pairs
        .iter()
        .filter(|pair| pair.kind == ScreenFixtureKind::Near)
        .all(|pair| pair.neural_input_l2_distance >= minimum_near_neural_l2);
    let information_loss_counts = BTreeMap::from([
        (
            "near_image_to_neural_zero".to_owned(),
            near_neural_zero_pair_count,
        ),
        (
            "near_neural_to_downstream_zero".to_owned(),
            pairs
                .iter()
                .filter(|pair| {
                    pair.kind == ScreenFixtureKind::Near
                        && pair.neural_input_l2_distance >= minimum_near_neural_l2
                        && pair.downstream_response_l2_distance
                            <= context.base.criteria.frozen.zero_response_epsilon
                })
                .count(),
        ),
        (
            "near_image_to_downstream_zero".to_owned(),
            near_downstream_zero_pair_count,
        ),
    ]);
    let mut feature_separation = FeatureSeparationReport::from_pairs(&pairs);
    feature_separation.set_passed(
        context.criteria.feature_separation.minimum_pair_l2,
        context
            .criteria
            .feature_separation
            .minimum_unrelated_to_near_p50_ratio,
    );
    let response_feature_dimension = cache
        .values()
        .next()
        .map(|stimulus| stimulus.response.features.len())
        .unwrap_or(0);
    let passed = same_pairs_ok
        && near_pairs_ok
        && near_downstream_zero_pair_count == 0
        && feature_separation.passed
        && unrelated_images.len() >= minimum_unrelated_image_count;
    Ok(ThreePointAuditReport {
        pair_count: pairs.len(),
        unique_image_count: all_images.len(),
        unrelated_unique_image_count: unrelated_images.len(),
        near_neural_zero_pair_count,
        near_downstream_zero_pair_count,
        information_loss_counts,
        response_feature_dimension,
        feature_separation,
        learning_feature_dimension: None,
        learning_feature_separation: None,
        passed,
        pairs,
    })
}

impl FeatureSeparationReport {
    fn from_pairs(pairs: &[ThreePointPairReport]) -> Self {
        let summary = |kind| {
            let selected = pairs
                .iter()
                .filter(|pair| pair.kind == kind)
                .collect::<Vec<_>>();
            let l2 = selected
                .iter()
                .map(|pair| pair.downstream_response_l2_distance)
                .collect::<Vec<_>>();
            let cosine = selected
                .iter()
                .map(|pair| pair.downstream_response_cosine_similarity)
                .collect::<Vec<_>>();
            (
                l2.len(),
                percentile(&l2, 0.0),
                percentile(&l2, 0.5),
                percentile(&l2, 1.0),
                percentile(&cosine, 0.0),
                percentile(&cosine, 0.5),
                percentile(&cosine, 1.0),
            )
        };
        let (
            near_pair_count,
            near_l2_min,
            near_l2_p50,
            near_l2_max,
            near_cosine_min,
            near_cosine_p50,
            near_cosine_max,
        ) = summary(ScreenFixtureKind::Near);
        let (
            unrelated_pair_count,
            unrelated_l2_min,
            unrelated_l2_p50,
            unrelated_l2_max,
            unrelated_cosine_min,
            unrelated_cosine_p50,
            unrelated_cosine_max,
        ) = summary(ScreenFixtureKind::Unrelated);
        let mut report = Self {
            near_pair_count,
            near_l2_min,
            near_l2_p50,
            near_l2_max,
            near_cosine_min,
            near_cosine_p50,
            near_cosine_max,
            unrelated_pair_count,
            unrelated_l2_min,
            unrelated_l2_p50,
            unrelated_l2_max,
            unrelated_cosine_min,
            unrelated_cosine_p50,
            unrelated_cosine_max,
            unrelated_to_near_p50_l2_ratio: unrelated_l2_p50 / near_l2_p50.max(f64::MIN_POSITIVE),
            passed: false,
        };
        report.set_passed(0.0, 0.0);
        report
    }

    fn from_feature_pairs(pairs: &[(ScreenFixtureKind, f64, f64)]) -> Self {
        let selected = |kind| {
            let values = pairs
                .iter()
                .filter(|(pair_kind, _, _)| *pair_kind == kind)
                .collect::<Vec<_>>();
            let l2 = values
                .iter()
                .map(|(_, value, _)| *value)
                .collect::<Vec<_>>();
            let cosine = values
                .iter()
                .map(|(_, _, value)| *value)
                .collect::<Vec<_>>();
            (
                l2.len(),
                percentile(&l2, 0.0),
                percentile(&l2, 0.5),
                percentile(&l2, 1.0),
                percentile(&cosine, 0.0),
                percentile(&cosine, 0.5),
                percentile(&cosine, 1.0),
            )
        };
        let (
            near_pair_count,
            near_l2_min,
            near_l2_p50,
            near_l2_max,
            near_cosine_min,
            near_cosine_p50,
            near_cosine_max,
        ) = selected(ScreenFixtureKind::Near);
        let (
            unrelated_pair_count,
            unrelated_l2_min,
            unrelated_l2_p50,
            unrelated_l2_max,
            unrelated_cosine_min,
            unrelated_cosine_p50,
            unrelated_cosine_max,
        ) = selected(ScreenFixtureKind::Unrelated);
        let mut report = Self {
            near_pair_count,
            near_l2_min,
            near_l2_p50,
            near_l2_max,
            near_cosine_min,
            near_cosine_p50,
            near_cosine_max,
            unrelated_pair_count,
            unrelated_l2_min,
            unrelated_l2_p50,
            unrelated_l2_max,
            unrelated_cosine_min,
            unrelated_cosine_p50,
            unrelated_cosine_max,
            unrelated_to_near_p50_l2_ratio: unrelated_l2_p50 / near_l2_p50.max(f64::MIN_POSITIVE),
            passed: false,
        };
        report.set_passed(0.0, 0.0);
        report
    }

    fn set_passed(&mut self, minimum_pair_l2: f64, minimum_ratio: f64) {
        self.passed = self.near_pair_count > 0
            && self.unrelated_pair_count > 0
            && self.near_l2_min >= minimum_pair_l2
            && self.unrelated_l2_min >= minimum_pair_l2
            && self.unrelated_to_near_p50_l2_ratio >= minimum_ratio;
    }
}

fn feature_metrics_f32(left: &[f32], right: &[f32]) -> (f64, f64, f64) {
    let left = left
        .iter()
        .map(|value| f64::from(*value))
        .collect::<Vec<_>>();
    let right = right
        .iter()
        .map(|value| f64::from(*value))
        .collect::<Vec<_>>();
    feature_metrics(&left, &right)
}

fn cached_feature_for_entry<'a>(
    entry: &LearningScheduleEntry,
    cache: &'a BTreeMap<String, CachedStimulus>,
) -> Result<&'a [f32], Box<dyn Error>> {
    cache
        .get(&entry.image_sha256)
        .map(|stimulus| stimulus.response.features.as_slice())
        .ok_or_else(|| format!("cached response is missing for {}", entry.id).into())
}

fn cached_brain_input(
    entry: &LearningScheduleEntry,
    features: Vec<f32>,
    stream_id: &str,
    position: u64,
) -> habitua::BrainInput {
    cached_brain_input_at_position(entry, features, stream_id, position, position)
}

fn cached_brain_input_at_position(
    entry: &LearningScheduleEntry,
    features: Vec<f32>,
    stream_id: &str,
    timestamp_ms: u64,
    processing_position: u64,
) -> habitua::BrainInput {
    habitua::BrainInput {
        // The stable primary key for case memory is the exact image hash.
        // `entry.id` remains the human-readable schedule identifier.
        id: entry.image_sha256.clone(),
        stream_id: stream_id.to_owned(),
        target: "screen".to_owned(),
        context: "malecns-frozen-response".to_owned(),
        schema_id: "malecns-frozen-response".to_owned(),
        schema_version: 2,
        features,
        failure: None,
        available_at: Timestamp::from_millis(timestamp_ms),
        processed_at: None,
        position: processing_position,
    }
}

fn build_batch_samples(
    entries: &[LearningScheduleEntry],
    cache: &BTreeMap<String, CachedStimulus>,
    criteria: &LearningEvaluationCriteria,
) -> Result<Vec<BatchSample>, Box<dyn Error>> {
    let mut history = Vec::<FrozenReadoutHistoryEntry>::new();
    let mut samples = Vec::with_capacity(entries.len());
    for entry in entries {
        let base_features = cached_feature_for_entry(entry, cache)?.to_vec();
        let timestamp_s = entry.timestamp_s;
        let (history_features, augmented_features) = frozen_readout_augmented_features_with_expiry(
            &base_features,
            &history,
            timestamp_s,
            criteria.recent_observations,
            criteria.distance_threshold as f32,
            criteria.previous_absolute_difference_scale as f32,
            criteria.history_elapsed_time_scale_seconds as f32,
            criteria.behavior.history_max_age_seconds as f32,
        );
        let distance = history_features.distance;
        let previous_difference = history_features.previous_absolute_difference;
        let automatic_change = distance.map(|value| value > criteria.distance_threshold as f32);
        samples.push(BatchSample {
            entry: entry.clone(),
            augmented_features: augmented_features.into_iter().map(f64::from).collect(),
            distance: distance.map(f64::from),
            previous_absolute_difference: previous_difference.map(f64::from),
            automatic_change,
        });
        history.push(FrozenReadoutHistoryEntry {
            features: base_features,
            timestamp_s,
        });
        while history.len() > criteria.recent_observations {
            history.remove(0);
        }
    }
    Ok(samples)
}

fn learning_feature_separation(
    groups: &[&[BatchSample]],
    pair_count: usize,
    criteria: &LearningFeatureSeparationCriteria,
) -> Result<FeatureSeparationReport, Box<dyn Error>> {
    let samples = groups
        .iter()
        .flat_map(|group| group.iter())
        .collect::<Vec<_>>();
    let mut pairs = Vec::new();
    for pair_index in 0..pair_count {
        for kind in [
            ScreenFixtureKind::Same,
            ScreenFixtureKind::Near,
            ScreenFixtureKind::Unrelated,
        ] {
            let left = samples
                .iter()
                .find(|sample| {
                    sample.entry.pair_index == pair_index
                        && sample.entry.kind == kind
                        && sample.entry.side == "a"
                })
                .ok_or_else(|| {
                    format!("missing learning feature left pair {kind:?}-{pair_index}")
                })?;
            let right = samples
                .iter()
                .find(|sample| {
                    sample.entry.pair_index == pair_index
                        && sample.entry.kind == kind
                        && sample.entry.side == "b"
                })
                .ok_or_else(|| {
                    format!("missing learning feature right pair {kind:?}-{pair_index}")
                })?;
            let (l2, cosine, _) =
                feature_metrics(&left.augmented_features, &right.augmented_features);
            pairs.push((kind, l2, cosine));
        }
    }
    let mut report = FeatureSeparationReport::from_feature_pairs(&pairs);
    report.set_passed(
        criteria.minimum_pair_l2,
        criteria.minimum_unrelated_to_near_p50_ratio,
    );
    Ok(report)
}

fn fit_feature_standardization(
    samples: &[BatchSample],
) -> Result<FeatureStandardization, Box<dyn Error>> {
    let eligible = samples
        .iter()
        .filter(|sample| sample.automatic_change.is_some())
        .collect::<Vec<_>>();
    let dimension = eligible
        .first()
        .ok_or("batch training samples are empty")?
        .augmented_features
        .len();
    let mut mean = vec![0.0; dimension];
    for sample in &eligible {
        if sample.augmented_features.len() != dimension {
            return Err("batch training feature dimensions differ".into());
        }
        for (slot, value) in mean.iter_mut().zip(&sample.augmented_features) {
            *slot += *value;
        }
    }
    let sample_count = eligible.len() as f64;
    for value in &mut mean {
        *value /= sample_count;
    }
    let mut variance = vec![0.0; dimension];
    for sample in &eligible {
        for ((slot, value), average) in variance
            .iter_mut()
            .zip(&sample.augmented_features)
            .zip(&mean)
        {
            let difference = *value - *average;
            *slot += difference * difference;
        }
    }
    let mut zero_variance_count = 0;
    let scale = variance
        .into_iter()
        .map(|value| {
            let standard_deviation = (value / sample_count).sqrt();
            if standard_deviation.is_finite() && standard_deviation > 1.0e-12 {
                standard_deviation
            } else {
                zero_variance_count += 1;
                1.0
            }
        })
        .collect();
    Ok(FeatureStandardization {
        mean,
        scale,
        zero_variance_count,
    })
}

fn standardize_batch_features(
    samples: &[BatchSample],
    standardization: &FeatureStandardization,
) -> Result<Vec<Vec<f64>>, Box<dyn Error>> {
    samples
        .iter()
        .map(|sample| {
            if sample.augmented_features.len() != standardization.mean.len()
                || sample.augmented_features.len() != standardization.scale.len()
            {
                return Err("batch feature dimension does not match standardization".into());
            }
            Ok(sample
                .augmented_features
                .iter()
                .zip(&standardization.mean)
                .zip(&standardization.scale)
                .map(|((value, mean), scale)| (*value - *mean) / *scale)
                .collect::<Vec<_>>())
        })
        .collect()
}

fn batch_target(sample: &BatchSample, role: &str) -> Option<bool> {
    sample
        .automatic_change
        .map(|changed| if role == "change" { changed } else { !changed })
}

fn shuffled_targets(targets: &[bool], mut seed: u64) -> Vec<bool> {
    let mut indices = (0..targets.len()).collect::<Vec<_>>();
    for index in (1..indices.len()).rev() {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let selected = (seed as usize) % (index + 1);
        indices.swap(index, selected);
    }
    indices.into_iter().map(|index| targets[index]).collect()
}

fn fit_batch_parameters(
    features: &[Vec<f64>],
    targets: &[bool],
    learning_rate: f64,
    l2: f64,
    epochs: usize,
) -> (Vec<f64>, f64) {
    let dimension = features.first().map_or(0, Vec::len);
    let mut weights = vec![0.0; dimension];
    let mut bias = 0.0;
    let inverse_count = 1.0 / features.len().max(1) as f64;
    for _ in 0..epochs {
        let mut weight_gradient = vec![0.0; dimension];
        let mut bias_gradient = 0.0;
        for (features, target) in features.iter().zip(targets) {
            let logit = bias
                + weights
                    .iter()
                    .zip(features)
                    .map(|(weight, feature)| weight * feature)
                    .sum::<f64>();
            let error = sigmoid_f64(logit) - f64::from(u8::from(*target));
            bias_gradient += error;
            for (gradient, feature) in weight_gradient.iter_mut().zip(features) {
                *gradient += error * *feature;
            }
        }
        for (weight, gradient) in weights.iter_mut().zip(weight_gradient) {
            *weight -= learning_rate * (gradient * inverse_count + l2 * *weight);
        }
        bias -= learning_rate * bias_gradient * inverse_count;
    }
    (weights, bias)
}

fn sigmoid_f64(value: f64) -> f64 {
    if value >= 0.0 {
        let exponent = (-value).exp();
        1.0 / (1.0 + exponent)
    } else {
        let exponent = value.exp();
        exponent / (1.0 + exponent)
    }
}

fn predict_batch_parameters(weights: &[f64], bias: f64, features: &[f64]) -> f64 {
    sigmoid_f64(
        bias + weights
            .iter()
            .zip(features)
            .map(|(weight, feature)| weight * feature)
            .sum::<f64>(),
    )
}

fn batch_log_loss(scores: &[f64], labels: &[bool]) -> f64 {
    let total = scores
        .iter()
        .zip(labels)
        .map(|(score, label)| {
            let score = score.clamp(1.0e-12, 1.0 - 1.0e-12);
            if *label {
                -score.ln()
            } else {
                -(1.0 - score).ln()
            }
        })
        .sum::<f64>();
    total / scores.len().max(1) as f64
}

fn fit_batch_readout(
    role: &str,
    training_samples: &[BatchSample],
    tuning_samples: &[BatchSample],
    standardization: &FeatureStandardization,
    criteria: &LearningEvaluationCriteria,
    shuffle_seed: Option<u64>,
) -> Result<BatchReadoutFit, Box<dyn Error>> {
    let eligible_training_samples = training_samples
        .iter()
        .filter(|sample| sample.automatic_change.is_some())
        .cloned()
        .collect::<Vec<_>>();
    let eligible_tuning_samples = tuning_samples
        .iter()
        .filter(|sample| sample.automatic_change.is_some())
        .cloned()
        .collect::<Vec<_>>();
    let training_features =
        standardize_batch_features(&eligible_training_samples, standardization)?;
    let tuning_features = standardize_batch_features(&eligible_tuning_samples, standardization)?;
    let canonical_training_targets = eligible_training_samples
        .iter()
        .map(|sample| batch_target(sample, role))
        .collect::<Option<Vec<_>>>()
        .ok_or("eligible batch training sample has no automatic label")?;
    let tuning_targets = eligible_tuning_samples
        .iter()
        .map(|sample| batch_target(sample, role))
        .collect::<Option<Vec<_>>>()
        .ok_or("eligible batch tuning sample has no automatic label")?;
    let training_targets = shuffle_seed
        .map(|seed| shuffled_targets(&canonical_training_targets, seed))
        .unwrap_or_else(|| canonical_training_targets.clone());
    let mut candidates: Vec<BatchCandidateReport> = Vec::new();
    let mut selected_index: Option<usize> = None;
    let mut selected: Option<SelectedBatchReadout> = None;
    for learning_rate in &criteria.batch_learning_rates {
        for l2 in &criteria.batch_l2_values {
            let (weights, bias) = fit_batch_parameters(
                &training_features,
                &training_targets,
                *learning_rate,
                *l2,
                criteria.batch_epochs,
            );
            let tuning_scores = tuning_features
                .iter()
                .map(|features| predict_batch_parameters(&weights, bias, features))
                .collect::<Vec<_>>();
            let tuning_balanced_accuracy = balanced_accuracy_at_threshold(
                &tuning_scores,
                &tuning_targets,
                criteria.readout_reaction_threshold,
            );
            let tuning_auroc = auroc(&tuning_scores, &tuning_targets);
            let tuning_log_loss = batch_log_loss(&tuning_scores, &tuning_targets);
            let candidate_score = (tuning_balanced_accuracy, tuning_auroc, -tuning_log_loss);
            let is_selected = selected
                .as_ref()
                .is_none_or(|(_, best_score, _)| candidate_score > *best_score);
            if is_selected {
                if let Some(index) = selected_index {
                    candidates[index].selected = false;
                }
                selected_index = Some(candidates.len());
                selected = Some((
                    FittedBatchReadout {
                        learning_rate: *learning_rate,
                        l2: *l2,
                        weights: weights.clone(),
                        bias,
                    },
                    candidate_score,
                    tuning_scores.clone(),
                ));
            }
            candidates.push(BatchCandidateReport {
                role: role.to_owned(),
                learning_rate: *learning_rate,
                l2: *l2,
                tuning_balanced_accuracy,
                tuning_auroc,
                tuning_log_loss,
                selected: false,
            });
            if is_selected {
                candidates
                    .last_mut()
                    .expect("candidate was just appended")
                    .selected = true;
            }
        }
    }
    let (fitted, _, tuning_scores) = selected.ok_or("batch hyperparameter grid is empty")?;
    Ok((fitted, candidates, tuning_scores))
}

fn select_decision_threshold(
    tuning_samples: &[BatchSample],
    tuning_scores: &BTreeMap<String, Vec<f64>>,
    fallback: f64,
) -> f64 {
    let eligible_samples = tuning_samples
        .iter()
        .filter(|sample| sample.automatic_change.is_some())
        .collect::<Vec<_>>();
    if eligible_samples.is_empty() {
        return fallback;
    }
    let mut candidates = vec![0.0, 1.0, fallback.clamp(0.0, 1.0)];
    for scores in tuning_scores.values() {
        candidates.extend(scores.iter().copied().filter(|score| score.is_finite()));
    }
    candidates.sort_by(f64::total_cmp);
    candidates.dedup_by(|left, right| (*left - *right).abs() <= 1.0e-12);
    candidates.extend(
        candidates
            .windows(2)
            .filter_map(|window| {
                let midpoint = (window[0] + window[1]) * 0.5;
                (midpoint > window[0] && midpoint < window[1]).then_some(midpoint)
            })
            .collect::<Vec<_>>(),
    );
    candidates.sort_by(f64::total_cmp);
    candidates.dedup_by(|left, right| (*left - *right).abs() <= 1.0e-12);
    let labels_by_role = ["change", "no_change"];
    candidates
        .into_iter()
        .max_by(|left, right| {
            let left_score =
                threshold_selection_score(left, &eligible_samples, tuning_scores, &labels_by_role);
            let right_score =
                threshold_selection_score(right, &eligible_samples, tuning_scores, &labels_by_role);
            left_score
                .partial_cmp(&right_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    let left_distance = (left - fallback).abs();
                    let right_distance = (right - fallback).abs();
                    right_distance
                        .partial_cmp(&left_distance)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        })
        .unwrap_or(fallback)
}

fn threshold_selection_score(
    threshold: &f64,
    samples: &[&BatchSample],
    scores_by_role: &BTreeMap<String, Vec<f64>>,
    roles: &[&str],
) -> (f64, f64, f64) {
    let mut balanced_accuracies = Vec::new();
    let mut youdens = Vec::new();
    let mut scores_present = false;
    for role in roles {
        let Some(scores) = scores_by_role.get(*role) else {
            continue;
        };
        let labels = samples
            .iter()
            .map(|sample| {
                if *role == "change" {
                    sample.automatic_change.unwrap_or(false)
                } else {
                    !sample.automatic_change.unwrap_or(false)
                }
            })
            .collect::<Vec<_>>();
        if scores.len() != labels.len() {
            continue;
        }
        scores_present = true;
        balanced_accuracies.push(balanced_accuracy_at_threshold(scores, &labels, *threshold));
        let (sensitivity, specificity) = sensitivity_specificity(scores, &labels, *threshold);
        youdens.push(sensitivity + specificity - 1.0);
    }
    if !scores_present {
        return (0.0, 0.0, 0.0);
    }
    (
        balanced_accuracies.iter().sum::<f64>() / balanced_accuracies.len() as f64,
        youdens.iter().sum::<f64>() / youdens.len() as f64,
        0.0,
    )
}

fn prediction_distributions_from_scores(
    scores: &[BrainScores],
    criteria: &LearningEvaluationCriteria,
) -> Vec<PredictionDistributionReport> {
    criteria
        .brain_roles
        .iter()
        .map(|brain| {
            let values = scores
                .iter()
                .map(|score| {
                    if brain.role == "change" {
                        score.brain_change_score
                    } else {
                        score.brain_no_change_score
                    }
                })
                .collect::<Vec<_>>();
            let mut distinct = values.clone();
            distinct.sort_by(f64::total_cmp);
            distinct.dedup_by(|left, right| (*left - *right).abs() <= 1.0e-12);
            let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
            let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            PredictionDistributionReport {
                role: brain.role.clone(),
                sample_count: values.len(),
                minimum,
                p10: percentile(&values, 0.10),
                median: percentile(&values, 0.50),
                p90: percentile(&values, 0.90),
                maximum,
                distinct_value_count: distinct.len(),
                constant_prediction: (maximum - minimum).abs() <= 1.0e-9,
            }
        })
        .collect()
}

fn standardization_report(standardization: &FeatureStandardization) -> StandardizationReport {
    StandardizationReport {
        dimension: standardization.mean.len(),
        zero_variance_count: standardization.zero_variance_count,
        mean_min: standardization
            .mean
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min),
        mean_max: standardization
            .mean
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max),
        scale_min: standardization
            .scale
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min),
        scale_max: standardization
            .scale
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max),
        mean_sha256: sha256_bytes(&serde_json::to_vec(&standardization.mean).unwrap_or_default()),
        scale_sha256: sha256_bytes(&serde_json::to_vec(&standardization.scale).unwrap_or_default()),
    }
}

fn teacher_rule_report(
    groups: &[&[BatchSample]],
    threshold: f64,
    _response_feature_dimension: usize,
) -> TeacherRuleReport {
    let mut rule_matches = [0_usize; 4];
    let mut sample_counts = [0_usize; 4];
    let mut fixture_matches = [0_usize; 4];
    let mut baseline_insufficient = [0_usize; 4];
    for (group_index, samples) in groups.iter().enumerate() {
        for sample in *samples {
            if let Some(automatic_change) = sample.automatic_change {
                let derived = sample.distance.is_some_and(|value| value > threshold);
                rule_matches[group_index] += usize::from(derived == automatic_change);
                fixture_matches[group_index] +=
                    usize::from(automatic_change == sample.entry.expected_change);
            } else {
                baseline_insufficient[group_index] += 1;
            }
            sample_counts[group_index] += 1;
        }
    }
    let match_rate = |matches: usize, samples: usize, insufficient: usize| {
        matches as f64 / samples.saturating_sub(insufficient).max(1) as f64
    };
    TeacherRuleReport {
        threshold,
        training_match_count: rule_matches[0],
        training_sample_count: sample_counts[0],
        tuning_match_count: rule_matches[1],
        tuning_sample_count: sample_counts[1],
        evaluation_match_count: rule_matches[2],
        evaluation_sample_count: sample_counts[2],
        held_out_match_count: rule_matches[3],
        held_out_sample_count: sample_counts[3],
        training_fixture_label_match_count: fixture_matches[0],
        tuning_fixture_label_match_count: fixture_matches[1],
        evaluation_fixture_label_match_count: fixture_matches[2],
        held_out_fixture_label_match_count: fixture_matches[3],
        training_baseline_insufficient_count: baseline_insufficient[0],
        tuning_baseline_insufficient_count: baseline_insufficient[1],
        evaluation_baseline_insufficient_count: baseline_insufficient[2],
        held_out_baseline_insufficient_count: baseline_insufficient[3],
        training_automatic_expected_label_match_rate: match_rate(
            fixture_matches[0],
            sample_counts[0],
            baseline_insufficient[0],
        ),
        tuning_automatic_expected_label_match_rate: match_rate(
            fixture_matches[1],
            sample_counts[1],
            baseline_insufficient[1],
        ),
        evaluation_automatic_expected_label_match_rate: match_rate(
            fixture_matches[2],
            sample_counts[2],
            baseline_insufficient[2],
        ),
        held_out_automatic_expected_label_match_rate: match_rate(
            fixture_matches[3],
            sample_counts[3],
            baseline_insufficient[3],
        ),
        all_match: rule_matches
            .iter()
            .zip(sample_counts.iter().zip(&baseline_insufficient))
            .all(|(matches, (count, insufficient))| *matches + *insufficient == *count),
    }
}

fn learning_presentation_reports(
    groups: &[(&str, &[BatchSample])],
    criteria: &LearningEvaluationCriteria,
) -> Vec<LearningPresentationReport> {
    groups
        .iter()
        .flat_map(|(split, samples)| {
            samples.iter().map(|sample| {
                let mut selected_feedback = BTreeMap::new();
                for role in ["change", "no_change"] {
                    let feedback = criteria
                        .human_feedback
                        .iter()
                        .find(|event| event.input_id == sample.entry.id)
                        .map(|event| event.kind.as_str().to_owned())
                        .or_else(|| {
                            sample.automatic_change.map(|changed| {
                                let reward = if role == "change" { changed } else { !changed };
                                if reward { "reward" } else { "punish" }.to_owned()
                            })
                        })
                        .unwrap_or_else(|| "none".to_owned());
                    selected_feedback.insert(role.to_owned(), feedback);
                }
                LearningPresentationReport {
                    split: (*split).to_owned(),
                    id: sample.entry.id.clone(),
                    pair_index: sample.entry.pair_index,
                    side: sample.entry.side.clone(),
                    distance: sample.distance,
                    distance_over_threshold: sample
                        .distance
                        .map(|value| value / criteria.distance_threshold),
                    previous_absolute_difference: sample.previous_absolute_difference,
                    previous_absolute_difference_over_scale: sample
                        .previous_absolute_difference
                        .map(|value| value / criteria.previous_absolute_difference_scale),
                    automatic_change: sample.automatic_change,
                    expected_change: sample.entry.expected_change,
                    automatic_expected_label_match: sample
                        .automatic_change
                        .map(|value| value == sample.entry.expected_change),
                    readiness: if sample.automatic_change.is_some() {
                        "evaluated".to_owned()
                    } else {
                        "baseline_insufficient".to_owned()
                    },
                    selected_feedback,
                }
            })
        })
        .collect()
}

struct CachedReadoutRunnerConfig<'a> {
    feature_dimension: usize,
    criteria: &'a LearningEvaluationCriteria,
    training_mode: TrainingMode,
    feedback: FrozenReadoutFeedback,
    seed_base: u64,
    standardization: &'a FeatureStandardization,
    reaction_threshold: f64,
    fitted: Option<&'a BTreeMap<String, FittedBatchReadout>>,
}

fn build_cached_readout_runner(
    config: CachedReadoutRunnerConfig<'_>,
) -> Result<BrainRunner, Box<dyn Error>> {
    let CachedReadoutRunnerConfig {
        feature_dimension,
        criteria,
        training_mode,
        feedback,
        seed_base,
        standardization,
        reaction_threshold,
        fitted,
    } = config;
    let mut registry = ModelRegistry::empty();
    register_frozen_readout_model(&mut registry, feature_dimension, seed_base, feedback)?;
    let configs = criteria
        .brain_roles
        .iter()
        .map(|brain| {
            let mut settings = BTreeMap::new();
            settings.insert("role".to_owned(), brain.role.clone());
            settings.insert(
                "feature_dimension".to_owned(),
                feature_dimension.to_string(),
            );
            settings.insert(
                "recent_observations".to_owned(),
                criteria.recent_observations.to_string(),
            );
            settings.insert(
                "distance_threshold".to_owned(),
                criteria.distance_threshold.to_string(),
            );
            settings.insert(
                "previous_absolute_difference_scale".to_owned(),
                criteria.previous_absolute_difference_scale.to_string(),
            );
            settings.insert(
                "history_elapsed_time_scale_seconds".to_owned(),
                criteria.history_elapsed_time_scale_seconds.to_string(),
            );
            settings.insert(
                "history_max_age_seconds".to_owned(),
                criteria.behavior.history_max_age_seconds.to_string(),
            );
            settings.insert(
                "l2_regularization".to_owned(),
                criteria.readout_l2_regularization.to_string(),
            );
            settings.insert(
                "learning_rate".to_owned(),
                criteria.readout_learning_rate.to_string(),
            );
            settings.insert(
                "reaction_threshold".to_owned(),
                reaction_threshold.to_string(),
            );
            settings.insert(
                "case_memory_distance_threshold".to_owned(),
                criteria.case_memory.distance_threshold.to_string(),
            );
            settings.insert(
                "case_memory_time_constant_seconds".to_owned(),
                criteria.case_memory.time_constant_seconds.to_string(),
            );
            settings.insert(
                "case_memory_logit_scale".to_owned(),
                criteria.case_memory.logit_scale.to_string(),
            );
            settings.insert(
                "case_memory_max_cases".to_owned(),
                criteria.case_memory.max_cases.to_string(),
            );
            settings.insert(
                "standardization_mean".to_owned(),
                format_float_vector(&standardization.mean),
            );
            settings.insert(
                "standardization_scale".to_owned(),
                format_float_vector(&standardization.scale),
            );
            if let Some(fitted) = fitted.and_then(|models| models.get(&brain.role)) {
                settings.insert(
                    "initial_weights".to_owned(),
                    format_float_vector(&fitted.weights),
                );
                settings.insert("initial_bias".to_owned(), fitted.bias.to_string());
            }
            settings.insert(
                "training_mode".to_owned(),
                training_mode_name(training_mode).to_owned(),
            );
            settings.insert("seed".to_owned(), brain.seed.to_string());
            let reaction_kinds = if brain.role == "change" {
                vec![habitua::ReactionKind::Novel]
            } else {
                vec![habitua::ReactionKind::Repeat]
            };
            BrainConfig {
                id: brain.id.clone(),
                role: brain.role.clone(),
                enabled: true,
                mode: BrainMode::Active,
                input: BrainInputSpec {
                    schema_id: "malecns-frozen-response".to_owned(),
                    schema_version: 2,
                    target: "screen".to_owned(),
                    context: "malecns-frozen-response".to_owned(),
                },
                model: ModelSpec {
                    kind: "frozen_readout".to_owned(),
                    version: 1,
                    seed: Some(brain.seed),
                    settings: ModelSettings::Custom(settings),
                },
                learning: habitua::BrainLearningPolicy {
                    mode: LearningMode::Manual,
                    weight: 1.0,
                    require_evaluated: false,
                    bootstrap: false,
                    learn_on_reaction: false,
                    frequency_limit: None,
                },
                reactions: ReactionPolicy {
                    threshold: reaction_threshold as f32,
                    kinds: reaction_kinds,
                    calibration: Some("frozen-readout-probability".to_owned()),
                },
                resources: ResourcePolicy {
                    max_streams: 8,
                    max_samples: criteria.presentation_count,
                    max_memory_bytes: 4_000_000,
                    evaluation_deadline: None,
                },
                state: StatePolicy {
                    format_version: 1,
                    configuration_identity: format!(
                        "malecns-frozen-response:{}:{}:{}",
                        criteria.readout_feature_definition,
                        criteria.readout_l2_regularization.to_bits(),
                        training_mode_name(training_mode)
                    ),
                    deletion: DeletionPolicy::Delete,
                },
            }
        })
        .collect::<Vec<_>>();
    let consensus_policy = ConsensusPolicy::new(
        "change",
        "no_change",
        "target_probability",
        reaction_threshold as f32,
        criteria.acceptance.consensus_margin as f32,
        ReactionKind::Novel,
        ReactionKind::Repeat,
    )?;
    Ok(BrainRunner::new_with_consensus_policy(
        configs,
        &registry,
        consensus_policy,
    )?)
}

fn format_float_vector(values: &[f64]) -> String {
    values
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

struct CachedReadoutCondition<'a> {
    name: &'a str,
    cache: &'a BTreeMap<String, CachedStimulus>,
    feature_dimension: usize,
    training_entries: &'a [LearningScheduleEntry],
    evaluation_entries: &'a [LearningScheduleEntry],
    held_out_entries: &'a [LearningScheduleEntry],
    training_samples: &'a [BatchSample],
    tuning_samples: &'a [BatchSample],
    evaluation_samples: &'a [BatchSample],
    held_out_samples: &'a [BatchSample],
    standardization: &'a FeatureStandardization,
    criteria: &'a LearningEvaluationCriteria,
    training_mode: TrainingMode,
    feedback: FrozenReadoutFeedback,
    seed_base: u64,
    shuffle_seed: Option<u64>,
    decision_threshold: Option<f64>,
    behavior: Option<&'a LearningBehaviorCriteria>,
}

struct CachedBehaviorBranches {
    no_food: BrainRunner,
    food: BrainRunner,
    reverse_food: BrainRunner,
    rest_probes: Vec<BrainRunner>,
    food_feedback: FrozenReadoutFeedback,
    reverse_food_feedback: FrozenReadoutFeedback,
}

#[allow(clippy::too_many_arguments)]
fn build_cached_behavior_branches(
    feature_dimension: usize,
    criteria: &LearningEvaluationCriteria,
    training_mode: TrainingMode,
    standardization: &FeatureStandardization,
    fitted: Option<&BTreeMap<String, FittedBatchReadout>>,
    feedback: &FrozenReadoutFeedback,
    seed_base: u64,
    decision_threshold: f64,
) -> Result<CachedBehaviorBranches, Box<dyn Error>> {
    let no_food_feedback = feedback.independent_clone();
    let food_feedback = feedback.independent_clone();
    let reverse_food_feedback = feedback.independent_clone();
    let no_food = build_cached_readout_runner(CachedReadoutRunnerConfig {
        feature_dimension,
        criteria,
        training_mode,
        feedback: no_food_feedback,
        seed_base,
        standardization,
        reaction_threshold: decision_threshold,
        fitted,
    })?;
    let food = build_cached_readout_runner(CachedReadoutRunnerConfig {
        feature_dimension,
        criteria,
        training_mode,
        feedback: food_feedback.clone(),
        seed_base,
        standardization,
        reaction_threshold: decision_threshold,
        fitted,
    })?;
    let reverse_food = build_cached_readout_runner(CachedReadoutRunnerConfig {
        feature_dimension,
        criteria,
        training_mode,
        feedback: reverse_food_feedback.clone(),
        seed_base,
        standardization,
        reaction_threshold: decision_threshold,
        fitted,
    })?;
    let rest_probes = (0..4)
        .map(|_| {
            build_cached_readout_runner(CachedReadoutRunnerConfig {
                feature_dimension,
                criteria,
                training_mode,
                feedback: feedback.independent_clone(),
                seed_base,
                standardization,
                reaction_threshold: decision_threshold,
                fitted,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CachedBehaviorBranches {
        no_food,
        food,
        reverse_food,
        rest_probes,
        food_feedback,
        reverse_food_feedback,
    })
}

fn run_cached_readout_condition(
    condition: CachedReadoutCondition<'_>,
) -> Result<
    (
        LearningConditionResult,
        CachedBehaviorReport,
        BatchConditionReport,
    ),
    Box<dyn Error>,
> {
    let CachedReadoutCondition {
        name,
        cache,
        feature_dimension,
        training_entries,
        evaluation_entries,
        held_out_entries: _held_out_entries,
        training_samples,
        tuning_samples,
        evaluation_samples,
        held_out_samples,
        standardization,
        criteria,
        training_mode,
        feedback,
        seed_base,
        shuffle_seed,
        decision_threshold,
        behavior,
    } = condition;
    let mut fitted = BTreeMap::new();
    let mut candidates = Vec::new();
    let mut tuning_scores_by_role = BTreeMap::new();
    if training_mode == TrainingMode::ReadoutOnly {
        for (role, role_seed) in [("change", 0_u64), ("no_change", 1_u64)] {
            let (model, mut role_candidates, tuning_scores) = fit_batch_readout(
                role,
                training_samples,
                tuning_samples,
                standardization,
                criteria,
                shuffle_seed.map(|seed| seed.wrapping_add(role_seed)),
            )?;
            candidates.append(&mut role_candidates);
            fitted.insert(role.to_owned(), model);
            tuning_scores_by_role.insert(role.to_owned(), tuning_scores);
        }
    }
    let selected_tuning_threshold = if fitted.is_empty() {
        criteria.readout_reaction_threshold
    } else {
        select_decision_threshold(
            tuning_samples,
            &tuning_scores_by_role,
            criteria.readout_reaction_threshold,
        )
    };
    let decision_threshold = decision_threshold.unwrap_or(selected_tuning_threshold);
    let mut runner = build_cached_readout_runner(CachedReadoutRunnerConfig {
        feature_dimension,
        criteria,
        training_mode,
        feedback: feedback.clone(),
        seed_base,
        standardization,
        reaction_threshold: decision_threshold,
        fitted: if fitted.is_empty() {
            None
        } else {
            Some(&fitted)
        },
    })?;
    let scores =
        run_cached_scoring_sequence(&mut runner, evaluation_samples, cache, "evaluation", 0)?;
    let unused_scores = run_cached_scoring_sequence(
        &mut runner,
        held_out_samples,
        cache,
        "held-out",
        evaluation_entries.len() as u64,
    )?;
    let evaluation_brain_results =
        brain_results_at_threshold(&scores, criteria, decision_threshold);
    let consensus = consensus_result_at_threshold(
        &scores,
        decision_threshold,
        criteria.acceptance.consensus_margin,
    );
    let held_out_brain_results =
        brain_results_at_threshold(&unused_scores, criteria, decision_threshold);
    let held_out_consensus = consensus_result_at_threshold(
        &unused_scores,
        decision_threshold,
        criteria.acceptance.consensus_margin,
    );
    let unused_screen_accuracy = held_out_consensus.accuracy;
    let behavior_branches = behavior
        .map(|_| {
            build_cached_behavior_branches(
                feature_dimension,
                criteria,
                training_mode,
                standardization,
                if fitted.is_empty() {
                    None
                } else {
                    Some(&fitted)
                },
                &feedback,
                seed_base,
                decision_threshold,
            )
        })
        .transpose()?;
    let behavior_report = if let Some(behavior) = behavior {
        run_cached_behavior_sequence(
            &mut runner,
            cache,
            training_entries,
            criteria,
            behavior,
            feedback,
            behavior_branches,
        )?
    } else {
        empty_cached_behavior_report()
    };
    let mut selected_learning_rates = BTreeMap::new();
    let mut selected_l2_values = BTreeMap::new();
    for (role, model) in &fitted {
        selected_learning_rates.insert(role.clone(), model.learning_rate);
        selected_l2_values.insert(role.clone(), model.l2);
    }
    let evaluation_prediction_distributions =
        prediction_distributions_from_scores(&scores, criteria);
    let held_out_prediction_distributions =
        prediction_distributions_from_scores(&unused_scores, criteria);
    let selected_readouts = fitted
        .iter()
        .map(|(role, model)| {
            let id = criteria
                .brain_roles
                .iter()
                .find(|brain| brain.role == *role)
                .map(|brain| brain.id.clone())
                .ok_or_else(|| format!("missing brain id for role {role}"))?;
            Ok(RateHelperBrain {
                id,
                role: role.clone(),
                feature_dimension,
                standardization_mean: standardization.mean.clone(),
                standardization_scale: standardization.scale.clone(),
                weights: model.weights.clone(),
                bias: model.bias,
                reaction_threshold: decision_threshold,
            })
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let batch_report = BatchConditionReport {
        name: name.to_owned(),
        epochs: criteria.batch_epochs,
        training_sample_count: training_samples.len(),
        tuning_sample_count: tuning_samples.len(),
        evaluation_sample_count: evaluation_samples.len(),
        held_out_sample_count: held_out_samples.len(),
        decision_threshold,
        candidates,
        selected_learning_rates,
        selected_l2_values,
        evaluation_prediction_distributions,
        held_out_prediction_distributions,
        selected_readouts,
    };
    let condition = LearningConditionResult {
        name: name.to_owned(),
        brain_results: evaluation_brain_results,
        consensus,
        held_out_brain_results,
        held_out_consensus,
        unused_screen_accuracy,
        training_presentations: training_samples.len(),
        tuning_presentations: tuning_samples.len(),
        evaluation_presentations: evaluation_samples.len(),
        held_out_presentations: held_out_samples.len(),
        positive_training_events: training_samples
            .iter()
            .filter(|sample| sample.automatic_change == Some(true))
            .count(),
        human_feedback_event_count: criteria.human_feedback.len(),
        wiring_changed_edge_count: 0,
        weight_changed_edge_count: 0,
        peak_rss_bytes: process_peak_rss_bytes(),
    };
    Ok((condition, behavior_report, batch_report))
}

fn run_cached_scoring_sequence(
    runner: &mut BrainRunner,
    samples: &[BatchSample],
    cache: &BTreeMap<String, CachedStimulus>,
    stream_id: &str,
    position_offset: u64,
) -> Result<Vec<BrainScores>, Box<dyn Error>> {
    let mut scores = Vec::with_capacity(samples.len());
    for (offset, sample) in samples.iter().enumerate() {
        let entry = &sample.entry;
        let features = cached_feature_for_entry(entry, cache)?.to_vec();
        let evaluation = runner.evaluate_all(&cached_brain_input_at_position(
            entry,
            features,
            stream_id,
            (entry.timestamp_s.max(0.0) * 1_000.0) as u64,
            position_offset + offset as u64,
        ))?;
        scores.push(brain_scores_from_evaluation(
            &evaluation,
            sample.automatic_change,
        )?);
        commit_evaluation(runner, evaluation, 0.0)?;
    }
    Ok(scores)
}

fn brain_scores_from_evaluation(
    evaluation: &habitua::OrchestrationEvaluation,
    expected_change: Option<bool>,
) -> Result<BrainScores, Box<dyn Error>> {
    let mut change = None;
    let mut no_change = None;
    for brain in &evaluation.brains {
        let probability = report_metric(&brain.report, "target_probability")?;
        match brain.role.as_str() {
            "change" => change = Some(probability),
            "no_change" => no_change = Some(probability),
            role => return Err(format!("unknown readout role {role}").into()),
        }
    }
    let brain_change_score = change.ok_or("change readout result is missing")?;
    let brain_no_change_score = no_change.ok_or("no-change readout result is missing")?;
    if !brain_change_score.is_finite() || !brain_no_change_score.is_finite() {
        return Err("readout probability is non-finite".into());
    }
    let readiness_sufficient = evaluation
        .brains
        .iter()
        .all(|brain| brain.report.readiness == habitua::Readiness::Evaluated);
    Ok(BrainScores {
        expected_change: expected_change
            .or_else(|| {
                evaluation
                    .brains
                    .iter()
                    .find(|brain| brain.role == "change")
                    .and_then(|brain| {
                        (brain.report.readiness == habitua::Readiness::Evaluated).then(|| {
                            brain
                                .report
                                .metrics
                                .iter()
                                .find(|metric| metric.name == "automatic_change")
                                .is_some_and(|metric| metric.value >= 0.5)
                        })
                    })
            })
            .unwrap_or(false),
        change_score: brain_change_score,
        brain_change_score,
        brain_no_change_score,
        readiness_sufficient,
    })
}

fn direct_teacher_condition(
    samples: &[BatchSample],
    held_out_samples: &[BatchSample],
    criteria: &LearningEvaluationCriteria,
    decision_threshold: f64,
) -> LearningConditionResult {
    let scores = samples
        .iter()
        .map(|sample| {
            let ready = sample.automatic_change.is_some();
            let changed = sample.automatic_change.unwrap_or(false);
            BrainScores {
                expected_change: changed,
                change_score: if ready {
                    f64::from(u8::from(changed))
                } else {
                    0.5
                },
                brain_change_score: if ready {
                    f64::from(u8::from(changed))
                } else {
                    0.5
                },
                brain_no_change_score: if ready {
                    f64::from(u8::from(!changed))
                } else {
                    0.5
                },
                readiness_sufficient: ready,
            }
        })
        .collect::<Vec<_>>();
    let held_out_scores = held_out_samples
        .iter()
        .map(|sample| {
            let ready = sample.automatic_change.is_some();
            let changed = sample.automatic_change.unwrap_or(false);
            BrainScores {
                expected_change: changed,
                change_score: if ready {
                    f64::from(u8::from(changed))
                } else {
                    0.5
                },
                brain_change_score: if ready {
                    f64::from(u8::from(changed))
                } else {
                    0.5
                },
                brain_no_change_score: if ready {
                    f64::from(u8::from(!changed))
                } else {
                    0.5
                },
                readiness_sufficient: ready,
            }
        })
        .collect::<Vec<_>>();
    let evaluation_consensus = consensus_result_at_threshold(
        &scores,
        decision_threshold,
        criteria.acceptance.consensus_margin,
    );
    let held_out_consensus = consensus_result_at_threshold(
        &held_out_scores,
        decision_threshold,
        criteria.acceptance.consensus_margin,
    );
    LearningConditionResult {
        name: "direct_teacher_rule".to_owned(),
        brain_results: brain_results_at_threshold(&scores, criteria, decision_threshold),
        consensus: evaluation_consensus.clone(),
        held_out_brain_results: brain_results_at_threshold(
            &held_out_scores,
            criteria,
            decision_threshold,
        ),
        held_out_consensus: held_out_consensus.clone(),
        unused_screen_accuracy: held_out_consensus.accuracy,
        training_presentations: 0,
        tuning_presentations: 0,
        evaluation_presentations: samples.len(),
        held_out_presentations: held_out_samples.len(),
        positive_training_events: 0,
        human_feedback_event_count: 0,
        wiring_changed_edge_count: 0,
        weight_changed_edge_count: 0,
        peak_rss_bytes: process_peak_rss_bytes(),
    }
}

fn empty_cached_behavior_report() -> CachedBehaviorReport {
    CachedBehaviorReport {
        steps: Vec::new(),
        rest_probes: Vec::new(),
        rest_history_expiry_verified: false,
        food_reversal: CachedFoodReversalReport {
            input_id: String::new(),
            change_probability_before: 0.0,
            change_probability_after: 0.0,
            no_change_probability_before: 0.0,
            no_change_probability_after: 0.0,
            consensus_before: "hold".to_owned(),
            consensus_after: "hold".to_owned(),
            reaction_count_before: 0,
            reaction_count_after: 0,
            reaction_signature_before: Vec::new(),
            reaction_signature_after: Vec::new(),
            diagnostic_reaction_count_before: 0,
            diagnostic_reaction_count_after: 0,
            diagnostic_reaction_signature_before: Vec::new(),
            diagnostic_reaction_signature_after: Vec::new(),
            selected_feedback_reversed: false,
            changed: false,
            feedback_commit_applied_before_re_evaluation: false,
            same_start_state_and_timestamp: false,
            no_food_control_change_probability: 0.0,
            no_food_control_consensus: "hold".to_owned(),
            no_food_control_reaction_signature: Vec::new(),
            no_food_control_probability_delta: 0.0,
            other_input_side_effect_probability_delta: 0.0,
            other_input_side_effect_action_changed: false,
            reverse_feedback_changed: false,
            automatic_overwrite_checked: false,
            automatic_overwrite_blocked: false,
            automatic_teacher_update_count_before: 0,
            automatic_teacher_update_count_after: 0,
            feedback_event_timestamp_s: 0.0,
            feedback_received_at_s: 0.0,
            feedback_commit_timestamp_s: 0.0,
            feedback_revision: 0,
            food_branch_start_timestamp_ms: 0,
            no_food_branch_start_timestamp_ms: 0,
            re_evaluation_timestamp_ms: 0,
            food_branch_start_state_sha256: String::new(),
            no_food_branch_start_state_sha256: String::new(),
            reverse_food_branch_start_state_sha256: String::new(),
            pre_commit_state_sha256: String::new(),
            post_commit_state_sha256: String::new(),
            state_hash_changed_after_commit: false,
            case_memory_match_count_before: 0,
            case_memory_match_count_after: 0,
            case_memory_match_count_other_input: 0,
            case_memory_nearest_distance_after: None,
            case_memory_adjustment_change_after: 0.0,
            case_memory_adjustment_no_change_after: 0.0,
        },
        repeat_change_probability: 0.0,
        different_change_probability: 0.0,
        after_rest_change_probability: 0.0,
        rest_recovery_probability_delta: 0.0,
        hold_rate: 0.0,
        rest_protocol: "none".to_owned(),
        long_repeat_count: 0,
        history_max_age_seconds: 0.0,
        food_branch_protocol: "none".to_owned(),
        acceptance: CachedBehaviorAcceptanceReport {
            initial_readiness_hold: false,
            repeat_change_lower_than_initial: false,
            no_change_retention: false,
            different_change_higher_than_repeat: false,
            rest_recovery: false,
            rest_history_expiry_verified: false,
            food_action_changed: false,
            food_reaction_changed: false,
            long_repeat_suppression: false,
            rest_gap_was_unobserved: false,
            food_no_food_control: false,
            reverse_food: false,
            no_other_input_side_effect: false,
            automatic_overwrite_blocked: false,
            all_passed: false,
        },
    }
}

fn run_cached_behavior_sequence(
    runner: &mut BrainRunner,
    cache: &BTreeMap<String, CachedStimulus>,
    entries: &[LearningScheduleEntry],
    criteria: &LearningEvaluationCriteria,
    behavior: &LearningBehaviorCriteria,
    _feedback: FrozenReadoutFeedback,
    mut behavior_branches: Option<CachedBehaviorBranches>,
) -> Result<CachedBehaviorReport, Box<dyn Error>> {
    let mut steps = Vec::new();
    let behavior_context = CachedBehaviorContext { cache, entries };
    run_cached_behavior_step(
        runner,
        &behavior_context,
        &mut steps,
        "initial",
        &behavior.initial_input_id,
        "behavior",
        50_000,
    )?;
    run_cached_behavior_step(
        runner,
        &behavior_context,
        &mut steps,
        "repeat",
        &behavior.repeat_input_id,
        "behavior",
        51_000,
    )?;
    for repeat_index in 0..behavior.long_repeat_count {
        run_cached_behavior_step(
            runner,
            &behavior_context,
            &mut steps,
            &format!("long_repeat_{:02}", repeat_index + 1),
            &behavior.repeat_input_id,
            "behavior",
            52_000 + repeat_index as u64 * 1_000,
        )?;
    }
    let different_timestamp_ms = 52_000 + behavior.long_repeat_count as u64 * 1_000;
    run_cached_behavior_step(
        runner,
        &behavior_context,
        &mut steps,
        "different",
        &behavior.different_input_id,
        "behavior",
        different_timestamp_ms,
    )?;
    let rest_probes = run_cached_rest_probes(
        &mut behavior_branches
            .as_mut()
            .ok_or("food branch runners were not constructed")?
            .rest_probes,
        &behavior_context,
        behavior,
        different_timestamp_ms,
    )?;

    let rest_start_s = different_timestamp_ms as f64 / 1_000.0;
    let after_rest_timestamp_ms =
        different_timestamp_ms.saturating_add((behavior.rest_duration_s.max(0.0) * 1_000.0) as u64);
    steps.push(CachedBehaviorStepReport {
        name: "rest_gap".to_owned(),
        input_id: "no_observation".to_owned(),
        stream_id: "behavior".to_owned(),
        elapsed_seconds: rest_start_s + behavior.rest_duration_s.max(0.0),
        brain_change_probability: 0.0,
        brain_no_change_probability: 0.0,
        automatic_change: None,
        readiness: "no_observation".to_owned(),
        consensus_action: "hold".to_owned(),
        active_reaction_count: 0,
        external_reaction_count: 0,
    });
    run_cached_behavior_step(
        runner,
        &behavior_context,
        &mut steps,
        "after_rest",
        &behavior.initial_input_id,
        "behavior",
        after_rest_timestamp_ms,
    )?;

    let reversal = run_food_branches(
        &behavior_context,
        behavior,
        behavior_branches
            .as_mut()
            .ok_or("food branch runners were not constructed")?,
    )?;
    let repeat = steps
        .iter()
        .find(|step| step.name == "repeat")
        .map(|step| step.brain_change_probability)
        .unwrap_or(0.0);
    let different = steps
        .iter()
        .find(|step| step.name == "different")
        .map(|step| step.brain_change_probability)
        .unwrap_or(0.0);
    let after_rest = steps
        .iter()
        .find(|step| step.name == "after_rest")
        .map(|step| step.brain_change_probability)
        .unwrap_or(0.0);
    let hold_rate = steps
        .iter()
        .filter(|step| step.consensus_action == "hold")
        .count() as f64
        / steps.len().max(1) as f64;
    let initial = steps.iter().find(|step| step.name == "initial");
    let repeat_step = steps.iter().find(|step| step.name == "repeat");
    let last_long_repeat_step = steps
        .iter()
        .rev()
        .find(|step| step.name.starts_with("long_repeat_"));
    let different_step = steps.iter().find(|step| step.name == "different");
    let after_rest_step = steps.iter().find(|step| step.name == "after_rest");
    let probability_margin = criteria.behavior_acceptance.minimum_probability_margin;
    let initial_readiness_hold = initial.is_some_and(|step| {
        step.readiness == "baseline_insufficient"
            && step.consensus_action == "hold"
            && step.active_reaction_count == 0
            && step.external_reaction_count == 0
    });
    let initial_change = initial.map(|step| step.brain_change_probability);
    let repeat_change_lower_than_initial = initial_change
        .zip(repeat_step.map(|step| step.brain_change_probability))
        .is_some_and(|(initial, repeat)| initial - repeat >= probability_margin);
    let no_change_retention = repeat_step
        .zip(different_step)
        .is_some_and(|(repeat, different)| {
            repeat.brain_no_change_probability - different.brain_no_change_probability
                >= probability_margin
        });
    let different_change_higher_than_repeat =
        repeat_step
            .zip(different_step)
            .is_some_and(|(repeat, different)| {
                different.brain_change_probability - repeat.brain_change_probability
                    >= probability_margin
            });
    let long_repeat_suppression =
        repeat_step
            .zip(last_long_repeat_step)
            .is_some_and(|(repeat, last_repeat)| {
                repeat.brain_change_probability - last_repeat.brain_change_probability
                    >= probability_margin
            });
    let rest_gap_was_unobserved = steps.iter().any(|step| {
        step.name == "rest_gap"
            && step.readiness == "no_observation"
            && step.consensus_action == "hold"
            && step.active_reaction_count == 0
            && step.external_reaction_count == 0
    });
    let rest_recovery = if criteria.behavior_acceptance.rest_recovery_rule
        == "after_rest_baseline_reset_and_external_hold"
    {
        after_rest_step.is_some_and(|step| {
            step.readiness == "baseline_insufficient"
                && step.consensus_action == "hold"
                && step.external_reaction_count == 0
        })
    } else {
        after_rest_step
            .zip(repeat_step)
            .is_some_and(|(after_rest, repeat)| {
                after_rest.brain_change_probability - repeat.brain_change_probability
                    >= criteria.behavior_acceptance.minimum_rest_recovery_delta
            })
    };
    let rest_history_expiry_verified = rest_probes
        .first()
        .is_some_and(|probe| probe.history_active)
        && rest_probes
            .last()
            .is_some_and(|probe| !probe.history_active && probe.consensus_action == "hold");
    let food_action_changed = reversal.consensus_before != reversal.consensus_after;
    let food_reaction_changed =
        reversal.reaction_signature_before != reversal.reaction_signature_after;
    let food_no_food_control = reversal.same_start_state_and_timestamp
        && reversal.consensus_after != reversal.no_food_control_consensus;
    let reverse_food = reversal.reverse_feedback_changed;
    let no_other_input_side_effect = !reversal.other_input_side_effect_action_changed
        && reversal.other_input_side_effect_probability_delta.abs()
            <= criteria
                .behavior_acceptance
                .maximum_other_input_side_effect_delta;
    let automatic_overwrite_blocked = reversal.automatic_overwrite_blocked;
    let acceptance = CachedBehaviorAcceptanceReport {
        initial_readiness_hold,
        repeat_change_lower_than_initial,
        no_change_retention,
        different_change_higher_than_repeat,
        rest_recovery,
        rest_history_expiry_verified,
        food_action_changed,
        food_reaction_changed,
        long_repeat_suppression,
        rest_gap_was_unobserved,
        food_no_food_control,
        reverse_food,
        no_other_input_side_effect,
        automatic_overwrite_blocked,
        all_passed: initial_readiness_hold
            && repeat_change_lower_than_initial
            && no_change_retention
            && different_change_higher_than_repeat
            && long_repeat_suppression
            && rest_gap_was_unobserved
            && rest_recovery
            && rest_history_expiry_verified
            && food_no_food_control
            && (!criteria.behavior_acceptance.require_reverse_food_change || reverse_food)
            && no_other_input_side_effect
            && (!criteria
                .behavior_acceptance
                .require_automatic_overwrite_check
                || automatic_overwrite_blocked)
            && (!criteria.behavior_acceptance.require_food_action_change || food_action_changed)
            && (!criteria.behavior_acceptance.require_food_reaction_change
                || food_reaction_changed),
    };
    Ok(CachedBehaviorReport {
        steps,
        rest_probes,
        rest_history_expiry_verified,
        food_reversal: reversal,
        repeat_change_probability: repeat,
        different_change_probability: different,
        after_rest_change_probability: after_rest,
        rest_recovery_probability_delta: after_rest - repeat,
        hold_rate,
        rest_protocol: behavior.rest_protocol.clone(),
        long_repeat_count: behavior.long_repeat_count,
        history_max_age_seconds: behavior.history_max_age_seconds,
        food_branch_protocol: behavior.food_branch_protocol.clone(),
        acceptance,
    })
}

fn run_food_branches(
    context: &CachedBehaviorContext<'_>,
    behavior: &LearningBehaviorCriteria,
    branches: &mut CachedBehaviorBranches,
) -> Result<CachedFoodReversalReport, Box<dyn Error>> {
    let stream = "food-reversal";
    let reversal_image_sha256 =
        image_sha256_for_input_id(&behavior.reversal_input_id, context.entries);
    let baseline_timestamp = 150_000;
    let feedback_timestamp = 151_000;
    let re_evaluation_timestamp = 151_000;
    let no_food_start_timestamp = baseline_timestamp;
    let food_start_timestamp = baseline_timestamp;
    let no_food_branch_start_state_sha256 = brain_runner_state_sha256(&branches.no_food)?;
    let food_branch_start_state_sha256 = brain_runner_state_sha256(&branches.food)?;
    let reverse_food_branch_start_state_sha256 = brain_runner_state_sha256(&branches.reverse_food)?;

    let no_food_before = run_step_capture(
        &mut branches.no_food,
        context,
        &behavior.reversal_input_id,
        stream,
        baseline_timestamp,
    )?;
    let food_before = run_step_capture(
        &mut branches.food,
        context,
        &behavior.reversal_input_id,
        stream,
        baseline_timestamp,
    )?;
    let reverse_before = run_step_capture(
        &mut branches.reverse_food,
        context,
        &behavior.reversal_input_id,
        stream,
        baseline_timestamp,
    )?;
    commit_evaluation(&mut branches.no_food, no_food_before.evaluation, 0.0)?;
    commit_evaluation(&mut branches.food, food_before.evaluation, 0.0)?;
    commit_evaluation(&mut branches.reverse_food, reverse_before.evaluation, 0.0)?;

    let no_food_feedback_control = run_step_capture(
        &mut branches.no_food,
        context,
        &behavior.reversal_input_id,
        stream,
        feedback_timestamp,
    )?;
    commit_evaluation(
        &mut branches.no_food,
        no_food_feedback_control.evaluation,
        0.0,
    )?;

    branches.food_feedback.insert(
        format!("change:{reversal_image_sha256}"),
        FeedbackEvent::new(
            "canonical-food-change",
            reversal_image_sha256.clone(),
            FeedbackKind::Reward,
            behavior.reversal_strength,
            101.0,
        )?,
    );
    branches.food_feedback.insert(
        format!("no_change:{reversal_image_sha256}"),
        FeedbackEvent::new(
            "canonical-food-no-change",
            reversal_image_sha256.clone(),
            FeedbackKind::Punish,
            behavior.reversal_strength,
            101.0,
        )?,
    );
    let food_feedback = run_step_capture(
        &mut branches.food,
        context,
        &behavior.reversal_input_id,
        stream,
        feedback_timestamp,
    )?;
    let selected_feedback_reversed = food_feedback.human_feedback_applied;
    let pre_commit_state_sha256 = brain_runner_state_sha256(&branches.food)?;
    commit_evaluation(&mut branches.food, food_feedback.evaluation, 1.0)?;
    let post_commit_state_sha256 = brain_runner_state_sha256(&branches.food)?;
    let food_after = run_step_capture_at(
        &mut branches.food,
        context,
        &behavior.reversal_input_id,
        stream,
        re_evaluation_timestamp,
        re_evaluation_timestamp + 1,
    )?;
    commit_evaluation(&mut branches.food, food_after.evaluation, 0.0)?;

    let no_food_after = run_step_capture(
        &mut branches.no_food,
        context,
        &behavior.reversal_input_id,
        stream,
        re_evaluation_timestamp,
    )?;
    commit_evaluation(&mut branches.no_food, no_food_after.evaluation, 0.0)?;

    branches.food_feedback.clear();
    let automatic_check = run_step_capture_at(
        &mut branches.food,
        context,
        &behavior.reversal_input_id,
        stream,
        re_evaluation_timestamp,
        re_evaluation_timestamp + 2,
    )?;
    commit_evaluation(&mut branches.food, automatic_check.evaluation, 0.0)?;
    let automatic_recheck = run_step_capture_at(
        &mut branches.food,
        context,
        &behavior.reversal_input_id,
        stream,
        re_evaluation_timestamp,
        re_evaluation_timestamp + 3,
    )?;
    let automatic_overwrite_blocked = !automatic_check.human_feedback_applied
        && !automatic_recheck.human_feedback_applied
        && automatic_recheck.readout_update_count == automatic_check.readout_update_count;
    commit_evaluation(&mut branches.food, automatic_recheck.evaluation, 0.0)?;

    branches.reverse_food_feedback.insert(
        format!("change:{reversal_image_sha256}"),
        FeedbackEvent::new(
            "canonical-reverse-food-change",
            reversal_image_sha256.clone(),
            FeedbackKind::Punish,
            behavior.reversal_strength,
            101.0,
        )?,
    );
    branches.reverse_food_feedback.insert(
        format!("no_change:{reversal_image_sha256}"),
        FeedbackEvent::new(
            "canonical-reverse-food-no-change",
            reversal_image_sha256,
            FeedbackKind::Reward,
            behavior.reversal_strength,
            101.0,
        )?,
    );
    let reverse_feedback = run_step_capture(
        &mut branches.reverse_food,
        context,
        &behavior.reversal_input_id,
        stream,
        feedback_timestamp,
    )?;
    commit_evaluation(&mut branches.reverse_food, reverse_feedback.evaluation, 1.0)?;
    let reverse_after = run_step_capture_at(
        &mut branches.reverse_food,
        context,
        &behavior.reversal_input_id,
        stream,
        re_evaluation_timestamp,
        re_evaluation_timestamp + 1,
    )?;
    commit_evaluation(&mut branches.reverse_food, reverse_after.evaluation, 0.0)?;

    let no_food_other = run_step_capture(
        &mut branches.no_food,
        context,
        &behavior.different_input_id,
        stream,
        152_000,
    )?;
    commit_evaluation(&mut branches.no_food, no_food_other.evaluation, 0.0)?;
    let food_other = run_step_capture(
        &mut branches.food,
        context,
        &behavior.different_input_id,
        stream,
        152_000,
    )?;
    commit_evaluation(&mut branches.food, food_other.evaluation, 0.0)?;

    Ok(CachedFoodReversalReport {
        input_id: behavior.reversal_input_id.clone(),
        change_probability_before: food_before.scores.brain_change_score,
        change_probability_after: food_after.scores.brain_change_score,
        no_change_probability_before: food_before.scores.brain_no_change_score,
        no_change_probability_after: food_after.scores.brain_no_change_score,
        consensus_before: food_before.action,
        consensus_after: food_after.action,
        reaction_count_before: food_before.external_reaction_count,
        reaction_count_after: food_after.external_reaction_count,
        reaction_signature_before: food_before.external_reaction_signature,
        reaction_signature_after: food_after.external_reaction_signature,
        diagnostic_reaction_count_before: food_before.diagnostic_reaction_count,
        diagnostic_reaction_count_after: food_after.diagnostic_reaction_count,
        diagnostic_reaction_signature_before: food_before.diagnostic_reaction_signature,
        diagnostic_reaction_signature_after: food_after.diagnostic_reaction_signature,
        selected_feedback_reversed,
        changed: (food_after.scores.brain_change_score - food_before.scores.brain_change_score)
            .abs()
            > 1.0e-6
            || (food_after.scores.brain_no_change_score - food_before.scores.brain_no_change_score)
                .abs()
                > 1.0e-6,
        feedback_commit_applied_before_re_evaluation: selected_feedback_reversed
            && feedback_timestamp <= re_evaluation_timestamp,
        same_start_state_and_timestamp: food_branch_start_state_sha256
            == no_food_branch_start_state_sha256
            && food_branch_start_state_sha256 == reverse_food_branch_start_state_sha256
            && food_start_timestamp == no_food_start_timestamp,
        no_food_control_change_probability: no_food_after.scores.brain_change_score,
        no_food_control_consensus: no_food_after.action,
        no_food_control_reaction_signature: no_food_after.external_reaction_signature,
        no_food_control_probability_delta: food_after.scores.brain_change_score
            - no_food_after.scores.brain_change_score,
        other_input_side_effect_probability_delta: food_other.scores.brain_change_score
            - no_food_other.scores.brain_change_score,
        other_input_side_effect_action_changed: food_other.action != no_food_other.action,
        reverse_feedback_changed: reverse_after.action != reverse_before.action
            || (reverse_after.scores.brain_change_score - reverse_before.scores.brain_change_score)
                .abs()
                > 1.0e-6,
        automatic_overwrite_checked: !automatic_check.human_feedback_applied
            && !automatic_recheck.human_feedback_applied,
        automatic_overwrite_blocked,
        automatic_teacher_update_count_before: automatic_check.readout_update_count,
        automatic_teacher_update_count_after: automatic_recheck.readout_update_count,
        feedback_event_timestamp_s: 101.0,
        feedback_received_at_s: 101.0,
        feedback_commit_timestamp_s: feedback_timestamp as f64 / 1_000.0,
        feedback_revision: 0,
        food_branch_start_timestamp_ms: food_start_timestamp,
        no_food_branch_start_timestamp_ms: no_food_start_timestamp,
        re_evaluation_timestamp_ms: re_evaluation_timestamp,
        food_branch_start_state_sha256,
        no_food_branch_start_state_sha256,
        reverse_food_branch_start_state_sha256,
        state_hash_changed_after_commit: pre_commit_state_sha256 != post_commit_state_sha256,
        pre_commit_state_sha256,
        post_commit_state_sha256,
        case_memory_match_count_before: food_before.case_memory_match_count,
        case_memory_match_count_after: food_after.case_memory_match_count,
        case_memory_match_count_other_input: food_other.case_memory_match_count,
        case_memory_nearest_distance_after: food_after.case_memory_nearest_distance,
        case_memory_adjustment_change_after: food_after.case_memory_adjustment_change,
        case_memory_adjustment_no_change_after: food_after.case_memory_adjustment_no_change,
    })
}

struct CachedBehaviorContext<'a> {
    cache: &'a BTreeMap<String, CachedStimulus>,
    entries: &'a [LearningScheduleEntry],
}

fn run_cached_behavior_step(
    runner: &mut BrainRunner,
    context: &CachedBehaviorContext<'_>,
    steps: &mut Vec<CachedBehaviorStepReport>,
    name: &str,
    input_id: &str,
    stream_id: &str,
    timestamp_ms: u64,
) -> Result<(), Box<dyn Error>> {
    let stimulus = cached_stimulus_by_id(input_id, context.entries, context.cache)?;
    let image_sha256 = image_sha256_for_input_id(input_id, context.entries);
    let entry = LearningScheduleEntry {
        id: input_id.to_owned(),
        kind: ScreenFixtureKind::Same,
        pair_index: 0,
        side: "a".to_owned(),
        image_sha256,
        expected_change: false,
        timestamp_s: timestamp_ms as f64 / 1_000.0,
    };
    let evaluation = runner.evaluate_all(&cached_brain_input(
        &entry,
        stimulus.response.features.clone(),
        stream_id,
        timestamp_ms,
    ))?;
    let scores = brain_scores_from_evaluation(&evaluation, None)?;
    let automatic_change = evaluation
        .brains
        .iter()
        .find(|brain| brain.role == "change")
        .and_then(|brain| {
            (brain.report.readiness == habitua::Readiness::Evaluated).then(|| {
                brain
                    .report
                    .metrics
                    .iter()
                    .find(|metric| metric.name == "automatic_change")
                    .is_some_and(|metric| metric.value >= 0.5)
            })
        });
    steps.push(CachedBehaviorStepReport {
        name: name.to_owned(),
        input_id: input_id.to_owned(),
        stream_id: stream_id.to_owned(),
        elapsed_seconds: timestamp_ms as f64 / 1_000.0,
        brain_change_probability: scores.brain_change_score,
        brain_no_change_probability: scores.brain_no_change_score,
        automatic_change,
        readiness: if scores.readiness_sufficient {
            "evaluated".to_owned()
        } else {
            "baseline_insufficient".to_owned()
        },
        consensus_action: consensus_action_name(evaluation.consensus_action).to_owned(),
        active_reaction_count: evaluation.active_reactions.len(),
        external_reaction_count: evaluation.external_reactions.len(),
    });
    commit_evaluation(runner, evaluation, 0.0)?;
    Ok(())
}

fn run_cached_rest_probes(
    runners: &mut [BrainRunner],
    context: &CachedBehaviorContext<'_>,
    behavior: &LearningBehaviorCriteria,
    rest_start_timestamp_ms: u64,
) -> Result<Vec<CachedRestProbeReport>, Box<dyn Error>> {
    let full_duration = behavior.rest_duration_s.max(0.0);
    let durations = [0.0, full_duration * 0.5, full_duration, full_duration * 2.0];
    if runners.len() != durations.len() {
        return Err("rest probe runner count does not match the fixed probe schedule".into());
    }
    let mut reports = Vec::with_capacity(durations.len());
    for (probe_index, (runner, duration_seconds)) in runners.iter_mut().zip(durations).enumerate() {
        let stream_id = format!("rest-probe-{probe_index}");
        let pre_rest = run_step_capture_at(
            runner,
            context,
            &behavior.different_input_id,
            &stream_id,
            rest_start_timestamp_ms,
            rest_start_timestamp_ms,
        )?;
        commit_evaluation(runner, pre_rest.evaluation, 0.0)?;
        let elapsed_ms = (duration_seconds.max(0.0) * 1_000.0) as u64;
        let probe = run_step_capture_at(
            runner,
            context,
            &behavior.initial_input_id,
            &stream_id,
            rest_start_timestamp_ms.saturating_add(elapsed_ms),
            rest_start_timestamp_ms
                .saturating_add(elapsed_ms)
                .saturating_add(1),
        )?;
        let history_active = probe.feature_distance.is_some();
        reports.push(CachedRestProbeReport {
            duration_seconds,
            feature_distance: probe.feature_distance,
            previous_absolute_difference: probe.previous_absolute_difference,
            elapsed_seconds: probe.elapsed_seconds,
            repeat_count: probe.repeat_count,
            history_active,
            brain_change_probability: probe.scores.brain_change_score,
            brain_no_change_probability: probe.scores.brain_no_change_score,
            readiness: if probe.scores.readiness_sufficient {
                "evaluated".to_owned()
            } else {
                "baseline_insufficient".to_owned()
            },
            consensus_action: probe.action,
        });
        commit_evaluation(runner, probe.evaluation, 0.0)?;
    }
    Ok(reports)
}

struct CapturedBehaviorEvaluation {
    evaluation: habitua::OrchestrationEvaluation,
    scores: BrainScores,
    action: String,
    diagnostic_reaction_count: usize,
    external_reaction_count: usize,
    diagnostic_reaction_signature: Vec<String>,
    external_reaction_signature: Vec<String>,
    human_feedback_applied: bool,
    readout_update_count: u64,
    case_memory_match_count: usize,
    case_memory_nearest_distance: Option<f64>,
    case_memory_adjustment_change: f64,
    case_memory_adjustment_no_change: f64,
    feature_distance: Option<f64>,
    previous_absolute_difference: Option<f64>,
    elapsed_seconds: Option<f64>,
    repeat_count: usize,
}

fn run_step_capture(
    runner: &mut BrainRunner,
    context: &CachedBehaviorContext<'_>,
    input_id: &str,
    stream_id: &str,
    position: u64,
) -> Result<CapturedBehaviorEvaluation, Box<dyn Error>> {
    run_step_capture_at(runner, context, input_id, stream_id, position, position)
}

fn run_step_capture_at(
    runner: &mut BrainRunner,
    context: &CachedBehaviorContext<'_>,
    input_id: &str,
    stream_id: &str,
    timestamp_ms: u64,
    processing_position: u64,
) -> Result<CapturedBehaviorEvaluation, Box<dyn Error>> {
    let stimulus = cached_stimulus_by_id(input_id, context.entries, context.cache)?;
    let image_sha256 = image_sha256_for_input_id(input_id, context.entries);
    let entry = LearningScheduleEntry {
        id: input_id.to_owned(),
        kind: ScreenFixtureKind::Same,
        pair_index: 0,
        side: "a".to_owned(),
        image_sha256,
        expected_change: false,
        timestamp_s: timestamp_ms as f64 / 1_000.0,
    };
    let evaluation = runner.evaluate_all(&cached_brain_input_at_position(
        &entry,
        stimulus.response.features.clone(),
        stream_id,
        timestamp_ms,
        processing_position,
    ))?;
    let scores = brain_scores_from_evaluation(&evaluation, None)?;
    let human_feedback_applied = !evaluation.brains.is_empty()
        && evaluation.brains.iter().all(|brain| {
            brain
                .report
                .metrics
                .iter()
                .any(|metric| metric.name == "human_feedback_applied" && metric.value >= 0.5)
        });
    let readout_update_count = evaluation
        .brains
        .iter()
        .filter_map(|brain| {
            brain
                .report
                .metrics
                .iter()
                .find(|metric| metric.name == "readout_update_count")
                .map(|metric| metric.value.max(0.0) as u64)
        })
        .sum();
    let case_memory_match_count = evaluation
        .brains
        .iter()
        .filter_map(|brain| {
            brain
                .report
                .metrics
                .iter()
                .find(|metric| metric.name == "case_memory_match_count")
                .map(|metric| metric.value.max(0.0) as usize)
        })
        .sum();
    let case_memory_nearest_distance = evaluation
        .brains
        .iter()
        .filter_map(|brain| {
            brain
                .report
                .metrics
                .iter()
                .find(|metric| metric.name == "case_memory_nearest_distance")
                .map(|metric| f64::from(metric.value))
        })
        .min_by(f64::total_cmp);
    let case_memory_adjustment_change = evaluation
        .brains
        .iter()
        .find(|brain| brain.role == "change")
        .and_then(|brain| {
            brain
                .report
                .metrics
                .iter()
                .find(|metric| metric.name == "case_memory_adjustment")
        })
        .map_or(0.0, |metric| f64::from(metric.value));
    let case_memory_adjustment_no_change = evaluation
        .brains
        .iter()
        .find(|brain| brain.role == "no_change")
        .and_then(|brain| {
            brain
                .report
                .metrics
                .iter()
                .find(|metric| metric.name == "case_memory_adjustment")
        })
        .map_or(0.0, |metric| f64::from(metric.value));
    let change_report = evaluation
        .brains
        .iter()
        .find(|brain| brain.role == "change")
        .map(|brain| &brain.report);
    let feature_distance =
        change_report.and_then(|report| optional_report_metric(report, "feature_distance"));
    let previous_absolute_difference = change_report
        .and_then(|report| optional_report_metric(report, "previous_absolute_difference"));
    let elapsed_seconds =
        change_report.and_then(|report| optional_report_metric(report, "elapsed_seconds"));
    let repeat_count = change_report
        .and_then(|report| optional_report_metric(report, "repeat_count"))
        .unwrap_or(0.0)
        .max(0.0) as usize;
    Ok(CapturedBehaviorEvaluation {
        action: consensus_action_name(evaluation.consensus_action).to_owned(),
        diagnostic_reaction_count: evaluation.active_reactions.len(),
        external_reaction_count: evaluation.external_reactions.len(),
        diagnostic_reaction_signature: reaction_signature(&evaluation.active_reactions),
        external_reaction_signature: reaction_signature(&evaluation.external_reactions),
        evaluation,
        scores,
        human_feedback_applied,
        readout_update_count,
        case_memory_match_count,
        case_memory_nearest_distance,
        case_memory_adjustment_change,
        case_memory_adjustment_no_change,
        feature_distance,
        previous_absolute_difference,
        elapsed_seconds,
        repeat_count,
    })
}

fn optional_report_metric(report: &habitua::ModelReport, name: &str) -> Option<f64> {
    report
        .metrics
        .iter()
        .find(|metric| metric.name == name)
        .map(|metric| f64::from(metric.value))
}

fn reaction_signature(reactions: &[habitua::Reaction]) -> Vec<String> {
    let mut signature = reactions
        .iter()
        .map(|reaction| format!("{}:{:?}", reaction.brain, reaction.kind))
        .collect::<Vec<_>>();
    signature.sort();
    signature
}

fn consensus_action_name(action: ConsensusAction) -> &'static str {
    match action {
        ConsensusAction::NotifyChange => "notify_change",
        ConsensusAction::SuppressNoChange => "suppress_no_change",
        ConsensusAction::Hold => "hold",
    }
}

fn cached_stimulus_by_id<'a>(
    input_id: &str,
    entries: &[LearningScheduleEntry],
    cache: &'a BTreeMap<String, CachedStimulus>,
) -> Result<&'a CachedStimulus, Box<dyn Error>> {
    let entry = entries
        .iter()
        .find(|entry| entry.id == input_id)
        .ok_or_else(|| format!("behavior input {input_id} is missing from the schedule"))?;
    cache
        .get(&entry.image_sha256)
        .ok_or_else(|| format!("behavior input {input_id} is not cached").into())
}

#[cfg(test)]
fn consensus_action(change_score: f64, no_change_score: f64, margin: f64) -> &'static str {
    consensus_action_at_threshold(change_score, no_change_score, 0.5, margin)
}

#[cfg(test)]
fn consensus_action_at_threshold(
    change_score: f64,
    no_change_score: f64,
    threshold: f64,
    margin: f64,
) -> &'static str {
    if change_score >= threshold + margin && no_change_score <= threshold - margin {
        "notify_change"
    } else if change_score <= threshold - margin && no_change_score >= threshold + margin {
        "suppress_no_change"
    } else {
        "hold"
    }
}

#[cfg(test)]
fn consensus_action_with_readiness(
    change_score: f64,
    no_change_score: f64,
    threshold: f64,
    margin: f64,
    readiness_sufficient: bool,
) -> &'static str {
    if !readiness_sufficient {
        "hold"
    } else {
        consensus_action_at_threshold(change_score, no_change_score, threshold, margin)
    }
}

fn learning_evaluate_legacy(values: &BTreeMap<String, String>) -> Result<(), Box<dyn Error>> {
    let manifest_path = required_path(values, "--learning-manifest")?;
    let output = required_path(values, "--output")?;
    let peak_rss_bytes = values
        .get("--peak-rss-bytes")
        .map(|value| value.parse::<u64>())
        .transpose()?;
    reject_unknown(
        values,
        &["--learning-manifest", "--output", "--peak-rss-bytes"],
    )?;
    let context = read_learning_context(&manifest_path)?;
    let graph = Arc::new(RateGraph::load(&context.manifest.canonical_pack)?);
    let retina = RetinaMap::from_graph(&graph, retina_config(&context.base.criteria))?;
    let readout_indices = graph
        .population(&context.base.criteria.readout_population)
        .map_err(|error| Box::<dyn Error>::from(error.to_string()))?
        .to_vec();
    let entries = learning_schedule(
        &context.criteria,
        &context.manifest.base_evaluation_manifest_sha256,
        &context.base.fixtures,
    )?;
    let main_graph = Arc::clone(&graph);
    let learned = run_learning_condition(
        "learned",
        main_graph,
        &retina,
        &readout_indices,
        &context.criteria,
        &entries.entries,
        TrainingMode::Full,
        human_feedback_for_criteria(&context.criteria, &entries.entries)?,
        0,
        context.criteria.seeds.base,
    )?;
    let frozen = run_learning_condition(
        "frozen",
        Arc::clone(&graph),
        &retina,
        &readout_indices,
        &context.criteria,
        &entries.entries,
        TrainingMode::Frozen,
        human_feedback_for_criteria(&context.criteria, &entries.entries)?,
        0,
        context.criteria.seeds.base,
    )?;
    let reward_shuffled = run_learning_condition(
        "reward_shuffled",
        Arc::clone(&graph),
        &retina,
        &readout_indices,
        &context.criteria,
        &entries.entries,
        TrainingMode::Full,
        reward_shuffled_feedback(&context.criteria, &entries.entries)?,
        0,
        context.criteria.seeds.reward_shuffle,
    )?;
    let wiring = shuffle_wiring(&graph, context.criteria.seeds.wiring_shuffle, 32)?;
    let wiring_changed_edge_count = wiring.stats.changed_edge_count;
    let wiring_graph = Arc::new(wiring.graph);
    let wiring_result = run_learning_condition(
        "wiring_shuffled",
        wiring_graph,
        &retina,
        &readout_indices,
        &context.criteria,
        &entries.entries,
        TrainingMode::Full,
        human_feedback_for_criteria(&context.criteria, &entries.entries)?,
        wiring_changed_edge_count,
        context.criteria.seeds.wiring_shuffle,
    )?;
    let weights = shuffle_weights(&graph, context.criteria.seeds.weight_shuffle)?;
    let weight_changed_edge_count = weights.stats.changed_edge_count;
    let weight_graph = Arc::new(weights.graph);
    let weight_result = run_learning_condition(
        "weight_shuffled",
        weight_graph,
        &retina,
        &readout_indices,
        &context.criteria,
        &entries.entries,
        TrainingMode::Full,
        human_feedback_for_criteria(&context.criteria, &entries.entries)?,
        0,
        context.criteria.seeds.weight_shuffle,
    )?;
    let mut conditions = vec![
        learned,
        frozen,
        reward_shuffled,
        wiring_result,
        weight_result,
    ];
    if let Some(condition) = conditions
        .iter_mut()
        .find(|condition| condition.name == "wiring_shuffled")
    {
        condition.wiring_changed_edge_count = wiring_changed_edge_count;
    }
    if let Some(condition) = conditions
        .iter_mut()
        .find(|condition| condition.name == "weight_shuffled")
    {
        condition.weight_changed_edge_count = weight_changed_edge_count;
    }
    let report = LearningEvaluationReport {
        manifest_sha256: context.manifest_sha256,
        criteria_sha256: context.manifest.criteria_sha256.clone(),
        base_evaluation_manifest_sha256: context.manifest.base_evaluation_manifest_sha256,
        fixtures_sha256: context.manifest.fixtures_sha256,
        pack_manifest_sha256: context.manifest.canonical_pack_manifest_sha256,
        seed_schedule_sha256: context.manifest.seed_schedule_sha256,
        input_order: context.criteria.input_order,
        distance_metric: context.criteria.distance_metric,
        distance_threshold: context.criteria.distance_threshold,
        acceptance: context.criteria.acceptance,
        conditions,
        all_rate_values_finite: true,
        peak_rss_bytes: peak_rss_bytes.or_else(process_peak_rss_bytes),
        rss_measurement: "macOS getrusage(RUSAGE_SELF).ru_maxrss の累積 high-water。全 condition を同一プロセスで順に実行し、外部 --peak-rss-bytes があればそれを全体値として併記する。".to_owned(),
    };
    write_json(&output, &report)
}

#[allow(clippy::too_many_arguments)]
fn run_learning_condition(
    name: &str,
    graph: Arc<RateGraph>,
    retina: &RetinaMap,
    readout_indices: &[u32],
    criteria: &LearningEvaluationCriteria,
    entries: &[LearningScheduleEntry],
    training_mode: TrainingMode,
    human_feedback: BTreeMap<String, FeedbackEvent>,
    wiring_changed_edge_count: usize,
    seed_base: u64,
) -> Result<LearningConditionResult, Box<dyn Error>> {
    let mut runner = build_rate_brain_runner(
        Arc::clone(&graph),
        readout_indices,
        criteria,
        training_mode,
        human_feedback,
        seed_base,
    )?;
    let mut positive_training_events = 0;
    for (position, entry) in entries.iter().enumerate() {
        let features = fixture_features(entry, retina, graph.neuron_count())?;
        let evaluation =
            runner.evaluate_all(&brain_input(entry, features, "training", position as u64))?;
        let weight = if entry.side == criteria.unlabeled_side {
            0.0
        } else {
            positive_training_events += 1;
            1.0
        };
        commit_evaluation(&mut runner, evaluation, weight)?;
    }
    let evaluation_scores = run_scoring_sequence(
        &mut runner,
        entries,
        retina,
        graph.neuron_count(),
        "evaluation",
        entries.len() as u64,
    )?;
    let held_out = learning_schedule_for_held_out(
        criteria,
        entries,
        retina.config.width,
        retina.config.height,
    )?;
    let unused_scores = run_scoring_sequence(
        &mut runner,
        &held_out,
        retina,
        graph.neuron_count(),
        "held-out",
        (entries.len() * 2) as u64,
    )?;
    let evaluation_brain_results = brain_results(&evaluation_scores, criteria);
    let consensus = consensus_result(&evaluation_scores, criteria.acceptance.consensus_margin);
    let unused_screen_accuracy =
        consensus_result(&unused_scores, criteria.acceptance.consensus_margin).accuracy;
    Ok(LearningConditionResult {
        name: name.to_owned(),
        brain_results: evaluation_brain_results,
        consensus,
        held_out_brain_results: brain_results(&unused_scores, criteria),
        held_out_consensus: consensus_result(&unused_scores, criteria.acceptance.consensus_margin),
        unused_screen_accuracy,
        training_presentations: entries.len(),
        tuning_presentations: 0,
        evaluation_presentations: evaluation_scores.len(),
        held_out_presentations: unused_scores.len(),
        positive_training_events,
        human_feedback_event_count: criteria.human_feedback.len(),
        wiring_changed_edge_count,
        weight_changed_edge_count: 0,
        peak_rss_bytes: process_peak_rss_bytes(),
    })
}

fn build_rate_brain_runner(
    graph: Arc<RateGraph>,
    readout_indices: &[u32],
    criteria: &LearningEvaluationCriteria,
    training_mode: TrainingMode,
    human_feedback: BTreeMap<String, FeedbackEvent>,
    seed_base: u64,
) -> Result<BrainRunner, Box<dyn Error>> {
    let learning_config = RateLearningConfig {
        steps_per_observation: criteria.rate_steps_per_observation,
        ..RateLearningConfig::default()
    };
    let mut registry = ModelRegistry::empty();
    register_rate_brain_model(
        &mut registry,
        graph,
        readout_indices.to_vec(),
        criteria.projection_dimension,
        seed_base,
        learning_config,
        human_feedback,
    )?;
    let configs = criteria
        .brain_roles
        .iter()
        .map(|brain| {
            let mut settings = BTreeMap::new();
            settings.insert("role".to_owned(), brain.role.clone());
            settings.insert(
                "recent_observations".to_owned(),
                criteria.recent_observations.to_string(),
            );
            settings.insert(
                "distance_threshold".to_owned(),
                criteria.distance_threshold.to_string(),
            );
            settings.insert(
                "training_mode".to_owned(),
                training_mode_name(training_mode).to_owned(),
            );
            settings.insert("seed".to_owned(), brain.seed.to_string());
            BrainConfig {
                id: brain.id.clone(),
                role: brain.role.clone(),
                enabled: true,
                mode: BrainMode::Active,
                input: BrainInputSpec {
                    schema_id: "malecns-retina".to_owned(),
                    schema_version: 1,
                    target: "screen".to_owned(),
                    context: "malecns-rate-learning".to_owned(),
                },
                model: ModelSpec {
                    kind: "rate_brain".to_owned(),
                    version: 1,
                    seed: Some(brain.seed),
                    settings: ModelSettings::Custom(settings),
                },
                learning: BrainLearningPolicy {
                    mode: LearningMode::EveryEvaluation,
                    weight: 1.0,
                    require_evaluated: false,
                    bootstrap: false,
                    learn_on_reaction: false,
                    frequency_limit: None,
                },
                reactions: ReactionPolicy::default(),
                resources: ResourcePolicy {
                    max_streams: 8,
                    max_samples: criteria.presentation_count,
                    max_memory_bytes: 4_000_000,
                    evaluation_deadline: None,
                },
                state: StatePolicy {
                    format_version: 1,
                    configuration_identity: format!(
                        "malecns-rate-learning:{}:{}",
                        criteria.distance_threshold.to_bits(),
                        training_mode_name(training_mode)
                    ),
                    deletion: DeletionPolicy::Delete,
                },
            }
        })
        .collect::<Vec<_>>();
    Ok(BrainRunner::new(configs, &registry)?)
}

fn training_mode_name(mode: TrainingMode) -> &'static str {
    match mode {
        TrainingMode::Frozen => "frozen",
        TrainingMode::ReadoutOnly => "readout_only",
        TrainingMode::Full => "full",
    }
}

fn commit_evaluation(
    runner: &mut BrainRunner,
    evaluation: habitua::OrchestrationEvaluation,
    weight: f32,
) -> Result<(), Box<dyn Error>> {
    for brain in evaluation.brains {
        if let Some(update) = brain.update {
            runner.commit(update, weight)?;
        }
    }
    Ok(())
}

fn brain_runner_state_sha256(runner: &BrainRunner) -> Result<String, Box<dyn Error>> {
    let mut bytes = Vec::new();
    runner.save(&mut bytes)?;
    Ok(sha256_bytes(&bytes))
}

fn run_scoring_sequence(
    runner: &mut BrainRunner,
    entries: &[LearningScheduleEntry],
    retina: &RetinaMap,
    neuron_count: usize,
    stream_id: &str,
    position_offset: u64,
) -> Result<Vec<BrainScores>, Box<dyn Error>> {
    let mut results = Vec::with_capacity(entries.len());
    for (offset, entry) in entries.iter().enumerate() {
        let features = fixture_features(entry, retina, neuron_count)?;
        let evaluation = runner.evaluate_all(&brain_input(
            entry,
            features,
            stream_id,
            position_offset + offset as u64,
        ))?;
        let mut brain_change_score = None;
        let mut brain_no_change_score = None;
        for brain in &evaluation.brains {
            let probability = report_metric(&brain.report, "value_probability")?;
            match brain.role.as_str() {
                "change" => brain_change_score = Some(probability),
                "no_change" => brain_no_change_score = Some(probability),
                role => return Err(format!("unknown rate brain role in report: {role}").into()),
            }
        }
        let brain_change_score = brain_change_score.ok_or("change brain result is missing")?;
        let brain_no_change_score =
            brain_no_change_score.ok_or("no-change brain result is missing")?;
        if !brain_change_score.is_finite() || !brain_no_change_score.is_finite() {
            return Err("rate brain returned a non-finite value probability".into());
        }
        results.push(BrainScores {
            expected_change: entry.expected_change,
            change_score: brain_change_score,
            brain_change_score,
            brain_no_change_score,
            readiness_sufficient: evaluation
                .brains
                .iter()
                .all(|brain| brain.report.readiness == habitua::Readiness::Evaluated),
        });
        commit_evaluation(runner, evaluation, 0.0)?;
    }
    Ok(results)
}

fn brain_input(
    entry: &LearningScheduleEntry,
    features: Vec<f32>,
    stream_id: &str,
    position: u64,
) -> BrainInput {
    BrainInput {
        id: entry.id.clone(),
        stream_id: stream_id.to_owned(),
        target: "screen".to_owned(),
        context: "malecns-rate-learning".to_owned(),
        schema_id: "malecns-retina".to_owned(),
        schema_version: 1,
        features,
        failure: None,
        available_at: Timestamp::from_millis(position),
        processed_at: None,
        position,
    }
}

fn fixture_features(
    entry: &LearningScheduleEntry,
    retina: &RetinaMap,
    neuron_count: usize,
) -> Result<Vec<f32>, Box<dyn Error>> {
    let fixture = ScreenFixture::generate(
        entry.kind,
        entry.pair_index,
        &entry.side,
        retina.config.width,
        retina.config.height,
    )?;
    if fixture.sha256 != entry.image_sha256 {
        return Err(format!("learning fixture {} hash mismatch", entry.id).into());
    }
    Ok(retina
        .encode(&fixture.image()?, neuron_count)?
        .input
        .into_iter()
        .map(|value| value as f32)
        .collect())
}

fn learning_schedule_for_held_out(
    criteria: &LearningEvaluationCriteria,
    entries: &[LearningScheduleEntry],
    width: usize,
    height: usize,
) -> Result<Vec<LearningScheduleEntry>, Box<dyn Error>> {
    let first = entries.first().ok_or("learning schedule is empty")?;
    let mut held_out = Vec::with_capacity(criteria.held_out_pair_count * 6);
    for pair_offset in 0..criteria.held_out_pair_count {
        let pair_index = criteria.pair_count + pair_offset;
        for (kind, kind_name) in [
            (ScreenFixtureKind::Same, "same"),
            (ScreenFixtureKind::Near, "near"),
            (ScreenFixtureKind::Unrelated, "unrelated"),
        ] {
            for side in ["a", "b"] {
                let fixture = ScreenFixture::generate(kind, pair_index, side, width, height)?;
                held_out.push(LearningScheduleEntry {
                    id: format!("heldout-{kind_name}-{pair_offset:02}-{side}"),
                    kind,
                    pair_index,
                    side: side.to_owned(),
                    image_sha256: fixture.sha256,
                    expected_change: kind != ScreenFixtureKind::Same,
                    timestamp_s: (criteria.presentation_count + held_out.len()) as f64,
                });
            }
        }
    }
    if first.pair_index != 0 {
        return Err("learning schedule does not start at pair zero".into());
    }
    Ok(held_out)
}

fn human_feedback_for_criteria(
    criteria: &LearningEvaluationCriteria,
    entries: &[LearningScheduleEntry],
) -> Result<BTreeMap<String, FeedbackEvent>, Box<dyn Error>> {
    let mut events = BTreeMap::new();
    for feedback in &criteria.human_feedback {
        let kind = match feedback.kind.as_str() {
            "reward" => FeedbackKind::Reward,
            "punish" => FeedbackKind::Punish,
            _ => return Err("unknown human feedback kind".into()),
        };
        let image_sha256 = image_sha256_for_input_id(&feedback.input_id, entries);
        events.insert(
            image_sha256.clone(),
            FeedbackEvent::new(
                feedback.event_id.clone(),
                image_sha256,
                kind,
                feedback.strength,
                0.0,
            )?,
        );
    }
    Ok(events)
}

fn reward_shuffled_feedback(
    criteria: &LearningEvaluationCriteria,
    entries: &[LearningScheduleEntry],
) -> Result<BTreeMap<String, FeedbackEvent>, Box<dyn Error>> {
    let mut result = BTreeMap::new();
    for (role, role_tag) in [("change", "change"), ("no_change", "no_change")] {
        let mut store = FeedbackStore::default();
        for entry in entries.iter().filter(|entry| entry.side == "b") {
            let changed = entry.expected_change;
            let kind = if (role == "change" && changed) || (role == "no_change" && !changed) {
                FeedbackKind::Reward
            } else {
                FeedbackKind::Punish
            };
            store.insert(FeedbackEvent::new(
                format!("canonical-{role_tag}-{}", entry.id),
                entry.image_sha256.clone(),
                kind,
                1.0,
                entry.pair_index as f64,
            )?)?;
        }
        let shuffled = shuffle_feedback(&store, criteria.seeds.reward_shuffle + role.len() as u64);
        for event in shuffled.events {
            result.insert(format!("{role_tag}:{}", event.input_id), event);
        }
    }
    Ok(result)
}

fn image_sha256_for_input_id(input_id: &str, entries: &[LearningScheduleEntry]) -> String {
    entries
        .iter()
        .find(|entry| entry.id == input_id)
        .map_or_else(|| input_id.to_owned(), |entry| entry.image_sha256.clone())
}

fn report_metric(report: &habitua::ModelReport, name: &str) -> Result<f64, Box<dyn Error>> {
    report
        .metrics
        .iter()
        .find(|metric| metric.name == name)
        .map(|metric| f64::from(metric.value))
        .ok_or_else(|| format!("rate brain metric {name} is missing").into())
}

fn brain_results(
    scores: &[BrainScores],
    criteria: &LearningEvaluationCriteria,
) -> Vec<LearningBrainResult> {
    brain_results_at_threshold(scores, criteria, 0.5)
}

fn brain_results_at_threshold(
    scores: &[BrainScores],
    criteria: &LearningEvaluationCriteria,
    threshold: f64,
) -> Vec<LearningBrainResult> {
    criteria
        .brain_roles
        .iter()
        .map(|brain| {
            let values = scores
                .iter()
                .filter(|score| score.readiness_sufficient)
                .map(|score| {
                    if brain.role == "change" {
                        score.brain_change_score
                    } else {
                        score.brain_no_change_score
                    }
                })
                .collect::<Vec<_>>();
            let labels = scores
                .iter()
                .filter(|score| score.readiness_sufficient)
                .map(|score| {
                    if brain.role == "change" {
                        score.expected_change
                    } else {
                        !score.expected_change
                    }
                })
                .collect::<Vec<_>>();
            let positive_count = labels.iter().filter(|label| **label).count();
            let negative_count = labels.len() - positive_count;
            let predicted_positive_count =
                values.iter().filter(|score| **score >= threshold).count();
            let (true_positive, false_positive, true_negative, false_negative) =
                confusion_matrix(&values, &labels, threshold);
            LearningBrainResult {
                brain_id: brain.id.clone(),
                role: brain.role.clone(),
                balanced_accuracy: balanced_accuracy_at_threshold(&values, &labels, threshold),
                auroc: auroc(&values, &labels),
                decision_threshold: threshold,
                sample_count: values.len(),
                positive_count,
                negative_count,
                score_min: values.iter().copied().fold(1.0, f64::min),
                score_max: values.iter().copied().fold(0.0, f64::max),
                predicted_positive_count,
                excluded_readiness_count: scores.len().saturating_sub(values.len()),
                true_positive,
                false_positive,
                true_negative,
                false_negative,
            }
        })
        .collect()
}

#[cfg(test)]
fn balanced_accuracy(scores: &[f64], labels: &[bool]) -> f64 {
    balanced_accuracy_at_threshold(scores, labels, 0.5)
}

fn balanced_accuracy_at_threshold(scores: &[f64], labels: &[bool], threshold: f64) -> f64 {
    let (true_positive, false_positive, true_negative, false_negative) =
        confusion_matrix(scores, labels, threshold);
    let positive = true_positive + false_negative;
    let negative = true_negative + false_positive;
    let sensitivity = true_positive as f64 / positive.max(1) as f64;
    let specificity = true_negative as f64 / negative.max(1) as f64;
    (sensitivity + specificity) * 0.5
}

fn sensitivity_specificity(scores: &[f64], labels: &[bool], threshold: f64) -> (f64, f64) {
    let (true_positive, false_positive, true_negative, false_negative) =
        confusion_matrix(scores, labels, threshold);
    let positive = true_positive + false_negative;
    let negative = true_negative + false_positive;
    (
        true_positive as f64 / positive.max(1) as f64,
        true_negative as f64 / negative.max(1) as f64,
    )
}

fn confusion_matrix(
    scores: &[f64],
    labels: &[bool],
    threshold: f64,
) -> (usize, usize, usize, usize) {
    let mut true_positive = 0;
    let mut false_positive = 0;
    let mut true_negative = 0;
    let mut false_negative = 0;
    for (score, label) in scores.iter().zip(labels) {
        let predicted_positive = *score >= threshold;
        match (*label, predicted_positive) {
            (true, true) => true_positive += 1,
            (false, true) => false_positive += 1,
            (false, false) => true_negative += 1,
            (true, false) => false_negative += 1,
        }
    }
    (true_positive, false_positive, true_negative, false_negative)
}

fn auroc(scores: &[f64], labels: &[bool]) -> f64 {
    let positives = labels.iter().filter(|label| **label).count();
    let negatives = labels.len() - positives;
    if positives == 0 || negatives == 0 {
        return 0.5;
    }
    let mut concordant = 0.0;
    for (score, label) in scores.iter().zip(labels) {
        if !*label {
            continue;
        }
        for (other_score, other_label) in scores.iter().zip(labels) {
            if *other_label {
                continue;
            }
            concordant += if score > other_score {
                1.0
            } else if score == other_score {
                0.5
            } else {
                0.0
            };
        }
    }
    concordant / (positives * negatives) as f64
}

fn consensus_result(scores: &[BrainScores], margin: f64) -> LearningConsensusResult {
    consensus_result_at_threshold(scores, 0.5, margin)
}

fn consensus_result_at_threshold(
    scores: &[BrainScores],
    threshold: f64,
    margin: f64,
) -> LearningConsensusResult {
    let mut correct = 0;
    let mut novel_count = 0;
    let mut habituated_count = 0;
    let mut hold_count = 0;
    let mut error_count = 0;
    let mut scored_sample_count = 0;
    let mut readiness_insufficient_count = 0;
    for score in scores {
        if !score.readiness_sufficient {
            hold_count += 1;
            readiness_insufficient_count += 1;
            continue;
        }
        scored_sample_count += 1;
        let novel = score.change_score >= threshold + margin
            && score.brain_no_change_score <= threshold - margin;
        let habituated = score.change_score <= threshold - margin
            && score.brain_no_change_score >= threshold + margin;
        if novel {
            novel_count += 1;
            if score.expected_change {
                correct += 1;
            } else {
                error_count += 1;
            }
        } else if habituated {
            habituated_count += 1;
            if !score.expected_change {
                correct += 1;
            } else {
                error_count += 1;
            }
        } else {
            hold_count += 1;
        }
    }
    LearningConsensusResult {
        accuracy: correct as f64 / scored_sample_count.max(1) as f64,
        error_rate: error_count as f64 / scored_sample_count.max(1) as f64,
        hold_rate: hold_count as f64 / scores.len().max(1) as f64,
        sample_count: scores.len(),
        novel_count,
        habituated_count,
        hold_count,
        scored_sample_count,
        readiness_insufficient_count,
    }
}

fn read_context(path: &Path) -> Result<EvaluationContext, Box<dyn Error>> {
    let manifest_bytes = fs::read(path)?;
    let manifest: RateEvaluationManifest = serde_json::from_slice(&manifest_bytes)?;
    if manifest.schema_version != RATE_EVALUATION_SCHEMA_VERSION {
        return Err("unsupported rate evaluation manifest schema version".into());
    }
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let criteria_path = resolve_path(base, &manifest.criteria_file);
    let criteria_bytes = fs::read(&criteria_path)?;
    let criteria: RateEvaluationCriteria = serde_json::from_slice(&criteria_bytes)?;
    validate_criteria(&criteria)?;
    if sha256_bytes(&criteria_bytes) != manifest.criteria_sha256 {
        return Err("rate evaluation criteria hash mismatch".into());
    }
    if manifest.retina_input_types != criteria.input_types
        || manifest.retina_coordinate_rule != criteria.retina.coordinate_rule
        || manifest.retina_unknown_side_policy != criteria.retina.unknown_side_policy
    {
        return Err("rate evaluation RetinaMap contract mismatch".into());
    }
    let fixtures_path = resolve_path(base, &manifest.fixtures_file);
    let fixtures_bytes = fs::read(&fixtures_path)?;
    let fixtures: FixtureManifest = serde_json::from_slice(&fixtures_bytes)?;
    if sha256_bytes(&fixtures_bytes) != manifest.fixtures_sha256 {
        return Err("rate evaluation fixture manifest hash mismatch".into());
    }
    let pack_manifest = Path::new(&manifest.canonical_pack).join("rate_manifest.json");
    if sha256_bytes(&fs::read(pack_manifest)?) != manifest.canonical_pack_manifest_sha256 {
        return Err("rate evaluation pack manifest hash mismatch".into());
    }
    let schedule = canonical_schedule(&criteria, &fixtures)?;
    if schedule != manifest.seed_schedule_canonical_bytes.as_bytes()
        || sha256_bytes(&schedule) != manifest.seed_schedule_sha256
    {
        return Err("rate evaluation seed schedule mismatch".into());
    }
    Ok(EvaluationContext {
        manifest,
        manifest_sha256: sha256_bytes(&manifest_bytes),
        criteria,
        fixtures,
    })
}

fn input_audit_passed(context: &EvaluationContext) -> Result<bool, Box<dyn Error>> {
    let mut same = 0;
    let mut near_min = f64::INFINITY;
    let mut unrelated_min = f64::INFINITY;
    for pair_index in 0..context.fixtures.pair_count {
        for (kind, name) in [
            (ScreenFixtureKind::Same, "same"),
            (ScreenFixtureKind::Near, "near"),
            (ScreenFixtureKind::Unrelated, "unrelated"),
        ] {
            let left = fixture_for(&context.fixtures, kind, pair_index, "a", name)?.image()?;
            let right = fixture_for(&context.fixtures, kind, pair_index, "b", name)?.image()?;
            let (l2, _, direction) = image_feature_metrics(&left.pixels, &right.pixels);
            if kind == ScreenFixtureKind::Same && l2 <= context.criteria.audit.same_l2_max {
                same += 1;
            }
            match kind {
                ScreenFixtureKind::Near => near_min = near_min.min(direction),
                ScreenFixtureKind::Unrelated => unrelated_min = unrelated_min.min(direction),
                ScreenFixtureKind::Same => {}
            }
        }
    }
    Ok(input_audit_decision(
        &context.criteria.audit,
        same,
        near_min,
        unrelated_min,
        true,
    ))
}

fn input_audit_decision(
    criteria: &RateAuditCriteria,
    exact_duplicate_pair_count: usize,
    near_min: f64,
    unrelated_min: f64,
    retina_contract_passed: bool,
) -> bool {
    exact_duplicate_pair_count == criteria.expected_same_pair_count
        && near_min >= criteria.near_direction_min
        && unrelated_min >= criteria.unrelated_direction_min
        && unrelated_min >= near_min + criteria.ordering_margin
        && retina_contract_passed
}

fn retina_config(criteria: &RateEvaluationCriteria) -> RetinaMapConfig {
    RetinaMapConfig {
        width: criteria.width,
        height: criteria.height,
        input_types: criteria.input_types.clone(),
        unknown_side_policy: criteria.retina.unknown_side_policy.clone(),
        coordinate_rule: criteria.retina.coordinate_rule.clone(),
        ..RetinaMapConfig::default()
    }
}

fn retina_contract_passed(
    audit: &habitua_connectome::RetinaAudit,
    criteria: &RateRetinaCriteria,
) -> bool {
    let spread_p95 = percentile(&audit.inferred_coordinate_spread, 0.95);
    audit.coordinate_rule == criteria.coordinate_rule
        && audit.unknown_side_policy == criteria.unknown_side_policy
        && audit.inferred_coordinate_count <= criteria.maximum_inferred_coordinate_count
        && spread_p95 <= criteria.maximum_inferred_spread_p95
        && audit
            .inferred_coordinate_spread
            .iter()
            .all(|spread| spread.is_finite() && *spread <= criteria.maximum_inferred_spread)
        && audit
            .inferred_majority_ratio
            .iter()
            .all(|ratio| ratio.is_finite() && (0.0..=1.0).contains(ratio))
}

fn frozen_report(
    context: &EvaluationContext,
    responses: &BTreeMap<String, ResponseSnapshot>,
    observations: &[RateObservationReport],
) -> RateFrozenReport {
    let mut pair_reports = Vec::new();
    let mut same_raw = Vec::new();
    let mut near_raw = Vec::new();
    let mut unrelated_raw = Vec::new();
    let mut same_delta = Vec::new();
    let mut near_delta = Vec::new();
    let mut unrelated_delta = Vec::new();
    let mut same_scaled_delta = Vec::new();
    let mut near_scaled_delta = Vec::new();
    let mut unrelated_scaled_delta = Vec::new();
    let mut all_response_valid = true;
    let mut all_activity_ok = true;
    let mut all_saturation_ok = true;
    for pair_index in 0..context.fixtures.pair_count {
        for (kind, name) in [
            (ScreenFixtureKind::Same, "same"),
            (ScreenFixtureKind::Near, "near"),
            (ScreenFixtureKind::Unrelated, "unrelated"),
        ] {
            let left_id = format!("{name}-{pair_index:02}-a");
            let right_id = format!("{name}-{pair_index:02}-b");
            let left = &responses[&left_id];
            let right = &responses[&right_id];
            let (raw_l2, raw_cosine, _) = feature_metrics(&left.raw, &right.raw);
            let (delta_l2, delta_cosine, _) = feature_metrics(&left.delta, &right.delta);
            let (scaled_delta_l2, scaled_delta_cosine, _) =
                feature_metrics(&left.scaled_delta, &right.scaled_delta);
            let norm_ok = observations
                .iter()
                .filter(|observation| observation.id == left_id || observation.id == right_id)
                .all(|observation| {
                    observation.scaled_delta_response_norm
                        > context.criteria.frozen.zero_response_epsilon
                        && !observation.components.is_empty()
                        && observation
                            .components
                            .iter()
                            .all(|component| !component.zero_response)
                });
            let active_ok = observations
                .iter()
                .filter(|observation| observation.id == left_id || observation.id == right_id)
                .all(|observation| {
                    observation.components.iter().all(|component| {
                        component.active_fraction
                            >= context.criteria.frozen.minimum_active_readout_fraction
                    })
                });
            let saturation_ok = observations
                .iter()
                .filter(|observation| observation.id == left_id || observation.id == right_id)
                .all(|observation| {
                    observation.components.iter().all(|component| {
                        component.saturation_fraction
                            <= context.criteria.frozen.maximum_saturation_fraction
                    })
                });
            all_response_valid &= norm_ok;
            all_activity_ok &= active_ok;
            all_saturation_ok &= saturation_ok;
            match kind {
                ScreenFixtureKind::Same => {
                    same_raw.push(raw_cosine);
                    same_delta.push(delta_cosine);
                    same_scaled_delta.push(scaled_delta_cosine);
                }
                ScreenFixtureKind::Near => {
                    near_raw.push(raw_cosine);
                    near_delta.push(delta_cosine);
                    near_scaled_delta.push(scaled_delta_cosine);
                }
                ScreenFixtureKind::Unrelated => {
                    unrelated_raw.push(raw_cosine);
                    unrelated_delta.push(delta_cosine);
                    unrelated_scaled_delta.push(scaled_delta_cosine);
                }
            }
            pair_reports.push(RateFrozenPairReport {
                id: format!("{name}-{pair_index:02}"),
                kind,
                raw_cosine_similarity: raw_cosine,
                raw_l2_distance: raw_l2,
                delta_cosine_similarity: delta_cosine,
                delta_l2_distance: delta_l2,
                scaled_delta_cosine_similarity: scaled_delta_cosine,
                scaled_delta_l2_distance: scaled_delta_l2,
                cosine_similarity: scaled_delta_cosine,
                l2_distance: scaled_delta_l2,
            });
        }
    }
    let raw_aa = percentile_option(&same_raw, 0.10);
    let raw_near = percentile_option(&near_raw, 0.50);
    let raw_ab = percentile_option(&unrelated_raw, 0.90);
    let delta_aa = percentile_option(&same_delta, 0.10);
    let delta_near = percentile_option(&near_delta, 0.50);
    let delta_ab = percentile_option(&unrelated_delta, 0.90);
    let scaled_delta_aa = percentile_option(&same_scaled_delta, 0.10);
    let scaled_delta_near = percentile_option(&near_scaled_delta, 0.50);
    let scaled_delta_ab = percentile_option(&unrelated_scaled_delta, 0.90);
    let passed = frozen_decision(
        all_response_valid,
        all_activity_ok,
        all_saturation_ok,
        scaled_delta_aa,
        scaled_delta_ab,
        context.criteria.frozen.minimum_separation_margin,
    );
    RateFrozenReport {
        pair_count: context.fixtures.pair_count * 3,
        simulation_count: context.fixtures.records.len(),
        aa_p10_cosine: scaled_delta_aa,
        near_p50_cosine: scaled_delta_near,
        ab_p90_cosine: scaled_delta_ab,
        raw_aa_p10_cosine: raw_aa,
        raw_near_p50_cosine: raw_near,
        raw_ab_p90_cosine: raw_ab,
        delta_aa_p10_cosine: delta_aa,
        delta_near_p50_cosine: delta_near,
        delta_ab_p90_cosine: delta_ab,
        scaled_delta_aa_p10_cosine: scaled_delta_aa,
        scaled_delta_near_p50_cosine: scaled_delta_near,
        scaled_delta_ab_p90_cosine: scaled_delta_ab,
        all_response_valid,
        all_activity_ok,
        all_saturation_ok,
        passed,
        pairs: pair_reports,
    }
}

fn frozen_decision(
    all_response_valid: bool,
    all_activity_ok: bool,
    all_saturation_ok: bool,
    aa_p10: Option<f64>,
    ab_p90: Option<f64>,
    minimum_separation_margin: f64,
) -> bool {
    all_response_valid
        && all_activity_ok
        && all_saturation_ok
        && aa_p10
            .zip(ab_p90)
            .is_some_and(|(aa, ab)| aa >= ab + minimum_separation_margin)
}

fn build_response_groups(
    graph: &RateGraph,
    rest_activity: &[f64],
    criteria: &RateFrozenCriteria,
) -> Result<Vec<ResponseGroup>, Box<dyn Error>> {
    let mut groups = Vec::with_capacity(criteria.response_groups.len());
    for group in &criteria.response_groups {
        let mut population_names = graph
            .populations
            .keys()
            .filter(|population_name| !population_name.contains('/') || population_name.is_empty())
            .filter(|population_name| {
                group
                    .prefixes
                    .iter()
                    .any(|prefix| population_name.starts_with(prefix))
            })
            .filter(|population_name| {
                !group
                    .excluded_population_names
                    .iter()
                    .any(|excluded| excluded == *population_name)
            })
            .cloned()
            .collect::<Vec<_>>();
        population_names.sort();
        let mut indices = population_names
            .iter()
            .flat_map(|name| graph.populations.get(name).into_iter().flatten().copied())
            .collect::<Vec<_>>();
        indices.sort_unstable();
        indices.dedup();
        if indices.is_empty() {
            return Err(format!("response group {} has no populations", group.name).into());
        }
        let rest_l2_norm = indices
            .iter()
            .map(|index| rest_activity[*index as usize] * rest_activity[*index as usize])
            .sum::<f64>()
            .sqrt();
        let scale = rest_l2_norm.max(criteria.component_scale_floor);
        groups.push(ResponseGroup {
            report: RateResponseGroupReport {
                name: group.name.clone(),
                population_names,
                neuron_count: indices.len(),
                rest_l2_norm,
                scale,
            },
            indices,
        });
    }
    if groups.is_empty() {
        return Err("response groups must not be empty".into());
    }
    Ok(groups)
}

fn response_snapshot(
    activity: &[f64],
    rest_activity: &[f64],
    groups: &[ResponseGroup],
    hmax: f64,
    zero_response_epsilon: f64,
) -> ResponseSnapshot {
    let mut raw = Vec::new();
    let mut delta = Vec::new();
    let mut scaled_delta = Vec::new();
    let mut components = Vec::with_capacity(groups.len());
    let mut active_count = 0;
    let mut saturation_count = 0;
    let mut selected_count = 0;
    for group in groups {
        let mut group_raw = Vec::with_capacity(group.indices.len());
        let mut group_delta = Vec::with_capacity(group.indices.len());
        let mut group_scaled_delta = Vec::with_capacity(group.indices.len());
        for index in &group.indices {
            let value = activity[*index as usize];
            let difference = value - rest_activity[*index as usize];
            raw.push(value);
            delta.push(difference);
            scaled_delta.push(difference / group.report.scale);
            group_raw.push(value);
            group_delta.push(difference);
            group_scaled_delta.push(difference / group.report.scale);
            active_count += usize::from(difference.abs() > zero_response_epsilon);
            saturation_count += usize::from(
                value <= zero_response_epsilon || value >= hmax - zero_response_epsilon,
            );
            selected_count += 1;
        }
        components.push(RateResponseComponentReport {
            name: group.report.name.clone(),
            raw_norm: l2_norm(&group_raw),
            delta_norm: l2_norm(&group_delta),
            scaled_delta_norm: l2_norm(&group_scaled_delta),
            active_fraction: group_delta
                .iter()
                .filter(|value| value.abs() > zero_response_epsilon)
                .count() as f64
                / group_delta.len().max(1) as f64,
            saturation_fraction: group
                .indices
                .iter()
                .filter(|index| {
                    let value = activity[**index as usize];
                    value <= zero_response_epsilon || value >= hmax - zero_response_epsilon
                })
                .count() as f64
                / group.indices.len().max(1) as f64,
            zero_response: l2_norm(&group_scaled_delta) <= zero_response_epsilon,
        });
    }
    ResponseSnapshot {
        raw_norm: l2_norm(&raw),
        delta_norm: l2_norm(&delta),
        scaled_delta_norm: l2_norm(&scaled_delta),
        active_fraction: active_count as f64 / selected_count.max(1) as f64,
        saturation_fraction: saturation_count as f64 / selected_count.max(1) as f64,
        raw,
        delta,
        scaled_delta,
        components,
    }
}

fn fixture_input(
    context: &EvaluationContext,
    retina: &RetinaMap,
    graph: &RateGraph,
    input_id: &str,
) -> Result<Vec<f64>, Box<dyn Error>> {
    let record = context
        .fixtures
        .records
        .iter()
        .find(|record| record.id == input_id)
        .ok_or_else(|| format!("fixture {input_id} is missing"))?;
    let fixture = fixture_from_record(record)?;
    Ok(retina
        .encode(&fixture.image()?, graph.neuron_count())?
        .input)
}

fn fixture_for(
    fixtures: &FixtureManifest,
    kind: ScreenFixtureKind,
    pair_index: usize,
    side: &str,
    name: &str,
) -> Result<ScreenFixture, Box<dyn Error>> {
    let record = fixtures
        .records
        .iter()
        .find(|record| {
            record.kind == kind && record.pair_index == pair_index && record.side == side
        })
        .ok_or_else(|| format!("fixture {name}-{pair_index:02}-{side} is missing"))?;
    fixture_from_record(record)
}

fn fixture_from_record(record: &FixtureRecord) -> Result<ScreenFixture, Box<dyn Error>> {
    let fixture = ScreenFixture::generate(
        record.kind,
        record.pair_index,
        &record.side,
        record.width,
        record.height,
    )?;
    if fixture.sha256 != record.sha256 {
        return Err(format!("fixture {} hash mismatch", record.id).into());
    }
    Ok(fixture)
}

fn feature_metrics(left: &[f64], right: &[f64]) -> (f64, f64, f64) {
    let squared = left
        .iter()
        .zip(right)
        .map(|(left, right)| (left - right) * (left - right))
        .sum::<f64>();
    let left_norm = l2_norm(left);
    let right_norm = l2_norm(right);
    let dot = left
        .iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum::<f64>();
    let cosine = if left_norm > 0.0 && right_norm > 0.0 {
        (dot / (left_norm * right_norm)).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    (squared.sqrt(), cosine, 1.0 - cosine)
}

fn image_feature_metrics(left: &[[f64; 3]], right: &[[f64; 3]]) -> (f64, f64, f64) {
    let left = left
        .iter()
        .flat_map(|pixel| pixel.iter().copied())
        .collect::<Vec<_>>();
    let right = right
        .iter()
        .flat_map(|pixel| pixel.iter().copied())
        .collect::<Vec<_>>();
    feature_metrics(&left, &right)
}

fn l2_norm(values: &[f64]) -> f64 {
    values.iter().map(|value| value * value).sum::<f64>().sqrt()
}

fn l1_norm(values: &[f64]) -> f64 {
    values.iter().map(|value| value.abs()).sum()
}

fn stage_report(
    name: &str,
    samples: Vec<f64>,
    peak_rss_bytes: Option<u64>,
) -> RateStagePerformanceReport {
    RateStagePerformanceReport {
        sample_count: samples.len(),
        p50_ms: percentile(&samples, 0.50),
        p95_ms: percentile(&samples, 0.95),
        peak_rss_bytes,
        name: name.to_owned(),
    }
}

#[cfg(target_os = "macos")]
fn process_peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage writes exactly one libc::rusage value into the valid pointer.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    (result == 0).then(|| unsafe { usage.assume_init().ru_maxrss as u64 })
}

#[cfg(target_os = "linux")]
fn process_peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage writes exactly one libc::rusage value into the valid pointer.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    (result == 0).then(|| unsafe { (usage.assume_init().ru_maxrss as u64).saturating_mul(1024) })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_peak_rss_bytes() -> Option<u64> {
    None
}

fn relative_difference(left: f64, right: f64) -> f64 {
    (left - right).abs() / left.abs().max(right.abs()).max(1.0e-12)
}

fn percentile(values: &[f64], fraction: f64) -> f64 {
    percentile_option(values, fraction).unwrap_or(0.0)
}

fn percentile_option(values: &[f64], fraction: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    let position = fraction.clamp(0.0, 1.0) * (values.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    Some(values[lower] + (values[upper] - values[lower]) * (position - lower as f64))
}

fn canonical_schedule(
    criteria: &RateEvaluationCriteria,
    fixtures: &FixtureManifest,
) -> Result<Vec<u8>, Box<dyn Error>> {
    #[derive(Serialize)]
    struct Entry<'a> {
        id: &'a str,
        kind: ScreenFixtureKind,
        pair_index: usize,
        side: &'a str,
        image_sha256: &'a str,
        input_types: &'a [String],
        retina_coordinate_rule: &'a str,
        retina_unknown_side_policy: &'a str,
        readout_population: &'a str,
        response_definition: &'a str,
        response_groups: &'a [RateResponseGroupCriteria],
        seed: u64,
    }
    let mut bytes = Vec::new();
    for (index, record) in fixtures.records.iter().enumerate() {
        let entry = Entry {
            id: &record.id,
            kind: record.kind,
            pair_index: record.pair_index,
            side: &record.side,
            image_sha256: &record.sha256,
            input_types: &criteria.input_types,
            retina_coordinate_rule: &criteria.retina.coordinate_rule,
            retina_unknown_side_policy: &criteria.retina.unknown_side_policy,
            readout_population: &criteria.readout_population,
            response_definition: &criteria.frozen.response_definition,
            response_groups: &criteria.frozen.response_groups,
            seed: criteria.simulation_seed_base + index as u64,
        };
        bytes.extend(serde_json::to_vec(&entry)?);
        bytes.push(b'\n');
    }
    Ok(bytes)
}

fn validate_criteria(criteria: &RateEvaluationCriteria) -> Result<(), Box<dyn Error>> {
    if criteria.schema_version != RATE_EVALUATION_SCHEMA_VERSION
        || criteria.width == 0
        || criteria.height == 0
        || criteria.pair_count != 12
        || criteria.input_types.is_empty()
        || criteria.readout_population.is_empty()
        || criteria.frozen.steps == 0
        || criteria.frozen.response_definition != "h_input_minus_h_rest"
        || criteria.frozen.component_scaling != "per_group_rest_l2_with_floor"
        || !criteria.frozen.component_scale_floor.is_finite()
        || criteria.frozen.component_scale_floor <= 0.0
        || !criteria.frozen.zero_response_epsilon.is_finite()
        || criteria.frozen.zero_response_epsilon <= 0.0
        || !criteria.frozen.maximum_saturation_fraction.is_finite()
        || !(0.0..=1.0).contains(&criteria.frozen.maximum_saturation_fraction)
        || criteria.frozen.response_groups.is_empty()
        || !criteria.numerical.f32_reference_abs_tolerance.is_finite()
        || criteria.numerical.f32_reference_abs_tolerance <= 0.0
        || !criteria
            .numerical
            .finite_difference_abs_tolerance
            .is_finite()
        || criteria.numerical.finite_difference_abs_tolerance <= 0.0
        || !criteria.numerical.nonlinear_boundary_margin.is_finite()
        || criteria.numerical.nonlinear_boundary_margin <= 0.0
        || criteria.retina.coordinate_rule != "assigned_ol_hex_then_weighted_outgoing_target"
        || criteria.retina.unknown_side_policy != "retain"
        || criteria.retina.maximum_inferred_coordinate_count == 0
        || !criteria.retina.maximum_inferred_spread_p95.is_finite()
        || criteria.retina.maximum_inferred_spread_p95 <= 0.0
        || !criteria.retina.maximum_inferred_spread.is_finite()
        || criteria.retina.maximum_inferred_spread <= 0.0
        || criteria.training.projection_dimension == 0
    {
        return Err("rate evaluation criteria is invalid or not canonical".into());
    }
    let mut group_names = criteria
        .frozen
        .response_groups
        .iter()
        .map(|group| group.name.as_str())
        .collect::<Vec<_>>();
    group_names.sort_unstable();
    if group_names.windows(2).any(|pair| pair[0] == pair[1])
        || criteria
            .frozen
            .response_groups
            .iter()
            .any(|group| group.name.is_empty() || group.prefixes.is_empty())
    {
        return Err("rate response group criteria is invalid".into());
    }
    Ok(())
}

fn parse_values(
    mut arguments: impl Iterator<Item = String>,
) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let mut values = BTreeMap::new();
    while let Some(flag) = arguments.next() {
        if !flag.starts_with("--") {
            return Err(format!("unexpected argument {flag}").into());
        }
        let value = arguments
            .next()
            .ok_or_else(|| format!("value is required for {flag}"))?;
        if value.starts_with('-') || values.insert(flag.clone(), value).is_some() {
            return Err(format!("invalid or duplicated argument {flag}").into());
        }
    }
    Ok(values)
}

fn required_path(values: &BTreeMap<String, String>, name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let value = values
        .get(name)
        .ok_or_else(|| format!("required argument {name} is missing"))?;
    if value.is_empty() {
        return Err(format!("argument {name} must not be empty").into());
    }
    Ok(PathBuf::from(value))
}

fn reject_unknown(
    values: &BTreeMap<String, String>,
    allowed: &[&str],
) -> Result<(), Box<dyn Error>> {
    if let Some(name) = values.keys().find(|name| !allowed.contains(&name.as_str())) {
        return Err(format!("unknown argument {name}").into());
    }
    Ok(())
}

fn resolve_path(base: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        let relative_to_manifest = base.join(&path);
        if relative_to_manifest.exists() {
            relative_to_manifest
        } else {
            path
        }
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audit_criteria() -> RateAuditCriteria {
        RateAuditCriteria {
            same_l2_max: 0.1,
            near_direction_min: 0.2,
            unrelated_direction_min: 0.5,
            ordering_margin: 0.1,
            expected_same_pair_count: 12,
        }
    }

    #[test]
    fn input_audit_decision_requires_all_fixed_conditions() {
        let criteria = audit_criteria();
        assert!(input_audit_decision(&criteria, 12, 0.2, 0.5, true));
        assert!(!input_audit_decision(&criteria, 11, 0.2, 0.5, true));
        assert!(!input_audit_decision(&criteria, 12, 0.19, 0.5, true));
        assert!(!input_audit_decision(&criteria, 12, 0.2, 0.5, false));
        assert!(!input_audit_decision(&criteria, 12, 0.45, 0.5, true));
    }

    #[test]
    fn frozen_decision_requires_nonzero_active_unsaturated_separation() {
        assert!(frozen_decision(true, true, true, Some(0.9), Some(0.7), 0.1));
        assert!(!frozen_decision(
            false,
            true,
            true,
            Some(0.9),
            Some(0.7),
            0.1
        ));
        assert!(!frozen_decision(
            true,
            false,
            true,
            Some(0.9),
            Some(0.7),
            0.1
        ));
        assert!(!frozen_decision(
            true,
            true,
            false,
            Some(0.9),
            Some(0.7),
            0.1
        ));
        assert!(!frozen_decision(true, true, true, None, Some(0.7), 0.1));
        assert!(!frozen_decision(
            true,
            true,
            true,
            Some(0.7),
            Some(0.7),
            0.1
        ));
    }

    #[test]
    fn learning_metrics_are_deterministic_and_consensus_requires_two_sided_evidence() {
        let scores = vec![
            BrainScores {
                expected_change: false,
                change_score: 0.1,
                brain_change_score: 0.1,
                brain_no_change_score: 0.9,
                readiness_sufficient: true,
            },
            BrainScores {
                expected_change: true,
                change_score: 0.9,
                brain_change_score: 0.9,
                brain_no_change_score: 0.1,
                readiness_sufficient: true,
            },
        ];
        let labels = [false, true];
        assert_eq!(balanced_accuracy(&[0.1, 0.9], &labels), 1.0);
        assert_eq!(auroc(&[0.1, 0.9], &labels), 1.0);
        let consensus = consensus_result(&scores, 0.1);
        assert_eq!(consensus.accuracy, 1.0);
        assert_eq!(consensus.novel_count, 1);
        assert_eq!(consensus.habituated_count, 1);
        assert_eq!(consensus.hold_count, 0);
    }

    #[test]
    fn consensus_uses_the_no_change_probability_without_complementing_it() {
        assert_eq!(consensus_action(0.9, 0.1, 0.1), "notify_change");
        assert_eq!(consensus_action(0.1, 0.9, 0.1), "suppress_no_change");
        assert_eq!(consensus_action(0.9, 0.9, 0.1), "hold");
        assert_eq!(consensus_action(0.1, 0.1, 0.1), "hold");
        assert_eq!(
            consensus_action_with_readiness(0.9, 0.1, 0.5, 0.1, false),
            "hold"
        );
    }

    #[test]
    fn decision_threshold_is_selected_from_tuning_scores() {
        let entry = |id: &str, changed: bool| BatchSample {
            entry: LearningScheduleEntry {
                id: id.to_owned(),
                kind: ScreenFixtureKind::Near,
                pair_index: 0,
                side: "a".to_owned(),
                image_sha256: id.to_owned(),
                expected_change: changed,
                timestamp_s: 0.0,
            },
            augmented_features: vec![0.0],
            distance: Some(if changed { 1.0 } else { 0.0 }),
            previous_absolute_difference: Some(0.0),
            automatic_change: Some(changed),
        };
        let samples = [
            entry("same-0", false),
            entry("same-1", false),
            entry("change-0", true),
            entry("change-1", true),
        ];
        let scores = BTreeMap::from([
            ("change".to_owned(), vec![0.1, 0.4, 0.45, 0.6]),
            ("no_change".to_owned(), vec![0.9, 0.6, 0.4, 0.1]),
        ]);
        let threshold = select_decision_threshold(&samples, &scores, 0.5);
        assert!(threshold < 0.5);
        assert!(threshold > 0.4);
    }

    #[test]
    fn batch_standardization_is_fitted_from_training_samples_only() {
        let entry = |id: &str, expected_change: bool| LearningScheduleEntry {
            id: id.to_owned(),
            kind: if expected_change {
                ScreenFixtureKind::Near
            } else {
                ScreenFixtureKind::Same
            },
            pair_index: 0,
            side: "a".to_owned(),
            image_sha256: id.to_owned(),
            expected_change,
            timestamp_s: 0.0,
        };
        let samples = [
            BatchSample {
                entry: entry("a", false),
                augmented_features: vec![1.0, 2.0],
                distance: Some(0.0),
                previous_absolute_difference: Some(0.0),
                automatic_change: Some(false),
            },
            BatchSample {
                entry: entry("b", true),
                augmented_features: vec![3.0, 2.0],
                distance: Some(1.0),
                previous_absolute_difference: Some(1.0),
                automatic_change: Some(true),
            },
        ];
        let standardization = fit_feature_standardization(&samples).expect("standardization");
        assert_eq!(standardization.mean, vec![2.0, 2.0]);
        assert_eq!(standardization.zero_variance_count, 1);
        assert!((standardization.scale[0] - 1.0).abs() < 1.0e-12);
        assert_eq!(standardization.scale[1], 1.0);
    }

    #[test]
    fn batch_and_runtime_history_features_share_the_same_84d_series() {
        let history = (0..5)
            .map(|timestamp_s| FrozenReadoutHistoryEntry {
                features: vec![timestamp_s as f32 / 5.0; 80],
                timestamp_s: timestamp_s as f64,
            })
            .collect::<Vec<_>>();
        let batch = frozen_readout_augmented_features_with_expiry(
            &[1.0; 80], &history, 5.0, 4, 0.01, 1.0, 3_600.0, 300.0,
        );
        let runtime_timestamp = Timestamp::from_millis(5_000).as_duration().as_secs_f64();
        let runtime = frozen_readout_augmented_features_with_expiry(
            &[1.0; 80],
            &history,
            runtime_timestamp,
            4,
            0.01,
            1.0,
            3_600.0,
            300.0,
        );
        assert_eq!(batch.0, runtime.0);
        assert_eq!(batch.1, runtime.1);
        assert_eq!(batch.1.len(), 84);
    }

    #[test]
    fn batch_logistic_fit_is_non_constant_on_separable_features() {
        let features = vec![vec![-2.0], vec![-1.0], vec![1.0], vec![2.0]];
        let labels = vec![false, false, true, true];
        let (weights, bias) = fit_batch_parameters(&features, &labels, 0.1, 0.001, 200);
        let scores = features
            .iter()
            .map(|features| predict_batch_parameters(&weights, bias, features))
            .collect::<Vec<_>>();
        assert!(scores[0] < 0.5);
        assert!(scores[3] > 0.5);
        assert!((scores[3] - scores[0]).abs() > 1.0e-9);
    }

    #[test]
    fn response_summary_contains_fixed_distribution_projection() {
        let group = ResponseGroup {
            report: RateResponseGroupReport {
                name: "test".to_owned(),
                population_names: vec!["test".to_owned()],
                neuron_count: 8,
                rest_l2_norm: 1.0,
                scale: 1.0,
            },
            indices: (0..8).collect(),
        };
        let response = cached_response_summary(
            &(0..8).map(|value| value as f64).collect::<Vec<_>>(),
            &[0.0; 8],
            &[group],
            10.0,
            1.0e-12,
            4,
        );
        assert_eq!(response.features.len(), 8);
        assert_ne!(response.features[4], response.features[7]);
    }

    #[test]
    fn teacher_rule_report_checks_threshold_reproduction_separately_from_fixture_labels() {
        let entry = |id: &str, expected_change: bool| LearningScheduleEntry {
            id: id.to_owned(),
            kind: ScreenFixtureKind::Near,
            pair_index: 0,
            side: "a".to_owned(),
            image_sha256: id.to_owned(),
            expected_change,
            timestamp_s: 0.0,
        };
        let samples = vec![
            BatchSample {
                entry: entry("same-history", false),
                augmented_features: vec![0.0, 0.1],
                distance: Some(0.1),
                previous_absolute_difference: Some(0.1),
                automatic_change: Some(false),
            },
            BatchSample {
                entry: entry("changed-history", true),
                augmented_features: vec![0.0, 0.9],
                distance: Some(0.9),
                previous_absolute_difference: Some(0.9),
                automatic_change: Some(true),
            },
        ];
        let report = teacher_rule_report(&[&samples, &[], &[], &[]], 0.5, 0);
        assert_eq!(report.training_match_count, 2);
        assert_eq!(report.training_fixture_label_match_count, 2);
        assert!(report.all_match);
    }

    #[test]
    fn case_memory_threshold_is_calibrated_from_screen_units() {
        let observation = |id: &str, app: &str, title: &str, value: f64| RealScreenObservation {
            record: RealScreenRecord {
                id: id.to_owned(),
                path: String::new(),
                captured_at_s: 0.0,
                stream_id: "test".to_owned(),
                split: "evaluation".to_owned(),
                app: app.to_owned(),
                title: title.to_owned(),
                independent_label: "no_change".to_owned(),
                label_reason: String::new(),
                width: 1,
                height: 1,
                sha256: id.to_owned(),
            },
            independent_label: Some(false),
            current_features: vec![value, 0.0],
            response_features: vec![value, 0.0],
            shuffled_response_features: vec![value, 0.0],
            history_features: Vec::new(),
            retina_features: Vec::new(),
            distance: Some(value),
        };
        let calibration = real_screen_case_memory_calibration(
            &[
                observation("same-a", "app", "document", 0.0),
                observation("same-a", "app", "document", 0.1),
                observation("other", "other-app", "document", 10.0),
            ],
            4.0,
            &[
                observation("same-a", "app", "document", 0.0),
                observation("same-a", "app", "document", 0.1),
                observation("other", "other-app", "document", 10.0),
            ],
        );
        assert_eq!(calibration.same_screen_pair_count, 1);
        assert_eq!(calibration.different_screen_pair_count, 2);
        assert_eq!(calibration.selected_threshold, Some(4.0));
        assert!(calibration.exact_image_hash_primary_key);
        assert!(calibration.runtime_boundary_verified);
        assert!(!calibration.neighbors_are_same_screen_only);
    }

    #[test]
    fn hold_is_reported_as_an_error_in_independent_accuracy() {
        let known = IndependentFeatureSample {
            split: "evaluation".to_owned(),
            screen_unit: "app\u{1f}document".to_owned(),
            record_id: "record-known".to_owned(),
            image_sha256: "hash-known".to_owned(),
            label: Some(false),
            features: vec![0.0],
        };
        let hold = IndependentFeatureSample {
            split: "evaluation".to_owned(),
            screen_unit: "app\u{1f}hold".to_owned(),
            record_id: "record-hold".to_owned(),
            image_sha256: "hash-hold".to_owned(),
            label: None,
            features: vec![0.0],
        };
        let samples = vec![&known, &hold];
        assert_eq!(accuracy_with_hold_as_error(&[0.1, 0.1], &samples, 0.5), 0.5);
    }

    #[test]
    fn paired_conditions_require_exact_record_and_image_identity() {
        let sample = IndependentFeatureSample {
            split: "evaluation".to_owned(),
            screen_unit: "session".to_owned(),
            record_id: "session:record-1".to_owned(),
            image_sha256: "image-hash-1".to_owned(),
            label: Some(true),
            features: vec![1.0],
        };
        let mut paired = vec![sample.clone()];
        assert!(paired_feature_samples_match(
            std::slice::from_ref(&sample),
            &paired
        ));
        paired[0].record_id = "session:record-2".to_owned();
        assert!(!paired_feature_samples_match(
            std::slice::from_ref(&sample),
            &paired
        ));
        paired[0].record_id = sample.record_id.clone();
        paired[0].image_sha256 = "image-hash-2".to_owned();
        assert!(!paired_feature_samples_match(&[sample], &paired));
    }

    #[test]
    fn real_screen_hold_validation_does_not_allow_arbitrary_exclusion() {
        let record = |id: &str, label: &str, reason: &str| RealScreenRecord {
            id: id.to_owned(),
            path: String::new(),
            captured_at_s: id.parse::<f64>().unwrap_or(0.0),
            stream_id: "stream".to_owned(),
            split: "evaluation".to_owned(),
            app: "app".to_owned(),
            title: "title".to_owned(),
            independent_label: label.to_owned(),
            label_reason: reason.to_owned(),
            width: 1,
            height: 1,
            sha256: id.to_owned(),
        };
        let records = vec![
            record(
                "0",
                "hold",
                "independent label contract: insufficient context",
            ),
            record("1", "hold", "caller requested hold"),
        ];
        let error = validate_real_screen_independent_labels(&records).expect_err("hold must fail");
        assert!(error.contains("lacks an independent annotation"));
        let human_hold = record("2", "hold", "human:hold");
        validate_real_screen_independent_labels(&[human_hold])
            .expect("annotator hold is a valid independent label");
    }

    #[test]
    fn criteria_session_split_assignment_requires_exact_manifest_sessions() {
        let record = |stream_id: &str, id: &str| RealScreenRecord {
            id: id.to_owned(),
            path: format!("{id}.png"),
            captured_at_s: id.parse::<f64>().unwrap_or(0.0),
            stream_id: stream_id.to_owned(),
            split: "unassigned".to_owned(),
            app: "app".to_owned(),
            title: "title".to_owned(),
            independent_label: "hold".to_owned(),
            label_reason: "independent_annotation_required".to_owned(),
            width: 1,
            height: 1,
            sha256: id.to_owned(),
        };
        let mut records = vec![record("screens:main", "1"), record("screens-2:main", "2")];
        assign_real_screen_splits_from_criteria(
            &mut records,
            &BTreeMap::from([
                ("screens".to_owned(), "training".to_owned()),
                ("screens-2".to_owned(), "evaluation".to_owned()),
            ]),
        )
        .expect("explicit session split assignment");
        assert_eq!(records[0].split, "training");
        assert_eq!(records[1].split, "evaluation");
        let missing = assign_real_screen_splits_from_criteria(
            &mut records.clone(),
            &BTreeMap::from([("screens".to_owned(), "training".to_owned())]),
        )
        .expect_err("a manifest session without criteria must be rejected");
        assert!(missing.contains("screens-2"));
        let extra = assign_real_screen_splits_from_criteria(
            &mut records,
            &BTreeMap::from([
                ("screens".to_owned(), "training".to_owned()),
                ("screens-2".to_owned(), "evaluation".to_owned()),
                ("screens-3".to_owned(), "held_out".to_owned()),
            ]),
        )
        .expect_err("a criteria session absent from manifests must be rejected");
        assert!(extra.contains("screens-3"));
    }

    #[test]
    fn independent_label_contract_maps_only_known_keys_and_keeps_hold_explicit() {
        let mut records = vec![RealScreenRecord {
            id: "screens:shot-000".to_owned(),
            path: "screen-0.png".to_owned(),
            captured_at_s: 1.0,
            stream_id: "screens:main".to_owned(),
            split: "training".to_owned(),
            app: "app".to_owned(),
            title: "title".to_owned(),
            independent_label: "hold".to_owned(),
            label_reason: "independent_annotation_required".to_owned(),
            width: 1,
            height: 1,
            sha256: "hash".to_owned(),
        }];
        let import = parse_independent_labels(
            br#"[{"key":"screens:0","session":"screens","label":"hold"}]"#,
        )
        .expect("page output");
        apply_independent_labels(&mut records, &import).expect("human hold label");
        assert_eq!(records[0].independent_label, "hold");
        assert_eq!(records[0].label_reason, "human:hold");
        validate_real_screen_independent_labels(&records).expect("human hold is valid");
        let import = parse_independent_labels(
            br#"[{"key":"screens:1","session":"screens","label":"notify"}]"#,
        )
        .expect("page output");
        let error = apply_independent_labels(&mut records, &import)
            .expect_err("unknown label key must be rejected");
        assert!(error.contains("does not identify a record"));
    }

    #[test]
    fn annotation_page_array_distinguishes_explicit_null_from_missing_keys() {
        let record = |index: usize| RealScreenRecord {
            id: format!("screens:shot-{index:03}"),
            path: format!("screen-{index}.png"),
            captured_at_s: index as f64,
            stream_id: "screens:main".to_owned(),
            split: "training".to_owned(),
            app: "app".to_owned(),
            title: "title".to_owned(),
            independent_label: "unlabeled".to_owned(),
            label_reason: "independent_annotation_required".to_owned(),
            width: 1,
            height: 1,
            sha256: format!("hash-{index}"),
        };
        let bytes = br#"[
          {"key":"screens:0","session":"screens","label":"notify"},
          {"key":"screens:1","session":"screens","label":null}
        ]"#;
        let import = parse_independent_labels(bytes).expect("annotation page JSON");
        assert_eq!(import.format, "annotation_page_array");
        assert_eq!(import.labels.get("screens:1"), Some(&None));
        let mut records = vec![record(0), record(1)];
        let stats = apply_independent_labels(&mut records, &import).expect("import labels");
        assert_eq!(stats.missing_count, 0);
        assert_eq!(stats.null_count, 1);
        assert_eq!(records[0].independent_label, "change");
        assert_eq!(records[1].independent_label, "unlabeled");
        assert_eq!(records[1].label_reason, "independent_annotation_null");
        let mut records_with_missing = vec![record(0), record(1), record(2)];
        let missing = apply_independent_labels(&mut records_with_missing, &import)
            .expect_err("missing record key must be an input error");
        assert!(missing.contains("missing 1 record keys"));
        let missing_label_field =
            parse_independent_labels(br#"[{"key":"screens:0","session":"screens"}]"#)
                .expect_err("missing label field must not become explicit null");
        assert!(
            missing_label_field
                .to_string()
                .contains("missing the label field")
        );
        let duplicate = parse_independent_labels(
            br#"[{"key":"screens:0","session":"screens","label":"notify"},{"key":"screens:0","session":"screens","label":"silence"}]"#,
        )
        .expect_err("duplicate annotation key");
        assert!(duplicate.to_string().contains("duplicate"));
    }

    #[test]
    fn history_features_do_not_cross_the_explicit_split_boundary() {
        let record = |id: &str, stream_id: &str, split: &str| RealScreenRecord {
            id: id.to_owned(),
            path: format!("{id}.png"),
            captured_at_s: 1.0,
            stream_id: stream_id.to_owned(),
            split: split.to_owned(),
            app: "app".to_owned(),
            title: "title".to_owned(),
            independent_label: "hold".to_owned(),
            label_reason: "independent_annotation_required".to_owned(),
            width: 1,
            height: 1,
            sha256: id.to_owned(),
        };
        let training = record("training", "screens:main", "training");
        let evaluation = record("evaluation", "screens-2:main", "evaluation");
        let response = vec![1.0_f32, 0.0];
        let mut histories = BTreeMap::<String, Vec<FrozenReadoutHistoryEntry>>::new();
        let training_scope = real_screen_history_scope(&training);
        let evaluation_scope = real_screen_history_scope(&evaluation);
        let (training_history, _) = frozen_readout_augmented_features_with_expiry(
            &response,
            histories.entry(training_scope.clone()).or_default(),
            1.0,
            4,
            1.0,
            1.0,
            60.0,
            300.0,
        );
        assert!(training_history.distance.is_none());
        histories
            .entry(training_scope)
            .or_default()
            .push(FrozenReadoutHistoryEntry {
                features: response.clone(),
                timestamp_s: 1.0,
            });
        let (evaluation_history, _) = frozen_readout_augmented_features_with_expiry(
            &response,
            histories.entry(evaluation_scope.clone()).or_default(),
            1.0,
            4,
            1.0,
            1.0,
            60.0,
            300.0,
        );
        assert!(evaluation_history.distance.is_none());
        histories
            .entry(evaluation_scope)
            .or_default()
            .push(FrozenReadoutHistoryEntry {
                features: response.clone(),
                timestamp_s: 1.0,
            });
        let (same_split_history, _) = frozen_readout_augmented_features_with_expiry(
            &response,
            histories
                .get("screens-2:main:evaluation")
                .expect("evaluation history"),
            2.0,
            4,
            1.0,
            1.0,
            60.0,
            300.0,
        );
        assert_eq!(same_split_history.distance, Some(0.0));
    }

    #[test]
    fn real_screen_bootstrap_reports_cluster_intervals_and_power_requirement() {
        let criteria = RealScreenEvaluationCriteria {
            trial: RealScreenTrialCriteria {
                bootstrap_replicates: 32,
                bootstrap_seed: 7,
                ba_ci_half_width_target: 0.1,
                required_class_count_method:
                    "conservative_95pct_ba_half_width_z_squared_p_max_over_2_half_width_squared"
                        .to_owned(),
                confidence_interval_method:
                    "screen_unit_cluster_bootstrap_for_adoption_record_bootstrap_for_trial_only"
                        .to_owned(),
                minimum_total_record_count: 1,
                maximum_total_record_count: 240,
                maximum_hold_fraction: 0.4,
                expected_record_counts_by_split: BTreeMap::new(),
                session_split_by_manifest: BTreeMap::from([
                    ("screens".to_owned(), "training".to_owned()),
                    ("screens-2".to_owned(), "evaluation".to_owned()),
                ]),
                split_assignment: "capture_session_order_training_evaluation_then_tuning_held_out"
                    .to_owned(),
                history_boundary: "stream_id_plus_split".to_owned(),
            },
            adoption: RealScreenAdoptionCriteria {
                minimum_evaluation_record_count: 4,
                minimum_independent_positive_count_for_claim: 2,
                minimum_independent_negative_count_for_claim: 2,
                minimum_independent_positive_screen_unit_count_for_claim: 2,
                minimum_independent_negative_screen_unit_count_for_claim: 2,
                minimum_evaluation_session_count: 2,
                maximum_hold_fraction: 0.4,
                performance_condition: "current_response_plus_history".to_owned(),
                minimum_balanced_accuracy: 0.55,
                minimum_auroc: 0.55,
                minimum_paired_balanced_accuracy_difference_lower_bound: 0.01,
                paired_difference_rationale:
                    "CNS response plus history must beat retina input plus history by at least 0.01 in the lower 95% session-cluster-bootstrap bound; zero or negative lower bound is not an adoption claim."
                        .to_owned(),
            },
        };
        let metric_samples = [
            ("session-1", false, 0.1),
            ("session-1", true, 0.9),
            ("session-2", false, 0.1),
            ("session-2", true, 0.9),
            ("session-3", false, 0.9),
            ("session-3", true, 0.1),
            ("session-4", false, 0.2),
            ("session-4", true, 0.8),
        ];
        let metric_samples = metric_samples
            .iter()
            .map(|(screen_unit, label, score)| IndependentFeatureSample {
                split: "evaluation".to_owned(),
                screen_unit: (*screen_unit).to_owned(),
                record_id: format!("{screen_unit}-record"),
                image_sha256: format!("{screen_unit}-hash"),
                label: Some(*label),
                features: vec![*score],
            })
            .collect::<Vec<_>>();
        let metric_refs = metric_samples.iter().collect::<Vec<_>>();
        let interval = real_screen_metric_confidence_intervals(
            &metric_refs,
            &metric_samples
                .iter()
                .map(|sample| sample.features[0])
                .collect::<Vec<_>>(),
            &metric_samples
                .iter()
                .map(|sample| sample.label.unwrap())
                .collect::<Vec<_>>(),
            0.5,
            &criteria.trial,
        )
        .expect("bootstrap interval");
        assert!(interval.0[0] < interval.0[1]);
        assert!(interval.1[0] < interval.1[1]);
        assert!(interval.2 > 0);
        let clusters = vec![vec![0], vec![1, 2], vec![3, 4, 5], vec![6, 7, 8, 9]];
        let mut rng = StdRng::seed_from_u64(criteria.trial.bootstrap_seed);
        let draws = (0..4_000)
            .map(|_| bootstrap_sample_indices(&clusters, 10, true, &mut rng))
            .collect::<Vec<_>>();
        assert!(draws.iter().any(|draw| {
            let unique = draw.iter().copied().collect::<BTreeSet<_>>();
            unique.len() < clusters.len()
        }));
        assert!(draws.iter().any(|draw| {
            let unique = draw.iter().copied().collect::<BTreeSet<_>>();
            unique.len() < draw.len()
        }));
        let cluster_selection_counts = clusters
            .iter()
            .map(|cluster| {
                draws
                    .iter()
                    .map(|draw| {
                        draw.iter().filter(|index| cluster.contains(index)).count() / cluster.len()
                    })
                    .sum::<usize>()
            })
            .collect::<Vec<_>>();
        assert!(cluster_selection_counts.iter().all(|count| *count > 0));
        let minimum = *cluster_selection_counts
            .iter()
            .min()
            .expect("cluster counts");
        let maximum = *cluster_selection_counts
            .iter()
            .max()
            .expect("cluster counts");
        let sampler_trials = 4_000 * clusters.len();
        let expected_fraction_interval =
            binomial_99_9_wilson_interval(sampler_trials / clusters.len(), sampler_trials)
                .expect("binomial interval");
        assert!(
            cluster_selection_counts.iter().all(|count| {
                let fraction = *count as f64 / sampler_trials as f64;
                fraction >= expected_fraction_interval[0]
                    && fraction <= expected_fraction_interval[1]
            }),
            "uniform cluster sampler counts={cluster_selection_counts:?}, interval={expected_fraction_interval:?}"
        );
        assert!(maximum >= minimum);
        let biased_draws = (0..4_000)
            .map(|_| bootstrap_cluster_indices_with_sampler(&clusters, |_| 0))
            .collect::<Vec<_>>();
        let biased_counts = clusters
            .iter()
            .map(|cluster| {
                biased_draws
                    .iter()
                    .map(|draw| {
                        draw.iter().filter(|index| cluster.contains(index)).count() / cluster.len()
                    })
                    .sum::<usize>()
            })
            .collect::<Vec<_>>();
        assert!(
            biased_counts.iter().enumerate().any(|(index, count)| {
                let fraction = *count as f64 / sampler_trials as f64;
                fraction < expected_fraction_interval[0]
                    || fraction > expected_fraction_interval[1]
                    || index > 0 && *count == 0
            }),
            "known biased sampler unexpectedly passed: {biased_counts:?}"
        );
        assert_eq!(
            real_screen_bootstrap_sampler_name(true),
            "StdRng::random_range_uniform_cluster_sampling"
        );
        assert_eq!(
            real_screen_confidence_interval_method(&metric_refs),
            "screen_unit_cluster_bootstrap"
        );
        assert_eq!(required_independent_class_count(&criteria.trial), 49);
        let one_cluster = [
            IndependentFeatureSample {
                split: "evaluation".to_owned(),
                screen_unit: "one-session".to_owned(),
                record_id: "one-session-record-0".to_owned(),
                image_sha256: "one-session-hash-0".to_owned(),
                label: Some(false),
                features: vec![0.0],
            },
            IndependentFeatureSample {
                split: "evaluation".to_owned(),
                screen_unit: "one-session".to_owned(),
                record_id: "one-session-record-1".to_owned(),
                image_sha256: "one-session-hash-1".to_owned(),
                label: Some(true),
                features: vec![1.0],
            },
        ];
        let one_cluster_refs = one_cluster.iter().collect::<Vec<_>>();
        assert!(
            real_screen_metric_confidence_intervals(
                &one_cluster_refs,
                &[0.1, 0.9],
                &[false, true],
                0.5,
                &criteria.trial,
            )
            .is_some()
        );
        assert_eq!(
            real_screen_confidence_interval_method(&one_cluster_refs),
            "record_bootstrap_trial_only"
        );
    }

    #[test]
    fn adoption_class_counts_are_independent_of_trial_interval_formula() {
        let mut criteria: LearningEvaluationCriteria = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/fixtures/malecns-learning-evaluation-criteria.json"
        )))
        .expect("learning criteria fixture");
        criteria
            .real_screen
            .adoption
            .minimum_independent_positive_count_for_claim = 37;
        criteria
            .real_screen
            .adoption
            .minimum_independent_negative_count_for_claim = 41;
        criteria
            .real_screen
            .adoption
            .minimum_evaluation_record_count = 78;
        validate_learning_criteria(&criteria)
            .expect("adoption counts are fixed independently from trial formula");
    }

    #[test]
    fn adoption_decision_requires_data_performance_and_cns_paired_threshold() {
        let criteria: LearningEvaluationCriteria = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/fixtures/malecns-learning-evaluation-criteria.json"
        )))
        .expect("learning criteria fixture");
        let adoption = &criteria.real_screen.adoption;
        let method = Some("screen_unit_cluster_bootstrap");
        assert!(!real_screen_performance_criteria_met(
            None,
            Some(0.9),
            Some(0.2),
            method,
            adoption
        ));
        assert!(!real_screen_performance_criteria_met(
            Some(0.55),
            Some(0.55),
            Some(0.009),
            method,
            adoption
        ));
        assert!(real_screen_performance_criteria_met(
            Some(0.55),
            Some(0.55),
            Some(0.01),
            method,
            adoption
        ));
        assert!(real_screen_performance_criteria_met(
            Some(0.9),
            Some(0.9),
            Some(0.2),
            method,
            adoption
        ));
        assert!(!real_screen_performance_criteria_met(
            Some(0.9),
            Some(0.9),
            Some(0.2),
            Some("record_bootstrap_trial_only"),
            adoption
        ));
        assert!(real_screen_adoption_criteria_met(true, true));
        assert!(!real_screen_adoption_criteria_met(false, true));
        assert!(!real_screen_adoption_criteria_met(true, false));
        assert!(!real_screen_adoption_criteria_met(false, false));
        let paired_report = |lower: Option<f64>| RealScreenPairedComparisonReport {
            cns_condition: "current_response_plus_history".to_owned(),
            retina_condition: "retina_input_plus_history".to_owned(),
            evaluation_sample_count: 4,
            cns_balanced_accuracy: Some(0.8),
            retina_balanced_accuracy: Some(0.7),
            paired_balanced_accuracy_difference: lower,
            ci95: lower.map(|value| [value, value + 0.1]),
            bootstrap_effective_replicates: lower.map_or(0, |_| 32),
            confidence_interval_method: "screen_unit_cluster_bootstrap".to_owned(),
            minimum_lower_bound: adoption.minimum_paired_balanced_accuracy_difference_lower_bound,
            criteria_met: lower.is_some_and(|value| {
                value >= adoption.minimum_paired_balanced_accuracy_difference_lower_bound
            }),
            rationale: adoption.paired_difference_rationale.clone(),
            error: None,
        };
        let (_, performance, adoption_met) = real_screen_adoption_decision(
            true,
            Some(0.55),
            Some(0.55),
            Some(&paired_report(Some(0.01))),
            adoption,
        );
        assert!(performance);
        assert!(adoption_met);
        let (_, performance, adoption_met) = real_screen_adoption_decision(
            false,
            Some(0.55),
            Some(0.55),
            Some(&paired_report(Some(0.01))),
            adoption,
        );
        assert!(performance);
        assert!(!adoption_met);
        let (_, performance, adoption_met) = real_screen_adoption_decision(
            true,
            Some(0.55),
            Some(0.55),
            Some(&paired_report(Some(0.009))),
            adoption,
        );
        assert!(!performance);
        assert!(!adoption_met);
        let (_, performance, adoption_met) = real_screen_adoption_decision(
            true,
            None,
            Some(0.55),
            Some(&paired_report(Some(0.01))),
            adoption,
        );
        assert!(!performance);
        assert!(!adoption_met);
    }

    #[test]
    fn runtime_case_lookup_rejects_a_different_image_hash_even_at_zero_distance() {
        let candidates = vec![("image-a".to_owned(), vec![0.0, 0.0])];
        assert_eq!(
            runtime_case_memory_lookup_count("image-b", &[0.0, 0.0], &candidates, 4.0),
            0
        );
    }

    fn schedule_for_split_hash_test(
        training_hash: &str,
        tuning_hash: &str,
    ) -> LearningSeedSchedule {
        let entry = |id: &str, image_sha256: &str| LearningScheduleEntry {
            id: id.to_owned(),
            kind: ScreenFixtureKind::Same,
            pair_index: 0,
            side: "a".to_owned(),
            image_sha256: image_sha256.to_owned(),
            expected_change: false,
            timestamp_s: 0.0,
        };
        LearningSeedSchedule {
            criteria_sha256: String::new(),
            base_evaluation_manifest_sha256: String::new(),
            presentation_count: 2,
            pair_count: 1,
            input_order: String::new(),
            unlabeled_side: String::new(),
            distance_metric: String::new(),
            distance_threshold_bits: 0,
            previous_absolute_difference_scale_bits: 0,
            history_elapsed_time_scale_seconds_bits: 0,
            brain_roles: Vec::new(),
            seeds: LearningSeeds {
                base: 0,
                reward_shuffle: 0,
                wiring_shuffle: 0,
                weight_shuffle: 0,
            },
            entries: Vec::new(),
            training_entries: vec![entry("training", training_hash)],
            tuning_entries: vec![entry("tuning", tuning_hash)],
            evaluation_entries: Vec::new(),
            held_out_entries: Vec::new(),
        }
    }

    #[test]
    fn learning_manifest_rejects_image_hash_overlap_between_splits() {
        let schedule = schedule_for_split_hash_test("same-hash", "same-hash");
        let error = validate_learning_split_image_hashes(&schedule)
            .expect_err("overlapping split hash must be rejected");
        assert!(error.to_string().contains("training and tuning"));
    }

    #[test]
    fn learning_manifest_allows_duplicate_image_hashes_inside_one_split() {
        let mut schedule = schedule_for_split_hash_test("same-hash", "different-hash");
        let duplicate = schedule.training_entries[0].clone();
        schedule.training_entries.push(duplicate);
        validate_learning_split_image_hashes(&schedule).expect("same-split duplicate is allowed");
    }

    #[test]
    fn real_screen_evaluation_rejects_cross_split_image_hash_overlap() {
        let record = |id: &str, split: &str| RealScreenRecord {
            id: id.to_owned(),
            path: format!("{id}.ppm"),
            captured_at_s: 1.0,
            stream_id: "screen".to_owned(),
            split: split.to_owned(),
            app: "app".to_owned(),
            title: "title".to_owned(),
            independent_label: "hold".to_owned(),
            label_reason: "test".to_owned(),
            width: 1,
            height: 1,
            sha256: "same-hash".to_owned(),
        };
        let error = validate_real_screen_split_image_hashes(&[
            record("training", "training"),
            record("evaluation", "evaluation"),
        ])
        .expect_err("overlapping real-screen split hash must be rejected");
        assert!(error.contains("training") && error.contains("evaluation"));
    }

    #[test]
    fn real_screen_split_assignment_preserves_manifest_session_order() {
        let record = |stream_id: &str, id: &str| RealScreenRecord {
            id: id.to_owned(),
            path: format!("{id}.png"),
            captured_at_s: id.parse::<f64>().unwrap_or(0.0),
            stream_id: stream_id.to_owned(),
            split: "unassigned".to_owned(),
            app: "app".to_owned(),
            title: "title".to_owned(),
            independent_label: "hold".to_owned(),
            label_reason: "independent_annotation_required".to_owned(),
            width: 1,
            height: 1,
            sha256: id.to_owned(),
        };
        let mut records = vec![
            record("screens:screen", "1"),
            record("screens:screen", "2"),
            record("screens-2:screen", "3"),
        ];
        assign_real_screen_splits(&mut records);
        assert_eq!(records[0].split, "training");
        assert_eq!(records[1].split, "training");
        assert_eq!(records[2].split, "evaluation");
        assert_eq!(
            real_screen_history_scope(&records[0]),
            "screens:screen:training"
        );
        assert_eq!(
            real_screen_history_scope(&records[2]),
            "screens-2:screen:evaluation"
        );
    }

    #[test]
    fn real_screen_normalization_requires_human_notification_annotation() {
        let mut records = vec![RealScreenRecord {
            id: "screen-1".to_owned(),
            path: "screen-1.png".to_owned(),
            captured_at_s: 1.0,
            stream_id: "screen".to_owned(),
            split: "unassigned".to_owned(),
            app: "Brave".to_owned(),
            title: "same title".to_owned(),
            independent_label: "no_change".to_owned(),
            label_reason: "app/title matched".to_owned(),
            width: 1,
            height: 1,
            sha256: "image".to_owned(),
        }];
        normalize_real_screen_records(&mut records, "screens", Path::new("."));
        assert_eq!(records[0].independent_label, "unlabeled");
        assert_eq!(records[0].label_reason, "independent_annotation_required");
    }

    #[test]
    fn runtime_case_lookup_uses_exact_hash_without_a_distance_boundary() {
        let candidates = vec![
            ("image-a".to_owned(), vec![0.0, 0.0]),
            ("image-a".to_owned(), vec![100.0, 100.0]),
            ("image-b".to_owned(), vec![0.0, 0.0]),
        ];
        assert_eq!(
            runtime_case_memory_lookup_count("image-a", &[0.0, 0.0], &candidates, 0.0),
            2
        );
    }

    #[test]
    fn small_manifest_annotation_and_criteria_use_the_report_generation_path() {
        let root = PathBuf::from(format!(
            "target/connectome-ratemodel-report-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let screens = root.join("screens");
        fs::create_dir_all(&screens).expect("report test directory");
        let manifest_path = screens.join("manifest.json");
        let labels_path = root.join("independent-labels.json");
        let output_path = root.join("report.json");
        let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let source_criteria_path =
            repository_root.join("docs/fixtures/malecns-learning-evaluation-criteria.json");
        let source_rate_criteria_path =
            repository_root.join("docs/fixtures/malecns-ratemodel-criteria.json");
        let rate_criteria: RateEvaluationCriteria = serde_json::from_slice(
            &fs::read(&source_rate_criteria_path).expect("rate criteria fixture"),
        )
        .expect("rate criteria");
        let rate_criteria_path = root.join("rate-criteria.json");
        fs::copy(&source_rate_criteria_path, &rate_criteria_path).expect("copy rate criteria");
        let mut fixture_records = Vec::new();
        for pair_index in 0..rate_criteria.pair_count {
            for (kind, name) in [
                (ScreenFixtureKind::Same, "same"),
                (ScreenFixtureKind::Near, "near"),
                (ScreenFixtureKind::Unrelated, "unrelated"),
            ] {
                for side in ["a", "b"] {
                    let fixture = ScreenFixture::generate(
                        kind,
                        pair_index,
                        side,
                        rate_criteria.width,
                        rate_criteria.height,
                    )
                    .expect("generate fixture");
                    fixture_records.push(FixtureRecord {
                        id: format!("{name}-{pair_index:02}-{side}"),
                        kind,
                        pair_index,
                        side: side.to_owned(),
                        path: String::new(),
                        width: rate_criteria.width,
                        height: rate_criteria.height,
                        sha256: fixture.sha256,
                    });
                }
            }
        }
        let fixtures = FixtureManifest {
            schema_version: RATE_EVALUATION_SCHEMA_VERSION,
            width: rate_criteria.width,
            height: rate_criteria.height,
            pair_count: rate_criteria.pair_count,
            records: fixture_records,
        };
        let fixtures_path = root.join("fixtures.json");
        write_json(&fixtures_path, &fixtures).expect("fixture manifest");
        let pack_path = root.join("pack");
        fs::create_dir_all(&pack_path).expect("pack directory");
        let pack_manifest_path = pack_path.join("rate_manifest.json");
        fs::write(&pack_manifest_path, b"{}\n").expect("pack manifest");
        let base_schedule = canonical_schedule(&rate_criteria, &fixtures).expect("base schedule");
        let base_manifest = RateEvaluationManifest {
            schema_version: RATE_EVALUATION_SCHEMA_VERSION,
            canonical_data_root: root.to_string_lossy().into_owned(),
            canonical_pack: pack_path.to_string_lossy().into_owned(),
            canonical_pack_manifest_sha256: sha256_bytes(
                &fs::read(&pack_manifest_path).expect("pack manifest bytes"),
            ),
            criteria_file: rate_criteria_path.to_string_lossy().into_owned(),
            criteria_sha256: sha256_bytes(
                &fs::read(&rate_criteria_path).expect("rate criteria bytes"),
            ),
            fixtures_file: fixtures_path.to_string_lossy().into_owned(),
            fixtures_sha256: sha256_bytes(&fs::read(&fixtures_path).expect("fixture bytes")),
            retina_input_types: rate_criteria.input_types.clone(),
            retina_coordinate_rule: rate_criteria.retina.coordinate_rule.clone(),
            retina_unknown_side_policy: rate_criteria.retina.unknown_side_policy.clone(),
            seed_schedule_canonical_bytes: String::from_utf8(base_schedule.clone())
                .expect("base schedule UTF-8"),
            seed_schedule_sha256: sha256_bytes(&base_schedule),
        };
        let base_manifest_path = root.join("base-manifest.json");
        write_json(&base_manifest_path, &base_manifest).expect("base manifest");
        let mut small_criteria: LearningEvaluationCriteria =
            serde_json::from_slice(&fs::read(&source_criteria_path).expect("criteria fixture"))
                .expect("criteria");
        small_criteria.real_screen.trial.session_split_by_manifest =
            BTreeMap::from([("screens".to_owned(), "training".to_owned())]);
        let criteria_path = root.join("small-criteria.json");
        write_json(&criteria_path, &small_criteria).expect("small criteria");
        let base_manifest_sha256 =
            sha256_bytes(&fs::read(&base_manifest_path).expect("base manifest bytes"));
        let small_schedule = learning_schedule(&small_criteria, &base_manifest_sha256, &fixtures)
            .expect("small criteria schedule");
        let small_schedule_bytes =
            serde_json::to_vec(&small_schedule).expect("small criteria schedule bytes");
        let learning_manifest = LearningEvaluationManifest {
            schema_version: RATE_EVALUATION_SCHEMA_VERSION,
            canonical_pack: base_manifest.canonical_pack,
            canonical_pack_manifest_sha256: base_manifest.canonical_pack_manifest_sha256,
            base_evaluation_manifest: base_manifest_path.to_string_lossy().into_owned(),
            base_evaluation_manifest_sha256: base_manifest_sha256,
            criteria_file: criteria_path.to_string_lossy().into_owned(),
            criteria_sha256: sha256_bytes(&fs::read(&criteria_path).expect("criteria bytes")),
            fixtures_file: base_manifest.fixtures_file,
            fixtures_sha256: base_manifest.fixtures_sha256,
            seed_schedule_canonical_bytes: String::from_utf8(small_schedule_bytes.clone())
                .expect("schedule UTF-8"),
            seed_schedule_sha256: sha256_bytes(&small_schedule_bytes),
        };
        let learning_manifest_path = root.join("learning-manifest.json");
        write_json(&learning_manifest_path, &learning_manifest).expect("small learning manifest");
        write_json(
            &manifest_path,
            &RealScreenManifest {
                schema_version: RATE_EVALUATION_SCHEMA_VERSION,
                source: "small report test".to_owned(),
                capture_status: "in_progress".to_owned(),
                label_contract: independent_label_contract(),
                records: vec![RealScreenRecord {
                    id: "shot-000".to_owned(),
                    path: "missing.png".to_owned(),
                    captured_at_s: 1.0,
                    stream_id: "screen".to_owned(),
                    split: "unassigned".to_owned(),
                    app: "Editor".to_owned(),
                    title: "document".to_owned(),
                    independent_label: "unlabeled".to_owned(),
                    label_reason: "independent_annotation_required".to_owned(),
                    width: 1,
                    height: 1,
                    sha256: "small-image-hash".to_owned(),
                }],
            },
        )
        .expect("small manifest");
        fs::write(
            &labels_path,
            br#"[{"key":"screens:0","session":"screens","label":null}]"#,
        )
        .expect("small annotation");
        real_screen_evaluate(&BTreeMap::from([
            (
                "--learning-manifest".to_owned(),
                learning_manifest_path.to_string_lossy().into_owned(),
            ),
            (
                "--real-manifest".to_owned(),
                manifest_path.to_string_lossy().into_owned(),
            ),
            (
                "--independent-labels".to_owned(),
                labels_path.to_string_lossy().into_owned(),
            ),
            (
                "--output".to_owned(),
                output_path.to_string_lossy().into_owned(),
            ),
        ]))
        .expect("small report generation");
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(&output_path).expect("report output"))
                .expect("report");
        assert_eq!(report["status"], "blocked");
        assert_eq!(report["calculation_status"], "not_started");
        assert_eq!(report["record_count"], 1);
        assert_eq!(report["independent_label_null_count"], 1);
        assert!(
            report["normalized_manifest_path"]
                .as_str()
                .is_some_and(|path| { Path::new(path).exists() })
        );
        fs::remove_dir_all(root).expect("remove report test directory");
    }
}
