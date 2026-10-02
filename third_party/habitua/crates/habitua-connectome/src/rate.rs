use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const LEGACY_RATE_PACK_FORMAT_VERSION: u16 = 1;
const RATE_PACK_FORMAT_VERSION: u16 = 2;
const LEGACY_RATE_NEURON_FORMAT_VERSION: u16 = 1;
const RATE_NEURON_FORMAT_VERSION: u16 = 2;
const RATE_OUT_MAGIC: &[u8; 8] = b"HRATEOUT";
const RATE_IN_MAGIC: &[u8; 8] = b"HRATEIN\0";

/// A source file recorded in a rate-pack manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceFile {
    /// Human-readable file name.
    pub name: String,
    /// URL from which the file was obtained.
    pub url: String,
    /// Source repository commit or release identifier.
    pub commit: String,
    /// Number of bytes obtained.
    pub size_bytes: u64,
    /// SHA-256 of the obtained file in lowercase hexadecimal.
    pub sha256: String,
    /// URL or document that states the applicable license.
    pub license_source: String,
}

/// Annotation retained for one neuron in a rate-model pack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateNeuronMetadata {
    /// Stable MaleCNS body ID.
    pub body_id: u64,
    /// MaleCNS type annotation.
    pub type_name: Option<String>,
    /// Annotation class.
    pub class: Option<String>,
    /// Annotation superclass.
    pub superclass: Option<String>,
    /// Annotation subclass.
    pub subclass: Option<String>,
    /// Soma side.
    pub soma_side: Option<String>,
    /// Root side.
    pub root_side: Option<String>,
    /// MaleCNS optic-lobe hex coordinate 1.
    pub assigned_ol_hex1: Option<f64>,
    /// MaleCNS optic-lobe hex coordinate 2.
    pub assigned_ol_hex2: Option<f64>,
    /// Selected transmitter label, if one was present.
    pub selected_neurotransmitter: Option<String>,
    /// Rule that selected the transmitter label.
    pub neurotransmitter_source: String,
    /// Signed source multiplier used by the rate model.
    pub source_sign: i8,
}

/// One outgoing edge in the rate-model CSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateOutgoingEdge {
    /// Target neuron index.
    pub target: u32,
    /// Aggregated contact count.
    pub contact_count: u64,
}

/// One incoming edge in the reverse CSR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateIncomingEdge {
    /// Source neuron index.
    pub source: u32,
    /// Position of the corresponding outgoing edge.
    pub edge_id: u32,
}

/// Manifest for a rate-model data pack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RatePackManifest {
    /// Rate-pack binary format version.
    pub format_version: u16,
    /// Dataset release used to create the pack.
    pub dataset_version: String,
    /// Converter revision or release label.
    pub source_commit: String,
    /// Generation timestamp supplied by the caller.
    pub generated_at: String,
    /// License for the source data.
    pub license: String,
    /// Number of neurons.
    pub neuron_count: usize,
    /// Number of unique directed cell pairs.
    pub edge_count: usize,
    /// Sum of contact counts over retained pairs.
    pub contact_count: u64,
    /// SHA-256 of `rate-neurons.json`.
    pub neurons_sha256: String,
    /// SHA-256 of `rate-out.bin`.
    pub outgoing_sha256: String,
    /// SHA-256 of `rate-in.bin`.
    pub incoming_sha256: String,
    /// SHA-256 of `rate-populations.json`.
    pub populations_sha256: String,
    /// Deterministic normalization rule.
    pub normalization: String,
    /// Neurotransmitter sign rule.
    pub sign_rule: String,
    /// Reasons for rows or annotations excluded during conversion.
    pub exclusion_reasons: BTreeMap<String, u64>,
    /// Counts of raw edge rows by selected sign rule.
    pub sign_edge_counts: BTreeMap<String, u64>,
    /// Number of source rows read from the connectome table.
    pub raw_edge_row_count: u64,
    /// Number of valid positive-contact rows retained before pair aggregation.
    pub retained_raw_edge_row_count: u64,
    /// Number of Parquet row groups read from the connectome table.
    pub connectome_row_group_count: u64,
    /// Optional row-group limit used for a bounded conversion trial.
    #[serde(default)]
    pub row_group_limit: Option<u64>,
    /// Number of external-sort runs created during conversion.
    pub external_sort_run_count: u64,
    /// Converter wall-clock duration in milliseconds.
    pub conversion_elapsed_ms: f64,
    /// Peak resident set size when measured by the caller.
    pub peak_rss_bytes: Option<u64>,
    /// Original files and hashes.
    pub source_files: Vec<SourceFile>,
    /// Feather-to-Parquet intermediates and hashes.
    pub intermediate_files: Vec<SourceFile>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RatePopulationFile {
    format_version: u16,
    populations: BTreeMap<String, Vec<u64>>,
}

/// A fixed graph for the scalar rate model.
#[derive(Clone, Debug, PartialEq)]
pub struct RateGraph {
    /// Body IDs in deterministic ascending order.
    pub root_ids: Vec<u64>,
    /// Outgoing CSR row offsets.
    pub outgoing_offsets: Vec<u64>,
    /// Outgoing CSR edges, ordered by source then target.
    pub outgoing_edges: Vec<RateOutgoingEdge>,
    /// Incoming CSR row offsets.
    pub incoming_offsets: Vec<u64>,
    /// Incoming CSR edges, ordered by target then source.
    pub incoming_edges: Vec<RateIncomingEdge>,
    /// Source sign for each neuron.
    pub source_sign: Vec<i8>,
    /// Retained annotation for each neuron in row order.
    pub neuron_metadata: Vec<RateNeuronMetadata>,
    /// Named populations as neuron indices.
    pub populations: BTreeMap<String, Vec<u32>>,
    /// Receiver-side denominator `max(1, sum_j contact_count_ij)`.
    pub incoming_contact_totals: Vec<u64>,
}

