use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{RateGraph, RateModelError, RetinaMap};

/// Protocol identifier used at the CooSenpAI observation boundary.
pub const RATE_HELPER_OBSERVATION_SCHEMA: &str = "coosenpai-observation-v1";

/// A response group and its fixed feature scaling in a helper artifact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateHelperResponseGroup {
    /// Stable group name.
    pub name: String,
    /// Neurons included in the group, in graph index order.
    pub neuron_indices: Vec<u32>,
    /// Fixed rest-response scale for the group.
    pub scale: f64,
}

/// One fitted logistic readout saved for the resident helper.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateHelperBrain {
    /// Stable brain identifier.
    pub id: String,
    /// Readout role, for example `change` or `no_change`.
    pub role: String,
    /// Number of response features before the four history features.
    pub feature_dimension: usize,
    /// Standardization mean for response and history features.
    pub standardization_mean: Vec<f64>,
    /// Standardization scale for response and history features.
    pub standardization_scale: Vec<f64>,
    /// Logistic weights after fitting.
    pub weights: Vec<f64>,
    /// Logistic bias after fitting.
    pub bias: f64,
    /// Probability threshold used by the helper's consensus policy.
    pub reaction_threshold: f64,
}

/// Configuration for exact-hash case memory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateHelperCaseMemory {
    /// The only primary match key accepted by the helper.
    pub match_policy: String,
    /// Exponential decay time constant in seconds.
    pub time_constant_seconds: f64,
    /// Probability-logit adjustment per unit case strength.
    pub logit_scale: f64,
    /// Maximum retained case count.
    pub max_cases: usize,
}

/// Complete frozen-CNS and readout artifact consumed by `connectome-helper`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateHelperArtifact {
    /// Boundary protocol identifier.
    pub observation_schema: String,
    /// Artifact schema version.
    pub artifact_version: u32,
    /// Canonical rate pack directory.
    pub pack_path: String,
    /// SHA-256 of the canonical rate pack manifest.
    pub pack_manifest_sha256: String,
    /// Fingerprint of the graph and fixed source signs.
    pub graph_fingerprint: String,
    /// Fixed image-to-retina mapping.
    pub retina_map: RetinaMap,
    /// Number of rate updates for one observation.
    pub rate_steps_per_observation: usize,
    /// Upper clipping value used by the rate engine.
    pub hmax: f64,
    /// Number of fixed distribution-projection buckets per response group.
    pub response_projection_dimension: usize,
    /// Number of recent observations used by the online history features.
    pub history_recent_observations: usize,
    /// Fixed normalized-distance threshold for the history features.
    pub history_distance_threshold: f64,
    /// Fixed scale for the previous absolute-difference history feature.
    pub history_previous_absolute_difference_scale: f64,
    /// Fixed scale for elapsed time in the history features.
    pub history_elapsed_time_scale_seconds: f64,
    /// Maximum age of an observation retained by the short-term history.
    pub history_max_age_seconds: f64,
    /// Epsilon used for active and zero-response measurements.
    pub zero_response_epsilon: f64,
    /// Fixed downstream response groups.
    pub response_groups: Vec<RateHelperResponseGroup>,
    /// Fitted readouts, normally one entry for each role.
    pub brains: Vec<RateHelperBrain>,
    /// Margin required for a two-role consensus action.
    pub consensus_margin: f64,
    /// Exact-hash case-memory policy.
    pub case_memory: RateHelperCaseMemory,
}