impl RateGraph {
    /// Constructs and validates a rate graph from both CSR directions.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        root_ids: Vec<u64>,
        outgoing_offsets: Vec<u64>,
        outgoing_edges: Vec<RateOutgoingEdge>,
        incoming_offsets: Vec<u64>,
        incoming_edges: Vec<RateIncomingEdge>,
        source_sign: Vec<i8>,
        neuron_metadata: Vec<RateNeuronMetadata>,
        populations: BTreeMap<String, Vec<u32>>,
    ) -> Result<Self, RateModelError> {
        let neuron_count = root_ids.len();
        if neuron_count == 0 {
            return Err(RateModelError::Invalid(
                "rate graph has no neurons".to_owned(),
            ));
        }
        if root_ids.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(RateModelError::Invalid(
                "rate graph body IDs must be strictly ascending".to_owned(),
            ));
        }
        if outgoing_offsets.len() != neuron_count + 1 || incoming_offsets.len() != neuron_count + 1
        {
            return Err(RateModelError::Invalid(
                "rate CSR offsets must have neuron_count + 1 entries".to_owned(),
            ));
        }
        validate_offsets(&outgoing_offsets, outgoing_edges.len(), "outgoing")?;
        validate_offsets(&incoming_offsets, incoming_edges.len(), "incoming")?;
        if source_sign.len() != neuron_count || neuron_metadata.len() != neuron_count {
            return Err(RateModelError::Invalid(
                "rate neuron arrays do not have a common length".to_owned(),
            ));
        }
        if source_sign.iter().any(|sign| !matches!(sign, -1..=1)) {
            return Err(RateModelError::Invalid(
                "rate source signs must be -1, 0, or 1".to_owned(),
            ));
        }
        if outgoing_edges
            .iter()
            .any(|edge| edge.target as usize >= neuron_count)
        {
            return Err(RateModelError::Invalid(
                "rate outgoing target is outside the neuron table".to_owned(),
            ));
        }
        if incoming_edges.iter().any(|edge| {
            edge.source as usize >= neuron_count || edge.edge_id as usize >= outgoing_edges.len()
        }) {
            return Err(RateModelError::Invalid(
                "rate incoming edge references an unknown neuron or edge".to_owned(),
            ));
        }
        if incoming_edges.len() != outgoing_edges.len() {
            return Err(RateModelError::Invalid(
                "rate incoming and outgoing CSRs have different edge counts".to_owned(),
            ));
        }
        let mut seen_outgoing = vec![false; outgoing_edges.len()];
        for (incoming_index, incoming_edge) in incoming_edges.iter().enumerate() {
            let target = offset_row(&incoming_offsets, incoming_index);
            let edge_id = incoming_edge.edge_id as usize;
            if seen_outgoing[edge_id] {
                return Err(RateModelError::Invalid(format!(
                    "rate incoming CSR references outgoing edge {edge_id} more than once"
                )));
            }
            seen_outgoing[edge_id] = true;
            let source = offset_row(&outgoing_offsets, edge_id);
            if incoming_edge.source as usize != source {
                return Err(RateModelError::Invalid(format!(
                    "rate incoming CSR source disagrees for outgoing edge {edge_id}"
                )));
            }
            if outgoing_edges[edge_id].target as usize != target {
                return Err(RateModelError::Invalid(format!(
                    "rate incoming CSR target disagrees for outgoing edge {edge_id}"
                )));
            }
        }
        if seen_outgoing.iter().any(|seen| !seen) {
            return Err(RateModelError::Invalid(
                "rate incoming CSR omits an outgoing edge".to_owned(),
            ));
        }
        validate_populations(&populations, neuron_count)?;
        let mut incoming_contact_totals = vec![0_u64; neuron_count];
        for edge in &outgoing_edges {
            incoming_contact_totals[edge.target as usize] = incoming_contact_totals
                [edge.target as usize]
                .checked_add(edge.contact_count)
                .ok_or_else(|| {
                    RateModelError::Invalid("incoming contact total overflowed".to_owned())
                })?;
        }
        for total in &mut incoming_contact_totals {
            *total = (*total).max(1);
        }
        Ok(Self {
            root_ids,
            outgoing_offsets,
            outgoing_edges,
            incoming_offsets,
            incoming_edges,
            source_sign,
            neuron_metadata,
            populations,
            incoming_contact_totals,
        })
    }

    /// Returns the number of neurons.
    pub fn neuron_count(&self) -> usize {
        self.root_ids.len()
    }

    /// Returns the number of unique directed cell pairs.
    pub fn edge_count(&self) -> usize {
        self.outgoing_edges.len()
    }

    /// Returns a deterministic hash of the graph structure and fixed signs.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        for body_id in &self.root_ids {
            hasher.update(body_id.to_le_bytes());
        }
        for source in 0..self.neuron_count() {
            for edge_id in self.outgoing_range(source).expect("validated rate graph") {
                let edge = self.outgoing_edges[edge_id];
                hasher.update((source as u32).to_le_bytes());
                hasher.update(edge.target.to_le_bytes());
                hasher.update(edge.contact_count.to_le_bytes());
            }
        }
        for sign in &self.source_sign {
            hasher.update([*sign as u8]);
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Returns a named population.
    pub fn population(&self, name: &str) -> Result<&[u32], RateModelError> {
        self.populations
            .get(name)
            .map(Vec::as_slice)
            .ok_or_else(|| RateModelError::Invalid(format!("rate population is missing: {name}")))
    }

    /// Returns the outgoing edge range for one source neuron.
    pub fn outgoing_range(&self, source: usize) -> Result<std::ops::Range<usize>, RateModelError> {
        if source >= self.neuron_count() {
            return Err(RateModelError::Invalid(format!(
                "rate source neuron {source} is outside the graph"
            )));
        }
        Ok(self.outgoing_offsets[source] as usize..self.outgoing_offsets[source + 1] as usize)
    }

    /// Returns the incoming edge range for one target neuron.
    pub fn incoming_range(&self, target: usize) -> Result<std::ops::Range<usize>, RateModelError> {
        if target >= self.neuron_count() {
            return Err(RateModelError::Invalid(format!(
                "rate target neuron {target} is outside the graph"
            )));
        }
        Ok(self.incoming_offsets[target] as usize..self.incoming_offsets[target + 1] as usize)
    }

    /// Writes the complete rate pack without adding data to the repository.
    pub fn write_pack(
        &self,
        directory: impl AsRef<Path>,
        manifest: &RatePackManifest,
    ) -> Result<(), RateModelError> {
        let directory = directory.as_ref();
        fs::create_dir_all(directory)?;
        let neurons = serde_json::to_vec_pretty(&self.neuron_metadata)?;
        let populations = serde_json::to_vec_pretty(&RatePopulationFile {
            format_version: RATE_NEURON_FORMAT_VERSION,
            populations: self
                .populations
                .iter()
                .map(|(name, indices)| {
                    (
                        name.clone(),
                        indices
                            .iter()
                            .map(|index| self.root_ids[*index as usize])
                            .collect(),
                    )
                })
                .collect(),
        })?;
        let outgoing = encode_outgoing(self)?;
        let incoming = encode_incoming(self)?;
        let mut manifest = manifest.clone();
        manifest.format_version = RATE_PACK_FORMAT_VERSION;
        manifest.neuron_count = self.neuron_count();
        manifest.edge_count = self.edge_count();
        manifest.contact_count = self
            .outgoing_edges
            .iter()
            .try_fold(0_u64, |sum, edge| sum.checked_add(edge.contact_count))
            .ok_or_else(|| RateModelError::Invalid("contact count overflowed".to_owned()))?;
        manifest.neurons_sha256 = sha256_hex(&neurons);
        manifest.outgoing_sha256 = sha256_hex(&outgoing);
        manifest.incoming_sha256 = sha256_hex(&incoming);
        manifest.populations_sha256 = sha256_hex(&populations);
        write_file(directory.join("rate-neurons.json"), &neurons)?;
        write_file(directory.join("rate-out.bin"), &outgoing)?;
        write_file(directory.join("rate-in.bin"), &incoming)?;
        write_file(directory.join("rate-populations.json"), &populations)?;
        write_file(
            directory.join("rate_manifest.json"),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        Ok(())
    }

    /// Loads and verifies a rate-model pack.
    pub fn load(directory: impl AsRef<Path>) -> Result<Self, RateModelError> {
        let directory = directory.as_ref();
        let manifest: RatePackManifest =
            serde_json::from_slice(&fs::read(directory.join("rate_manifest.json"))?)?;
        if !is_supported_rate_pack_version(manifest.format_version) {
            return Err(RateModelError::Invalid(format!(
                "unsupported rate pack format version {}",
                manifest.format_version
            )));
        }
        let neurons_bytes = fs::read(directory.join("rate-neurons.json"))?;
        let outgoing_bytes = fs::read(directory.join("rate-out.bin"))?;
        let incoming_bytes = fs::read(directory.join("rate-in.bin"))?;
        let populations_bytes = fs::read(directory.join("rate-populations.json"))?;
        verify_hash(
            &neurons_bytes,
            &manifest.neurons_sha256,
            "rate-neurons.json",
        )?;
        verify_hash(&outgoing_bytes, &manifest.outgoing_sha256, "rate-out.bin")?;
        verify_hash(&incoming_bytes, &manifest.incoming_sha256, "rate-in.bin")?;
        verify_hash(
            &populations_bytes,
            &manifest.populations_sha256,
            "rate-populations.json",
        )?;
        let neuron_metadata: Vec<RateNeuronMetadata> = serde_json::from_slice(&neurons_bytes)?;
        let root_ids: Vec<u64> = neuron_metadata
            .iter()
            .map(|metadata| metadata.body_id)
            .collect();
        let source_sign: Vec<i8> = neuron_metadata
            .iter()
            .map(|metadata| metadata.source_sign)
            .collect();
        let (outgoing_offsets, outgoing_edges) = decode_outgoing(&outgoing_bytes)?;
        let (incoming_offsets, incoming_edges) = decode_incoming(&incoming_bytes)?;
        let population_file: RatePopulationFile = serde_json::from_slice(&populations_bytes)?;
        if !is_supported_rate_neuron_version(population_file.format_version) {
            return Err(RateModelError::Invalid(format!(
                "unsupported rate population format version {}",
                population_file.format_version
            )));
        }
        if manifest.neuron_count != root_ids.len()
            || manifest.edge_count != outgoing_edges.len()
            || manifest.neuron_count != outgoing_offsets.len().saturating_sub(1)
            || manifest.edge_count != incoming_edges.len()
        {
            return Err(RateModelError::Invalid(
                "rate manifest counts do not match pack files".to_owned(),
            ));
        }
        let root_to_index = root_ids
            .iter()
            .enumerate()
            .map(|(index, root_id)| (*root_id, index as u32))
            .collect::<BTreeMap<_, _>>();
        let populations = population_file
            .populations
            .into_iter()
            .map(|(name, roots)| {
                let mut indices = roots
                    .into_iter()
                    .map(|root| {
                        root_to_index.get(&root).copied().ok_or_else(|| {
                            RateModelError::Invalid(format!(
                                "rate population {name} references unknown body ID {root}"
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                indices.sort_unstable();
                indices.dedup();
                Ok((name, indices))
            })
            .collect::<Result<BTreeMap<_, _>, RateModelError>>()?;
        Self::new(
            root_ids,
            outgoing_offsets,
            outgoing_edges,
            incoming_offsets,
            incoming_edges,
            source_sign,
            neuron_metadata,
            populations,
        )
    }
}

/// Parameters of the nfly-style scalar rate update.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateParameters {
    /// Alpha logits, transformed with sigmoid before each update.
    pub alpha_logits: Vec<f64>,
    /// Per-neuron bias.
    pub bias: Vec<f64>,
    /// Per-edge log gains in outgoing CSR order.
    pub log_gain: Vec<f64>,
    /// Upper clipping value of the rate state.
    pub hmax: f64,
}

impl RateParameters {
    /// Returns the plan's initial parameters: alpha=.7, bias=.1, gain=1, hmax=10.
    pub fn initial(graph: &RateGraph) -> Self {
        let alpha_logit = (0.7_f64 / 0.3).ln();
        Self {
            alpha_logits: vec![alpha_logit; graph.neuron_count()],
            bias: vec![0.1; graph.neuron_count()],
            log_gain: vec![0.0; graph.edge_count()],
            hmax: 10.0,
        }
    }

    pub(crate) fn validate(&self, graph: &RateGraph) -> Result<(), RateModelError> {
        if self.alpha_logits.len() != graph.neuron_count()
            || self.bias.len() != graph.neuron_count()
            || self.log_gain.len() != graph.edge_count()
        {
            return Err(RateModelError::Invalid(
                "rate parameter arrays do not match the graph".to_owned(),
            ));
        }
        if !self.hmax.is_finite() || self.hmax <= 0.0 {
            return Err(RateModelError::Invalid(
                "rate hmax must be finite and positive".to_owned(),
            ));
        }
        if self
            .alpha_logits
            .iter()
            .chain(&self.bias)
            .chain(&self.log_gain)
            .any(|value| !value.is_finite())
        {
            return Err(RateModelError::Invalid(
                "rate parameters must be finite".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Forward state history used by the dedicated reverse pass.
#[derive(Clone, Debug, PartialEq)]
pub struct RateForwardResult {
    /// State history, including h^0 and h^T.
    pub activity_history: Vec<Vec<f64>>,
    /// Pre-clipping affine values z^t.
    pub preactivation_history: Vec<Vec<f64>>,
}

impl RateForwardResult {
    /// Returns the final state h^T.
    pub fn final_activity(&self) -> &[f64] {
        self.activity_history
            .last()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Returns the number of recurrent updates.
    pub fn steps(&self) -> usize {
        self.preactivation_history.len()
    }
}

/// Scalar rate engine using the outgoing CSR and receiver-side normalization.
#[derive(Clone, Debug)]
pub struct RateEngine {
    graph: RateGraph,
    parameters: RateParameters,
}

impl RateEngine {
    /// Creates an engine with validated nfly-style parameters.
    pub fn new(graph: RateGraph, parameters: RateParameters) -> Result<Self, RateModelError> {
        parameters.validate(&graph)?;
        Ok(Self { graph, parameters })
    }

    /// Returns the immutable graph.
    pub fn graph(&self) -> &RateGraph {
        &self.graph
    }

    /// Returns the current parameters.
    pub fn parameters(&self) -> &RateParameters {
        &self.parameters
    }

    /// Replaces parameters after validation.
    pub fn set_parameters(&mut self, parameters: RateParameters) -> Result<(), RateModelError> {
        parameters.validate(&self.graph)?;
        self.parameters = parameters;
        Ok(())
    }

    /// Runs `steps` recurrent updates from zero or the supplied initial state.
    pub fn forward(
        &self,
        input: &[f64],
        steps: usize,
        initial: Option<&[f64]>,
    ) -> Result<RateForwardResult, RateModelError> {
        forward_with_parameters(&self.graph, &self.parameters, input, steps, initial)
    }

    /// Runs the same update with f32 arithmetic for the production-precision
    /// validation path. The f64 path remains the independent reference path.
    pub fn forward_f32(
        &self,
        input: &[f32],
        steps: usize,
        initial: Option<&[f32]>,
    ) -> Result<RateForwardF32, RateModelError> {
        forward_f32_with_parameters(&self.graph, &self.parameters, input, steps, initial)
    }
}

/// State history from the f32 rate update.
#[derive(Clone, Debug, PartialEq)]
pub struct RateForwardF32 {
    /// State history, including h^0 and h^T.
    pub activity_history: Vec<Vec<f32>>,
    /// Pre-clipping affine values z^t.
    pub preactivation_history: Vec<Vec<f32>>,
}

/// One forward-pass timing breakdown for the large rate graph.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RateForwardTiming {
    /// Time spent traversing outgoing CSR edges, in milliseconds.
    pub csr_traversal_ms: f64,
    /// Time spent applying the per-neuron update, in milliseconds.
    pub neuron_update_ms: f64,
    /// Time spent in setup and buffer management, in milliseconds.
    pub other_ms: f64,
}

impl RateForwardF32 {
    /// Returns the final f32 state.
    pub fn final_activity(&self) -> &[f32] {
        self.activity_history
            .last()
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

pub(crate) fn forward_with_parameters(
    graph: &RateGraph,
    parameters: &RateParameters,
    input: &[f64],
    steps: usize,
    initial: Option<&[f64]>,
) -> Result<RateForwardResult, RateModelError> {
    parameters.validate(graph)?;
    let neuron_count = graph.neuron_count();
    validate_vector(input, neuron_count, "rate input")?;
    if steps == 0 {
        return Err(RateModelError::Invalid(
            "rate forward steps must be positive".to_owned(),
        ));
    }
    let mut activity = initial
        .map(|value| {
            validate_vector(value, neuron_count, "rate initial activity")?;
            Ok::<Vec<f64>, RateModelError>(value.to_vec())
        })
        .transpose()?
        .unwrap_or_else(|| vec![0.0; neuron_count]);
    let mut activity_history = Vec::with_capacity(steps + 1);
    let mut preactivation_history = Vec::with_capacity(steps);
    activity_history.push(activity.clone());
    for _ in 0..steps {
        let mut preactivation = parameters.bias.clone();
        for (index, value) in input.iter().enumerate() {
            preactivation[index] += *value;
        }
        for (source, source_activity) in activity.iter().copied().enumerate() {
            let sign = f64::from(graph.source_sign[source]);
            if sign == 0.0 || source_activity == 0.0 {
                continue;
            }
            for edge_id in graph.outgoing_range(source)? {
                let edge = graph.outgoing_edges[edge_id];
                let weight = f64::from(graph.source_sign[source]) * edge.contact_count as f64
                    / graph.incoming_contact_totals[edge.target as usize] as f64
                    * parameters.log_gain[edge_id].exp();
                preactivation[edge.target as usize] += weight * source_activity;
            }
        }
        let mut next = vec![0.0; neuron_count];
        for index in 0..neuron_count {
            let alpha = sigmoid(parameters.alpha_logits[index]);
            let clipped = preactivation[index].clamp(0.0, parameters.hmax);
            next[index] = (1.0 - alpha) * activity[index] + alpha * clipped;
        }
        preactivation_history.push(preactivation);
        activity = next;
        activity_history.push(activity.clone());
    }
    Ok(RateForwardResult {
        activity_history,
        preactivation_history,
    })
}

/// Runs the recurrent interval without retaining the history needed by
/// reverse propagation. This is the inference-only path for large packs.
pub(crate) fn final_activity_with_parameters(
    graph: &RateGraph,
    parameters: &RateParameters,
    input: &[f64],
    steps: usize,
    initial: Option<&[f64]>,
) -> Result<Vec<f64>, RateModelError> {
    Ok(
        final_activity_with_parameters_timed(graph, parameters, input, steps, initial, None, None)?
            .0,
    )
}

/// Runs one inference interval and returns a timing breakdown for profiling.
pub fn final_activity_with_parameters_profile(
    graph: &RateGraph,
    parameters: &RateParameters,
    input: &[f64],
    steps: usize,
    initial: Option<&[f64]>,
) -> Result<(Vec<f64>, RateForwardTiming), RateModelError> {
    let mut timing = RateForwardTiming::default();
    let activity = final_activity_with_parameters_timed(
        graph,
        parameters,
        input,
        steps,
        initial,
        Some(&mut timing),
        None,
    )?
    .0;
    Ok((activity, timing))
}

/// Bounded display samples of the actual inference steps (initial state included).
#[derive(Debug, Default)]
pub struct RateActivitySamples {
    /// Neuron indices sampled for display; the recurrent computation remains complete.
    pub neuron_indices: Vec<usize>,
    /// Actual rate step and activity converted to f32 for display.
    pub frames: Vec<(usize, Vec<f32>)>,
}

/// Runs the same inference with bounded display sampling, without full history retention.
pub fn final_activity_with_parameters_sampled_profile(
    graph: &RateGraph,
    parameters: &RateParameters,
    input: &[f64],
    steps: usize,
    samples: &mut RateActivitySamples,
) -> Result<(Vec<f64>, RateForwardTiming), RateModelError> {
    if samples.neuron_indices.len() > 1024
        || samples
            .neuron_indices
            .iter()
            .any(|index| *index >= graph.neuron_count())
        || samples
            .neuron_indices
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err(RateModelError::Invalid(
            "invalid activity sample indices".into(),
        ));
    }
    samples.frames.clear();
    let mut timing = RateForwardTiming::default();
    final_activity_with_parameters_timed(
        graph,
        parameters,
        input,
        steps,
        None,
        Some(&mut timing),
        Some(samples),
    )
}

fn record_activity_sample(samples: &mut RateActivitySamples, step: usize, activity: &[f64]) {
    if samples.neuron_indices.is_empty() {
        return;
    }
    samples.frames.push((
        step,
        samples
            .neuron_indices
            .iter()
            .map(|index| activity[*index] as f32)
            .collect(),
    ));
}

fn final_activity_with_parameters_timed(
    graph: &RateGraph,
    parameters: &RateParameters,
    input: &[f64],
    steps: usize,
    initial: Option<&[f64]>,
    mut timing: Option<&mut RateForwardTiming>,
    mut samples: Option<&mut RateActivitySamples>,
) -> Result<(Vec<f64>, RateForwardTiming), RateModelError> {
    parameters.validate(graph)?;
    let neuron_count = graph.neuron_count();
    validate_vector(input, neuron_count, "rate final activity input")?;
    if steps == 0 {
        return Err(RateModelError::Invalid(
            "rate final activity steps must be positive".to_owned(),
        ));
    }
    let mut activity = initial
        .map(|value| {
            validate_vector(value, neuron_count, "rate final activity initial")?;
            Ok::<Vec<f64>, RateModelError>(value.to_vec())
        })
        .transpose()?
        .unwrap_or_else(|| vec![0.0; neuron_count]);
    if let Some(sample) = samples.as_deref_mut() {
        record_activity_sample(sample, 0, &activity);
    }
    let sample_stride = steps.div_ceil(16);
    for step in 1..=steps {
        let other_started = Instant::now();
        let mut preactivation = parameters.bias.clone();
        for (index, value) in input.iter().enumerate() {
            preactivation[index] += *value;
        }
        if let Some(profile) = timing.as_deref_mut() {
            profile.other_ms += other_started.elapsed().as_secs_f64() * 1_000.0;
        }
        let csr_started = Instant::now();
        for (source, source_activity) in activity.iter().copied().enumerate() {
            let sign = f64::from(graph.source_sign[source]);
            if sign == 0.0 || source_activity == 0.0 {
                continue;
            }
            for edge_id in graph.outgoing_range(source)? {
                let edge = graph.outgoing_edges[edge_id];
                preactivation[edge.target as usize] +=
                    edge_weight(graph, parameters, edge_id, source, edge) * source_activity;
            }
        }
        if let Some(profile) = timing.as_deref_mut() {
            profile.csr_traversal_ms += csr_started.elapsed().as_secs_f64() * 1_000.0;
        }
        let neuron_update_started = Instant::now();
        let mut next = vec![0.0; neuron_count];
        for index in 0..neuron_count {
            let alpha = sigmoid(parameters.alpha_logits[index]);
            next[index] = (1.0 - alpha) * activity[index]
                + alpha * preactivation[index].clamp(0.0, parameters.hmax);
        }
        if let Some(profile) = timing.as_deref_mut() {
            profile.neuron_update_ms += neuron_update_started.elapsed().as_secs_f64() * 1_000.0;
        }
        activity = next;
        if (step.is_multiple_of(sample_stride) || step == steps)
            && let Some(sample) = samples.as_deref_mut()
        {
            record_activity_sample(sample, step, &activity);
        }
    }
    Ok((activity, timing.map(|profile| *profile).unwrap_or_default()))
}

fn forward_f32_with_parameters(
    graph: &RateGraph,
    parameters: &RateParameters,
    input: &[f32],
    steps: usize,
    initial: Option<&[f32]>,
) -> Result<RateForwardF32, RateModelError> {
    parameters.validate(graph)?;
    let neuron_count = graph.neuron_count();
    if input.len() != neuron_count || input.iter().any(|value| !value.is_finite()) {
        return Err(RateModelError::Invalid(
            "rate f32 input has an invalid length or value".to_owned(),
        ));
    }
    if steps == 0 {
        return Err(RateModelError::Invalid(
            "rate f32 forward steps must be positive".to_owned(),
        ));
    }
    let mut activity = initial
        .map(|value| {
            if value.len() != neuron_count || value.iter().any(|item| !item.is_finite()) {
                return Err(RateModelError::Invalid(
                    "rate f32 initial activity has an invalid length or value".to_owned(),
                ));
            }
            Ok(value.to_vec())
        })
        .transpose()?
        .unwrap_or_else(|| vec![0.0; neuron_count]);
    let mut activity_history = Vec::with_capacity(steps + 1);
    let mut preactivation_history = Vec::with_capacity(steps);
    activity_history.push(activity.clone());
    let hmax = parameters.hmax as f32;
    for _ in 0..steps {
        let mut preactivation = parameters
            .bias
            .iter()
            .zip(input)
            .map(|(bias, value)| *bias as f32 + *value)
            .collect::<Vec<_>>();
        for (source, source_activity) in activity.iter().copied().enumerate() {
            let sign = graph.source_sign[source] as f32;
            if sign == 0.0 || source_activity == 0.0 {
                continue;
            }
            for edge_id in graph.outgoing_range(source)? {
                let edge = graph.outgoing_edges[edge_id];
                let weight = sign * edge.contact_count as f32
                    / graph.incoming_contact_totals[edge.target as usize] as f32
                    * (parameters.log_gain[edge_id] as f32).exp();
                preactivation[edge.target as usize] += weight * source_activity;
            }
        }
        let mut next = vec![0.0; neuron_count];
        for index in 0..neuron_count {
            let alpha = sigmoid_f32(parameters.alpha_logits[index] as f32);
            next[index] =
                (1.0 - alpha) * activity[index] + alpha * preactivation[index].clamp(0.0, hmax);
        }
        preactivation_history.push(preactivation);
        activity = next;
        activity_history.push(activity.clone());
    }
    Ok(RateForwardF32 {
        activity_history,
        preactivation_history,
    })
}

/// Gradient arrays produced by the full-interval reverse pass.
#[derive(Clone, Debug, PartialEq)]
pub struct RateGradients {
    /// Derivative with respect to each edge log gain.
    pub log_gain: Vec<f64>,
    /// Derivative with respect to each neuron bias.
    pub bias: Vec<f64>,
    /// Derivative with respect to each alpha logit.
    pub alpha_logits: Vec<f64>,
    /// Derivative with respect to the constant input vector.
    pub input: Vec<f64>,
    /// Derivative with respect to h^0.
    pub initial_activity: Vec<f64>,
}

/// Dedicated reverse-mode implementation for the complete expanded interval.
pub struct RateBackward;

impl RateBackward {
    /// Backpropagates a final-state loss gradient through every recurrent update.
    pub fn backward(
        engine: &RateEngine,
        forward: &RateForwardResult,
        final_gradient: &[f64],
    ) -> Result<RateGradients, RateModelError> {
        Self::backward_with_parameters(&engine.graph, &engine.parameters, forward, final_gradient)
    }

    /// Backpropagates without cloning the graph or parameter arrays.
    pub fn backward_with_parameters(
        graph: &RateGraph,
        parameters: &RateParameters,
        forward: &RateForwardResult,
        final_gradient: &[f64],
    ) -> Result<RateGradients, RateModelError> {
        parameters.validate(graph)?;
        let neuron_count = graph.neuron_count();
        validate_vector(final_gradient, neuron_count, "rate final gradient")?;
        if forward.activity_history.len() != forward.preactivation_history.len() + 1
            || forward.activity_history.len() < 2
        {
            return Err(RateModelError::Invalid(
                "rate forward history is incomplete".to_owned(),
            ));
        }
        let mut gradients = RateGradients {
            log_gain: vec![0.0; graph.edge_count()],
            bias: vec![0.0; neuron_count],
            alpha_logits: vec![0.0; neuron_count],
            input: vec![0.0; neuron_count],
            initial_activity: vec![0.0; neuron_count],
        };
        let mut lambda_next = final_gradient.to_vec();
        for time in (0..forward.steps()).rev() {
            let activity = &forward.activity_history[time];
            let preactivation = &forward.preactivation_history[time];
            let mut delta = vec![0.0; neuron_count];
            for index in 0..neuron_count {
                let derivative = clip_derivative(preactivation[index], parameters.hmax);
                let alpha = sigmoid(parameters.alpha_logits[index]);
                delta[index] = lambda_next[index] * alpha * derivative;
                gradients.bias[index] += delta[index];
                gradients.input[index] += delta[index];
                gradients.alpha_logits[index] += lambda_next[index]
                    * (preactivation[index].clamp(0.0, parameters.hmax) - activity[index])
                    * alpha
                    * (1.0 - alpha);
            }
            let mut lambda = vec![0.0; neuron_count];
            for index in 0..neuron_count {
                let alpha = sigmoid(parameters.alpha_logits[index]);
                lambda[index] += (1.0 - alpha) * lambda_next[index];
            }
            for source in 0..neuron_count {
                for edge_id in graph.outgoing_range(source)? {
                    let edge = graph.outgoing_edges[edge_id];
                    let weight = edge_weight(graph, parameters, edge_id, source, edge);
                    let target = edge.target as usize;
                    gradients.log_gain[edge_id] += delta[target] * weight * activity[source];
                    lambda[source] += weight * delta[target];
                }
            }
            lambda_next = lambda;
        }
        gradients.initial_activity = lambda_next;
        Ok(gradients)
    }
}

fn edge_weight(
    graph: &RateGraph,
    parameters: &RateParameters,
    edge_id: usize,
    source: usize,
    edge: RateOutgoingEdge,
) -> f64 {
    f64::from(graph.source_sign[source]) * edge.contact_count as f64
        / graph.incoming_contact_totals[edge.target as usize] as f64
        * parameters.log_gain[edge_id].exp()
}

/// Independent f64 reference implementation used for numerical validation.
pub fn reference_forward(
    graph: &RateGraph,
    parameters: &RateParameters,
    input: &[f64],
    steps: usize,
    initial: Option<&[f64]>,
) -> Result<Vec<f64>, RateModelError> {
    parameters.validate(graph)?;
    validate_vector(input, graph.neuron_count(), "rate reference input")?;
    let mut activity = initial
        .map(|value| {
            validate_vector(
                value,
                graph.neuron_count(),
                "rate reference initial activity",
            )?;
            Ok::<Vec<f64>, RateModelError>(value.to_vec())
        })
        .transpose()?
        .unwrap_or_else(|| vec![0.0; graph.neuron_count()]);
    for _ in 0..steps {
        let mut next = vec![0.0; graph.neuron_count()];
        for target in 0..graph.neuron_count() {
            let mut z = parameters.bias[target] + input[target];
            for (source, source_activity) in activity.iter().copied().enumerate() {
                for edge_id in graph.outgoing_range(source)? {
                    let edge = graph.outgoing_edges[edge_id];
                    if edge.target as usize == target {
                        let weight = f64::from(graph.source_sign[source])
                            * edge.contact_count as f64
                            / graph.incoming_contact_totals[target] as f64
                            * parameters.log_gain[edge_id].exp();
                        z += weight * source_activity;
                    }
                }
            }
            let alpha = sigmoid(parameters.alpha_logits[target]);
            next[target] = (1.0 - alpha) * activity[target] + alpha * z.clamp(0.0, parameters.hmax);
        }
        activity = next;
    }
    Ok(activity)
}

/// Errors from rate-pack I/O and numerical validation.
#[derive(Debug)]
pub enum RateModelError {
    /// Operating-system I/O failure.
    Io(io::Error),
    /// JSON encoding or decoding failure.
    Json(serde_json::Error),
    /// Invalid graph, pack, or numerical input.
    Invalid(String),
}

impl fmt::Display for RateModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "rate model I/O error: {error}"),
            Self::Json(error) => write!(formatter, "rate model JSON error: {error}"),
            Self::Invalid(reason) => write!(formatter, "invalid rate model: {reason}"),
        }
    }
}

impl std::error::Error for RateModelError {}

impl From<io::Error> for RateModelError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for RateModelError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

fn validate_offsets(
    offsets: &[u64],
    edge_count: usize,
    direction: &str,
) -> Result<(), RateModelError> {
    if offsets.first().copied() != Some(0)
        || offsets.last().copied() != Some(edge_count as u64)
        || offsets.windows(2).any(|pair| pair[0] > pair[1])
    {
        return Err(RateModelError::Invalid(format!(
            "{direction} CSR offsets are invalid"
        )));
    }
    Ok(())
}

fn offset_row(offsets: &[u64], edge_id: usize) -> usize {
    offsets
        .partition_point(|offset| *offset <= edge_id as u64)
        .saturating_sub(1)
}

fn validate_populations(
    populations: &BTreeMap<String, Vec<u32>>,
    neuron_count: usize,
) -> Result<(), RateModelError> {
    for (name, indices) in populations {
        if name.is_empty()
            || indices.windows(2).any(|pair| pair[0] >= pair[1])
            || indices.iter().any(|index| *index as usize >= neuron_count)
        {
            return Err(RateModelError::Invalid(format!(
                "rate population {name} is not sorted or is out of range"
            )));
        }
    }
    Ok(())
}

fn validate_vector(values: &[f64], expected: usize, label: &str) -> Result<(), RateModelError> {
    if values.len() != expected || values.iter().any(|value| !value.is_finite()) {
        return Err(RateModelError::Invalid(format!(
            "{label} must contain {expected} finite values"
        )));
    }
    Ok(())
}

fn sigmoid(value: f64) -> f64 {
    if value >= 0.0 {
        let e = (-value).exp();
        1.0 / (1.0 + e)
    } else {
        let e = value.exp();
        e / (1.0 + e)
    }
}

fn sigmoid_f32(value: f32) -> f32 {
    if value >= 0.0 {
        let e = (-value).exp();
        1.0 / (1.0 + e)
    } else {
        let e = value.exp();
        e / (1.0 + e)
    }
}

fn clip_derivative(value: f64, hmax: f64) -> f64 {
    f64::from((0.0 < value && value < hmax) as u8)
}

fn encode_outgoing(graph: &RateGraph) -> Result<Vec<u8>, RateModelError> {
    encode_outgoing_with_version(graph, RATE_PACK_FORMAT_VERSION)
}

fn encode_outgoing_with_version(
    graph: &RateGraph,
    version: u16,
) -> Result<Vec<u8>, RateModelError> {
    let mut bytes =
        Vec::with_capacity(28 + (graph.neuron_count() + 1) * 8 + graph.edge_count() * 12);
    bytes.extend_from_slice(RATE_OUT_MAGIC);
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&(graph.neuron_count() as u64).to_le_bytes());
    bytes.extend_from_slice(&(graph.edge_count() as u64).to_le_bytes());
    for offset in &graph.outgoing_offsets {
        bytes.extend_from_slice(&offset.to_le_bytes());
    }
    for edge in &graph.outgoing_edges {
        bytes.extend_from_slice(&edge.target.to_le_bytes());
        bytes.extend_from_slice(&edge.contact_count.to_le_bytes());
    }
    Ok(bytes)
}

fn encode_incoming(graph: &RateGraph) -> Result<Vec<u8>, RateModelError> {
    encode_incoming_with_version(graph, RATE_PACK_FORMAT_VERSION)
}

fn encode_incoming_with_version(
    graph: &RateGraph,
    version: u16,
) -> Result<Vec<u8>, RateModelError> {
    let mut bytes =
        Vec::with_capacity(28 + (graph.neuron_count() + 1) * 8 + graph.edge_count() * 8);
    bytes.extend_from_slice(RATE_IN_MAGIC);
    bytes.extend_from_slice(&version.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&(graph.neuron_count() as u64).to_le_bytes());
    bytes.extend_from_slice(&(graph.edge_count() as u64).to_le_bytes());
    for offset in &graph.incoming_offsets {
        bytes.extend_from_slice(&offset.to_le_bytes());
    }
    for edge in &graph.incoming_edges {
        bytes.extend_from_slice(&edge.source.to_le_bytes());
        bytes.extend_from_slice(&edge.edge_id.to_le_bytes());
    }
    Ok(bytes)
}

fn decode_outgoing(bytes: &[u8]) -> Result<(Vec<u64>, Vec<RateOutgoingEdge>), RateModelError> {
    let mut cursor = Cursor::new(bytes);
    cursor.expect(RATE_OUT_MAGIC)?;
    cursor.expect_version()?;
    let neuron_count = cursor.read_u64()? as usize;
    let edge_count = cursor.read_u64()? as usize;
    let offsets = (0..=neuron_count)
        .map(|_| cursor.read_u64())
        .collect::<Result<Vec<_>, _>>()?;
    let edges = (0..edge_count)
        .map(|_| {
            Ok(RateOutgoingEdge {
                target: cursor.read_u32()?,
                contact_count: cursor.read_u64()?,
            })
        })
        .collect::<Result<Vec<_>, RateModelError>>()?;
    cursor.finish()?;
    Ok((offsets, edges))
}

fn decode_incoming(bytes: &[u8]) -> Result<(Vec<u64>, Vec<RateIncomingEdge>), RateModelError> {
    let mut cursor = Cursor::new(bytes);
    cursor.expect(RATE_IN_MAGIC)?;
    cursor.expect_version()?;
    let neuron_count = cursor.read_u64()? as usize;
    let edge_count = cursor.read_u64()? as usize;
    let offsets = (0..=neuron_count)
        .map(|_| cursor.read_u64())
        .collect::<Result<Vec<_>, _>>()?;
    let edges = (0..edge_count)
        .map(|_| {
            Ok(RateIncomingEdge {
                source: cursor.read_u32()?,
                edge_id: cursor.read_u32()?,
            })
        })
        .collect::<Result<Vec<_>, RateModelError>>()?;
    cursor.finish()?;
    Ok((offsets, edges))
}

fn write_file(path: impl AsRef<Path>, bytes: &[u8]) -> Result<(), RateModelError> {
    fs::write(path, bytes)?;
    Ok(())
}

fn verify_hash(bytes: &[u8], expected: &str, name: &str) -> Result<(), RateModelError> {
    if sha256_hex(bytes) != expected {
        return Err(RateModelError::Invalid(format!(
            "{name} SHA-256 does not match rate manifest"
        )));
    }
    Ok(())
}

fn is_supported_rate_pack_version(version: u16) -> bool {
    matches!(
        version,
        LEGACY_RATE_PACK_FORMAT_VERSION | RATE_PACK_FORMAT_VERSION
    )
}

fn is_supported_rate_neuron_version(version: u16) -> bool {
    matches!(
        version,
        LEGACY_RATE_NEURON_FORMAT_VERSION | RATE_NEURON_FORMAT_VERSION
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn expect(&mut self, expected: &[u8]) -> Result<(), RateModelError> {
        if self.read(expected.len())? != expected {
            return Err(RateModelError::Invalid(
                "rate binary magic mismatch".to_owned(),
            ));
        }
        Ok(())
    }

    fn expect_version(&mut self) -> Result<(), RateModelError> {
        let version = self.read_u16()?;
        if !is_supported_rate_pack_version(version) {
            return Err(RateModelError::Invalid(format!(
                "unsupported rate binary version {version}"
            )));
        }
        let _reserved = self.read_u16()?;
        Ok(())
    }

    fn read(&mut self, length: usize) -> Result<&'a [u8], RateModelError> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| RateModelError::Invalid("rate binary length overflowed".to_owned()))?;
        let result = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| RateModelError::Invalid("rate binary is truncated".to_owned()))?;
        self.position = end;
        Ok(result)
    }

    fn read_u16(&mut self) -> Result<u16, RateModelError> {
        Ok(u16::from_le_bytes(
            self.read(2)?.try_into().expect("slice length is fixed"),
        ))
    }

    fn read_u32(&mut self) -> Result<u32, RateModelError> {
        Ok(u32::from_le_bytes(
            self.read(4)?.try_into().expect("slice length is fixed"),
        ))
    }

    fn read_u64(&mut self) -> Result<u64, RateModelError> {
        Ok(u64::from_le_bytes(
            self.read(8)?.try_into().expect("slice length is fixed"),
        ))
    }

    fn finish(self) -> Result<(), RateModelError> {
        if self.position != self.bytes.len() {
            return Err(RateModelError::Invalid(
                "rate binary has trailing bytes".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct RateFixture {
        format_version: u16,
        neuron_metadata: Vec<RateNeuronMetadata>,
        outgoing_offsets: Vec<u64>,
        outgoing_edges: Vec<FixtureOutgoingEdge>,
        incoming_offsets: Vec<u64>,
        incoming_edges: Vec<FixtureIncomingEdge>,
        populations: BTreeMap<String, Vec<u64>>,
    }

    #[derive(Deserialize)]
    struct FixtureOutgoingEdge {
        target: u32,
        contact_count: u64,
    }

    #[derive(Deserialize)]
    struct FixtureIncomingEdge {
        source: u32,
        edge_id: u32,
    }

    fn graph() -> RateGraph {
        let metadata = (0..3)
            .map(|body_id| RateNeuronMetadata {
                body_id,
                type_name: Some(format!("T{body_id}")),
                class: None,
                superclass: None,
                subclass: None,
                soma_side: None,
                root_side: None,
                assigned_ol_hex1: None,
                assigned_ol_hex2: None,
                selected_neurotransmitter: None,
                neurotransmitter_source: "test".to_owned(),
                source_sign: if body_id == 1 { -1 } else { 1 },
            })
            .collect();
        RateGraph::new(
            vec![0, 1, 2],
            vec![0, 2, 3, 4],
            vec![
                RateOutgoingEdge {
                    target: 1,
                    contact_count: 2,
                },
                RateOutgoingEdge {
                    target: 2,
                    contact_count: 1,
                },
                RateOutgoingEdge {
                    target: 2,
                    contact_count: 3,
                },
                RateOutgoingEdge {
                    target: 0,
                    contact_count: 1,
                },
            ],
            vec![0, 1, 2, 4],
            vec![
                RateIncomingEdge {
                    source: 2,
                    edge_id: 3,
                },
                RateIncomingEdge {
                    source: 0,
                    edge_id: 0,
                },
                RateIncomingEdge {
                    source: 0,
                    edge_id: 1,
                },
                RateIncomingEdge {
                    source: 1,
                    edge_id: 2,
                },
            ],
            vec![1, -1, 1],
            metadata,
            BTreeMap::new(),
        )
        .expect("test graph should be valid")
    }

    fn edge_case_graph() -> RateGraph {
        let metadata = (0..5)
            .map(|body_id| RateNeuronMetadata {
                body_id,
                type_name: Some(format!("T{body_id}")),
                class: None,
                superclass: None,
                subclass: None,
                soma_side: None,
                root_side: None,
                assigned_ol_hex1: None,
                assigned_ol_hex2: None,
                selected_neurotransmitter: None,
                neurotransmitter_source: "test".to_owned(),
                source_sign: [1, -1, 1, 0, 1][body_id as usize],
            })
            .collect();
        RateGraph::new(
            vec![0, 1, 2, 3, 4],
            vec![0, 2, 3, 4, 5, 5],
            vec![
                RateOutgoingEdge {
                    target: 0,
                    contact_count: 2,
                },
                RateOutgoingEdge {
                    target: 1,
                    contact_count: 1,
                },
                RateOutgoingEdge {
                    target: 2,
                    contact_count: 1,
                },
                RateOutgoingEdge {
                    target: 2,
                    contact_count: 1,
                },
                RateOutgoingEdge {
                    target: 4,
                    contact_count: 1,
                },
            ],
            vec![0, 1, 2, 4, 4, 5],
            vec![
                RateIncomingEdge {
                    source: 0,
                    edge_id: 0,
                },
                RateIncomingEdge {
                    source: 0,
                    edge_id: 1,
                },
                RateIncomingEdge {
                    source: 1,
                    edge_id: 2,
                },
                RateIncomingEdge {
                    source: 2,
                    edge_id: 3,
                },
                RateIncomingEdge {
                    source: 3,
                    edge_id: 4,
                },
            ],
            vec![1, -1, 1, 0, 1],
            metadata,
            BTreeMap::new(),
        )
        .expect("edge-case graph should be valid")
    }

    #[test]
    fn display_samples_match_actual_forward_steps_without_changing_final_activity() {
        let graph = graph();
        let parameters = RateParameters::initial(&graph);
        let input = [1e-6, 0.0, 0.0];
        let engine = RateEngine::new(graph.clone(), parameters.clone()).expect("engine");
        let reference = engine.forward(&input, 32, None).expect("forward");
        let mut samples = RateActivitySamples {
            neuron_indices: vec![0, 2],
            frames: Vec::new(),
        };
        let (final_activity, _) = final_activity_with_parameters_sampled_profile(
            &graph,
            &parameters,
            &input,
            32,
            &mut samples,
        )
        .expect("sampled inference");
        assert_eq!(final_activity, reference.final_activity());
        assert_eq!(samples.frames.len(), 17);
        assert_eq!(samples.frames.first().unwrap().0, 0);
        assert_eq!(samples.frames.last().unwrap().0, 32);
        for (step, values) in &samples.frames {
            for (index, value) in samples.neuron_indices.iter().zip(values) {
                assert_eq!(*value, reference.activity_history[*step][*index] as f32);
            }
        }
        assert!(samples.frames[1].1[0] > 0.0);
        samples.neuron_indices = vec![2, 0];
        assert!(
            final_activity_with_parameters_sampled_profile(
                &graph,
                &parameters,
                &input,
                32,
                &mut samples
            )
            .is_err()
        );
    }

    #[test]
    fn forward_matches_the_independent_f64_reference() {
        let graph = graph();
        let parameters = RateParameters::initial(&graph);
        let engine =
            RateEngine::new(graph.clone(), parameters.clone()).expect("engine should build");
        let input = vec![0.2, 0.1, 0.0];
        let result = engine.forward(&input, 5, None).expect("forward should run");
        let reference =
            reference_forward(&graph, &parameters, &input, 5, None).expect("reference should run");
        for (actual, expected) in result.final_activity().iter().zip(reference) {
            assert!((actual - expected).abs() < 1.0e-12);
        }
    }

    #[test]
    fn backward_matches_finite_difference_for_inhibitory_and_zero_edges() {
        let graph = graph();
        let parameters = RateParameters::initial(&graph);
        let engine =
            RateEngine::new(graph.clone(), parameters.clone()).expect("engine should build");
        let input = vec![0.3, 0.2, 0.0];
        let forward = engine.forward(&input, 4, None).expect("forward should run");
        let gradient = vec![1.0, 0.0, 0.0];
        let actual =
            RateBackward::backward(&engine, &forward, &gradient).expect("backward should run");
        let epsilon = 1.0e-6;
        for edge_id in 0..engine.graph().edge_count() {
            let mut plus = parameters.clone();
            plus.log_gain[edge_id] += epsilon;
            let mut minus = parameters.clone();
            minus.log_gain[edge_id] -= epsilon;
            let plus_engine = RateEngine::new(engine.graph().clone(), plus).expect("plus engine");
            let minus_engine =
                RateEngine::new(engine.graph().clone(), minus).expect("minus engine");
            let plus_value = plus_engine
                .forward(&input, 4, None)
                .expect("plus forward")
                .final_activity()[0];
            let minus_value = minus_engine
                .forward(&input, 4, None)
                .expect("minus forward")
                .final_activity()[0];
            let finite_difference = (plus_value - minus_value) / (2.0 * epsilon);
            assert!((actual.log_gain[edge_id] - finite_difference).abs() < 1.0e-5);
        }
    }

    #[test]
    fn backward_covers_self_edge_zero_input_and_unreachable_readout() {
        let graph = edge_case_graph();
        let mut parameters = RateParameters::initial(&graph);
        parameters.bias.fill(2.0);
        parameters.alpha_logits.fill((0.4_f64 / 0.6).ln());
        let engine = RateEngine::new(graph.clone(), parameters.clone()).expect("engine");
        let input = vec![0.1, 0.0, 0.1, 0.0, 0.0];
        let initial = vec![0.2, 0.1, 0.1, 0.0, 0.1];
        let forward = engine
            .forward(&input, 3, Some(&initial))
            .expect("forward should run");
        assert!(
            forward
                .preactivation_history
                .iter()
                .flatten()
                .all(|value| *value > 0.0 && *value < parameters.hmax)
        );
        let gradient = vec![0.2, -0.3, 0.7, 0.0, 0.0];
        let actual = RateBackward::backward(&engine, &forward, &gradient).expect("backward");
        let epsilon = 1.0e-6;
        for edge_id in 0..engine.graph().edge_count() {
            let mut plus = parameters.clone();
            plus.log_gain[edge_id] += epsilon;
            let mut minus = parameters.clone();
            minus.log_gain[edge_id] -= epsilon;
            let plus_value = RateEngine::new(graph.clone(), plus)
                .expect("plus engine")
                .forward(&input, 3, Some(&initial))
                .expect("plus forward")
                .final_activity()
                .iter()
                .zip(&gradient)
                .map(|(value, weight)| value * weight)
                .sum::<f64>();
            let minus_value = RateEngine::new(graph.clone(), minus)
                .expect("minus engine")
                .forward(&input, 3, Some(&initial))
                .expect("minus forward")
                .final_activity()
                .iter()
                .zip(&gradient)
                .map(|(value, weight)| value * weight)
                .sum::<f64>();
            let finite_difference = (plus_value - minus_value) / (2.0 * epsilon);
            assert!((actual.log_gain[edge_id] - finite_difference).abs() < 1.0e-5);
        }
        for index in 0..engine.graph().neuron_count() {
            let mut plus = parameters.clone();
            plus.alpha_logits[index] += epsilon;
            let mut minus = parameters.clone();
            minus.alpha_logits[index] -= epsilon;
            let plus_value = RateEngine::new(graph.clone(), plus)
                .expect("plus alpha engine")
                .forward(&input, 3, Some(&initial))
                .expect("plus alpha forward")
                .final_activity()
                .iter()
                .zip(&gradient)
                .map(|(value, weight)| value * weight)
                .sum::<f64>();
            let minus_value = RateEngine::new(graph.clone(), minus)
                .expect("minus alpha engine")
                .forward(&input, 3, Some(&initial))
                .expect("minus alpha forward")
                .final_activity()
                .iter()
                .zip(&gradient)
                .map(|(value, weight)| value * weight)
                .sum::<f64>();
            assert!(
                (actual.alpha_logits[index] - (plus_value - minus_value) / (2.0 * epsilon)).abs()
                    < 1.0e-5
            );

            let mut plus = parameters.clone();
            plus.bias[index] += epsilon;
            let mut minus = parameters.clone();
            minus.bias[index] -= epsilon;
            let plus_value = RateEngine::new(graph.clone(), plus)
                .expect("plus bias engine")
                .forward(&input, 3, Some(&initial))
                .expect("plus bias forward")
                .final_activity()
                .iter()
                .zip(&gradient)
                .map(|(value, weight)| value * weight)
                .sum::<f64>();
            let minus_value = RateEngine::new(graph.clone(), minus)
                .expect("minus bias engine")
                .forward(&input, 3, Some(&initial))
                .expect("minus bias forward")
                .final_activity()
                .iter()
                .zip(&gradient)
                .map(|(value, weight)| value * weight)
                .sum::<f64>();
            assert!(
                (actual.bias[index] - (plus_value - minus_value) / (2.0 * epsilon)).abs() < 1.0e-5
            );
        }
        for index in 0..engine.graph().neuron_count() {
            let mut plus_input = input.clone();
            plus_input[index] += epsilon;
            let mut minus_input = input.clone();
            minus_input[index] -= epsilon;
            let plus_value = engine
                .forward(&plus_input, 3, Some(&initial))
                .expect("plus input forward")
                .final_activity()
                .iter()
                .zip(&gradient)
                .map(|(value, weight)| value * weight)
                .sum::<f64>();
            let minus_value = engine
                .forward(&minus_input, 3, Some(&initial))
                .expect("minus input forward")
                .final_activity()
                .iter()
                .zip(&gradient)
                .map(|(value, weight)| value * weight)
                .sum::<f64>();
            assert!(
                (actual.input[index] - (plus_value - minus_value) / (2.0 * epsilon)).abs() < 1.0e-5
            );

            let mut plus_initial = initial.clone();
            plus_initial[index] += epsilon;
            let mut minus_initial = initial.clone();
            minus_initial[index] -= epsilon;
            let plus_value = engine
                .forward(&input, 3, Some(&plus_initial))
                .expect("plus initial forward")
                .final_activity()
                .iter()
                .zip(&gradient)
                .map(|(value, weight)| value * weight)
                .sum::<f64>();
            let minus_value = engine
                .forward(&input, 3, Some(&minus_initial))
                .expect("minus initial forward")
                .final_activity()
                .iter()
                .zip(&gradient)
                .map(|(value, weight)| value * weight)
                .sum::<f64>();
            assert!(
                (actual.initial_activity[index] - (plus_value - minus_value) / (2.0 * epsilon))
                    .abs()
                    < 1.0e-5
            );
        }
        let unreachable = RateBackward::backward(&engine, &forward, &[0.0, 0.0, 0.0, 0.0, 1.0])
            .expect("unreachable readout backward");
        assert!(
            unreachable
                .log_gain
                .iter()
                .all(|value| value.abs() < 1.0e-12)
        );
        assert!(
            unreachable.input[..4]
                .iter()
                .all(|value| value.abs() < 1.0e-12)
        );
    }

    #[test]
    fn f32_forward_matches_f64_reference_away_from_clip_boundaries() {
        let graph = edge_case_graph();
        let mut parameters = RateParameters::initial(&graph);
        parameters.bias.fill(2.0);
        parameters.alpha_logits.fill((0.4_f64 / 0.6).ln());
        let engine = RateEngine::new(graph.clone(), parameters.clone()).expect("engine");
        let input_f64 = vec![0.1, 0.0, 0.1, 0.0, 0.0];
        let input_f32 = input_f64
            .iter()
            .map(|value| *value as f32)
            .collect::<Vec<_>>();
        let f64_forward = engine.forward(&input_f64, 3, None).expect("f64 forward");
        assert!(
            f64_forward
                .preactivation_history
                .iter()
                .flatten()
                .all(|value| *value > 1.0e-3 && *value < parameters.hmax - 1.0e-3)
        );
        let expected =
            reference_forward(&graph, &parameters, &input_f64, 3, None).expect("f64 reference");
        let actual_forward = engine
            .forward_f32(&input_f32, 3, None)
            .expect("f32 forward");
        for (actual, expected) in actual_forward.final_activity().iter().zip(expected) {
            assert!((*actual as f64 - expected).abs() < 2.0e-5);
        }
    }

    #[test]
    fn incoming_csr_must_be_a_complete_transpose() {
        let valid = edge_case_graph();
        let mut duplicated = valid.incoming_edges.clone();
        duplicated[1].edge_id = duplicated[0].edge_id;
        assert!(
            RateGraph::new(
                valid.root_ids.clone(),
                valid.outgoing_offsets.clone(),
                valid.outgoing_edges.clone(),
                valid.incoming_offsets.clone(),
                duplicated,
                valid.source_sign.clone(),
                valid.neuron_metadata.clone(),
                valid.populations.clone(),
            )
            .is_err()
        );

        let mut wrong_source = valid.incoming_edges.clone();
        wrong_source[0].source = 1;
        assert!(
            RateGraph::new(
                valid.root_ids.clone(),
                valid.outgoing_offsets.clone(),
                valid.outgoing_edges.clone(),
                valid.incoming_offsets.clone(),
                wrong_source,
                valid.source_sign.clone(),
                valid.neuron_metadata.clone(),
                valid.populations.clone(),
            )
            .is_err()
        );

        let mut wrong_target = valid.incoming_edges.clone();
        wrong_target[0].edge_id = 1;
        assert!(
            RateGraph::new(
                valid.root_ids,
                valid.outgoing_offsets,
                valid.outgoing_edges,
                valid.incoming_offsets,
                wrong_target,
                valid.source_sign,
                valid.neuron_metadata,
                valid.populations,
            )
            .is_err()
        );
    }

    #[test]
    fn rate_pack_round_trip_preserves_both_csrs() {
        let graph = graph();
        let directory = std::env::current_dir()
            .expect("current directory")
            .join("target")
            .join(format!("habitua-rate-pack-test-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("pack directory should be writable");
        let manifest = RatePackManifest {
            format_version: 0,
            dataset_version: "test".to_owned(),
            source_commit: "test".to_owned(),
            generated_at: "test".to_owned(),
            license: "test".to_owned(),
            neuron_count: 0,
            edge_count: 0,
            contact_count: 0,
            neurons_sha256: String::new(),
            outgoing_sha256: String::new(),
            incoming_sha256: String::new(),
            populations_sha256: String::new(),
            normalization: "receiver_total_incoming_contact_count".to_owned(),
            sign_rule: "test".to_owned(),
            exclusion_reasons: BTreeMap::new(),
            sign_edge_counts: BTreeMap::new(),
            raw_edge_row_count: 0,
            retained_raw_edge_row_count: 0,
            connectome_row_group_count: 0,
            row_group_limit: None,
            external_sort_run_count: 0,
            conversion_elapsed_ms: 0.0,
            peak_rss_bytes: None,
            source_files: Vec::new(),
            intermediate_files: Vec::new(),
        };
        graph
            .write_pack(&directory, &manifest)
            .expect("pack should write");
        let loaded = RateGraph::load(&directory).expect("pack should load");
        assert_eq!(loaded, graph);
        fs::remove_dir_all(directory).expect("pack should be removable");
    }

    #[test]
    fn rate_pack_loader_reads_the_checked_in_real_v1_fixture() {
        let fixture: RateFixture =
            serde_json::from_str(include_str!("../tests/fixtures/rate-v1-fixture.json"))
                .expect("rate v1 fixture JSON");
        assert_eq!(fixture.format_version, LEGACY_RATE_PACK_FORMAT_VERSION);
        let root_ids = fixture
            .neuron_metadata
            .iter()
            .map(|metadata| metadata.body_id)
            .collect::<Vec<_>>();
        let root_to_index = root_ids
            .iter()
            .enumerate()
            .map(|(index, root)| (*root, index as u32))
            .collect::<BTreeMap<_, _>>();
        let source_sign = fixture
            .neuron_metadata
            .iter()
            .map(|metadata| metadata.source_sign)
            .collect::<Vec<_>>();
        let outgoing_edges = fixture
            .outgoing_edges
            .into_iter()
            .map(|edge| RateOutgoingEdge {
                target: edge.target,
                contact_count: edge.contact_count,
            })
            .collect::<Vec<_>>();
        let incoming_edges = fixture
            .incoming_edges
            .into_iter()
            .map(|edge| RateIncomingEdge {
                source: edge.source,
                edge_id: edge.edge_id,
            })
            .collect::<Vec<_>>();
        let population_roots = fixture.populations;
        let population_indices = population_roots
            .iter()
            .map(|(name, roots)| {
                (
                    name.clone(),
                    roots
                        .iter()
                        .map(|root| root_to_index[root])
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let graph = RateGraph::new(
            root_ids,
            fixture.outgoing_offsets,
            outgoing_edges,
            fixture.incoming_offsets,
            incoming_edges,
            source_sign,
            fixture.neuron_metadata,
            population_indices,
        )
        .expect("fixture graph");
        let directory = std::env::current_dir()
            .expect("current directory")
            .join("target")
            .join(format!("habitua-rate-v1-fixture-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("fixture directory");
        let neurons = serde_json::to_vec_pretty(&graph.neuron_metadata).expect("neurons JSON");
        let populations = serde_json::to_vec_pretty(&RatePopulationFile {
            format_version: LEGACY_RATE_NEURON_FORMAT_VERSION,
            populations: population_roots,
        })
        .expect("populations JSON");
        let outgoing =
            encode_outgoing_with_version(&graph, fixture.format_version).expect("v1 outgoing");
        let incoming =
            encode_incoming_with_version(&graph, fixture.format_version).expect("v1 incoming");
        fs::write(directory.join("rate-neurons.json"), &neurons).expect("neurons");
        fs::write(directory.join("rate-populations.json"), &populations).expect("populations");
        fs::write(directory.join("rate-out.bin"), &outgoing).expect("outgoing");
        fs::write(directory.join("rate-in.bin"), &incoming).expect("incoming");
        let manifest = RatePackManifest {
            format_version: fixture.format_version,
            dataset_version: "fixture-v1".to_owned(),
            source_commit: "fixture".to_owned(),
            generated_at: "fixture".to_owned(),
            license: "test".to_owned(),
            neuron_count: graph.neuron_count(),
            edge_count: graph.edge_count(),
            contact_count: graph
                .outgoing_edges
                .iter()
                .map(|edge| edge.contact_count)
                .sum(),
            neurons_sha256: sha256_hex(&neurons),
            outgoing_sha256: sha256_hex(&outgoing),
            incoming_sha256: sha256_hex(&incoming),
            populations_sha256: sha256_hex(&populations),
            normalization: "receiver_total_incoming_contact_count".to_owned(),
            sign_rule: "fixture".to_owned(),
            exclusion_reasons: BTreeMap::new(),
            sign_edge_counts: BTreeMap::new(),
            raw_edge_row_count: graph.edge_count() as u64,
            retained_raw_edge_row_count: graph.edge_count() as u64,
            connectome_row_group_count: 1,
            row_group_limit: None,
            external_sort_run_count: 1,
            conversion_elapsed_ms: 0.0,
            peak_rss_bytes: None,
            source_files: Vec::new(),
            intermediate_files: Vec::new(),
        };
        fs::write(
            directory.join("rate_manifest.json"),
            serde_json::to_vec_pretty(&manifest).expect("manifest"),
        )
        .expect("manifest write");
        let loaded = RateGraph::load(&directory).expect("checked-in v1 pack should load");
        assert_eq!(loaded, graph);
        assert_eq!(loaded.population("input").expect("input"), &[0]);
        assert_eq!(loaded.population("readout").expect("readout"), &[2]);
        fs::remove_dir_all(directory).expect("fixture directory removal");
    }

    #[test]
    fn rate_pack_loader_accepts_a_version_one_fixture() {
        let graph = graph();
        let directory = std::env::current_dir()
            .expect("current directory")
            .join("target")
            .join(format!("habitua-rate-pack-v1-test-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("pack directory should be writable");
        let manifest = RatePackManifest {
            format_version: 0,
            dataset_version: "test".to_owned(),
            source_commit: "test".to_owned(),
            generated_at: "test".to_owned(),
            license: "test".to_owned(),
            neuron_count: 0,
            edge_count: 0,
            contact_count: 0,
            neurons_sha256: String::new(),
            outgoing_sha256: String::new(),
            incoming_sha256: String::new(),
            populations_sha256: String::new(),
            normalization: "receiver_total_incoming_contact_count".to_owned(),
            sign_rule: "test".to_owned(),
            exclusion_reasons: BTreeMap::new(),
            sign_edge_counts: BTreeMap::new(),
            raw_edge_row_count: 0,
            retained_raw_edge_row_count: 0,
            connectome_row_group_count: 0,
            row_group_limit: None,
            external_sort_run_count: 0,
            conversion_elapsed_ms: 0.0,
            peak_rss_bytes: None,
            source_files: Vec::new(),
            intermediate_files: Vec::new(),
        };
        graph
            .write_pack(&directory, &manifest)
            .expect("v2 pack should write");
        let mut outgoing = fs::read(directory.join("rate-out.bin")).expect("outgoing");
        outgoing[8..10].copy_from_slice(&LEGACY_RATE_PACK_FORMAT_VERSION.to_le_bytes());
        let mut incoming = fs::read(directory.join("rate-in.bin")).expect("incoming");
        incoming[8..10].copy_from_slice(&LEGACY_RATE_PACK_FORMAT_VERSION.to_le_bytes());
        fs::write(directory.join("rate-out.bin"), &outgoing).expect("outgoing rewrite");
        fs::write(directory.join("rate-in.bin"), &incoming).expect("incoming rewrite");
        let mut populations: serde_json::Value = serde_json::from_slice(
            &fs::read(directory.join("rate-populations.json")).expect("populations"),
        )
        .expect("populations JSON");
        populations["format_version"] = serde_json::json!(LEGACY_RATE_NEURON_FORMAT_VERSION);
        let populations = serde_json::to_vec_pretty(&populations).expect("populations rewrite");
        fs::write(directory.join("rate-populations.json"), &populations)
            .expect("populations write");
        let mut manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(directory.join("rate_manifest.json")).expect("manifest"),
        )
        .expect("manifest JSON");
        manifest["format_version"] = serde_json::json!(LEGACY_RATE_PACK_FORMAT_VERSION);
        manifest["outgoing_sha256"] = serde_json::json!(sha256_hex(&outgoing));
        manifest["incoming_sha256"] = serde_json::json!(sha256_hex(&incoming));
        manifest["populations_sha256"] = serde_json::json!(sha256_hex(&populations));
        fs::write(
            directory.join("rate_manifest.json"),
            serde_json::to_vec_pretty(&manifest).expect("manifest rewrite"),
        )
        .expect("manifest write");
        let loaded = RateGraph::load(&directory).expect("v1 pack should load");
        assert_eq!(loaded, graph);
        fs::remove_dir_all(directory).expect("v1 test pack should be removable");
    }
}