impl RateHelperArtifact {
    /// Loads an artifact from JSON without accepting an invalid shape.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, RateModelError> {
        let bytes = fs::read(path)?;
        let artifact: Self = serde_json::from_slice(&bytes)?;
        artifact.validate_shape(None)?;
        Ok(artifact)
    }

    /// Writes an artifact as pretty JSON after validating its shape.
    pub fn write(&self, path: impl AsRef<Path>) -> Result<(), RateModelError> {
        self.validate_shape(None)?;
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// Validates an artifact and, when supplied, its graph-dependent shape.
    pub fn validate_shape(&self, graph: Option<&RateGraph>) -> Result<(), RateModelError> {
        if self.observation_schema != RATE_HELPER_OBSERVATION_SCHEMA
            || self.artifact_version != 1
            || self.pack_path.is_empty()
            || !is_sha256_hex(&self.pack_manifest_sha256)
            || !is_sha256_hex(&self.graph_fingerprint)
            || self.rate_steps_per_observation == 0
            || !self.hmax.is_finite()
            || self.hmax <= 0.0
            || self.response_projection_dimension == 0
            || self.history_recent_observations == 0
            || !self.history_distance_threshold.is_finite()
            || self.history_distance_threshold <= 0.0
            || !self.history_previous_absolute_difference_scale.is_finite()
            || self.history_previous_absolute_difference_scale <= 0.0
            || !self.history_elapsed_time_scale_seconds.is_finite()
            || self.history_elapsed_time_scale_seconds <= 0.0
            || !self.history_max_age_seconds.is_finite()
            || self.history_max_age_seconds <= 0.0
            || !self.zero_response_epsilon.is_finite()
            || self.zero_response_epsilon <= 0.0
            || self.response_groups.is_empty()
            || !self.consensus_margin.is_finite()
            || !(0.0..=1.0).contains(&self.consensus_margin)
            || self.case_memory.match_policy != "exact_image_sha256"
            || !self.case_memory.time_constant_seconds.is_finite()
            || self.case_memory.time_constant_seconds <= 0.0
            || !self.case_memory.logit_scale.is_finite()
            || self.case_memory.logit_scale <= 0.0
            || self.case_memory.max_cases == 0
        {
            return Err(RateModelError::Invalid(
                "rate helper artifact header or policy is invalid".to_owned(),
            ));
        }
        let Some(graph) = graph else {
            self.validate_readout_shapes(None)?;
            return Ok(());
        };
        if self.graph_fingerprint != graph.fingerprint() {
            return Err(RateModelError::Invalid(
                "rate helper artifact graph fingerprint does not match pack".to_owned(),
            ));
        }
        if self
            .retina_map
            .entries
            .iter()
            .any(|entry| entry.neuron_index as usize >= graph.neuron_count())
        {
            return Err(RateModelError::Invalid(
                "rate helper artifact retina entry is outside the pack".to_owned(),
            ));
        }
        for group in &self.response_groups {
            if group.name.is_empty()
                || group.neuron_indices.is_empty()
                || group
                    .neuron_indices
                    .windows(2)
                    .any(|pair| pair[0] >= pair[1])
                || group
                    .neuron_indices
                    .iter()
                    .any(|index| *index as usize >= graph.neuron_count())
                || !group.scale.is_finite()
                || group.scale <= 0.0
            {
                return Err(RateModelError::Invalid(
                    "rate helper artifact response group is invalid".to_owned(),
                ));
            }
        }
        self.validate_readout_shapes(Some(graph.neuron_count()))
    }

    fn validate_readout_shapes(&self, _neuron_count: Option<usize>) -> Result<(), RateModelError> {
        let response_dimension = self
            .response_groups
            .len()
            .checked_mul(4 + self.response_projection_dimension)
            .ok_or_else(|| {
                RateModelError::Invalid("rate helper feature dimension overflowed".to_owned())
            })?;
        if self.brains.len() != 2
            || self
                .brains
                .iter()
                .any(|brain| brain.feature_dimension != response_dimension)
        {
            return Err(RateModelError::Invalid(
                "rate helper artifact readout dimensions do not match groups".to_owned(),
            ));
        }
        for brain in &self.brains {
            let dimension = response_dimension.checked_add(4).ok_or_else(|| {
                RateModelError::Invalid("rate helper readout dimension overflowed".to_owned())
            })?;
            if brain.id.is_empty()
                || brain.role.is_empty()
                || brain.standardization_mean.len() != dimension
                || brain.standardization_scale.len() != dimension
                || brain.weights.len() != dimension
                || brain
                    .standardization_mean
                    .iter()
                    .any(|value| !value.is_finite())
                || brain
                    .standardization_scale
                    .iter()
                    .any(|value| !value.is_finite() || *value <= 0.0)
                || brain.weights.iter().any(|value| !value.is_finite())
                || !brain.bias.is_finite()
                || !brain.reaction_threshold.is_finite()
                || !(0.0..=1.0).contains(&brain.reaction_threshold)
            {
                return Err(RateModelError::Invalid(
                    "rate helper artifact readout is invalid".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
