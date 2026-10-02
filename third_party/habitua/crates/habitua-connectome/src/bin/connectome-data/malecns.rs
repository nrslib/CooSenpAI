use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use habitua_connectome::{
    RateGraph, RateIncomingEdge, RateNeuronMetadata, RateOutgoingEdge, RatePackManifest, SourceFile,
};
use parquet::column::reader::ColumnReader;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;
use sha2::{Digest, Sha256};

const DATASET_VERSION: &str = "MaleCNS v1.0";
const DATA_URL: &str =
    "https://storage.googleapis.com/flyem-male-cns/v1.0/connectome-data/flat-connectome/";
const LICENSE_URL: &str = "https://male-cns.janelia.org/download/";
const SOURCE_COMMIT: &str = "MaleCNS v1.0";

type PopulationRoots = BTreeMap<String, BTreeSet<u64>>;

#[derive(Clone, Debug)]
struct Annotation {
    type_name: Option<String>,
    class: Option<String>,
    superclass: Option<String>,
    subclass: Option<String>,
    soma_side: Option<String>,
    root_side: Option<String>,
    assigned_ol_hex1: Option<f64>,
    assigned_ol_hex2: Option<f64>,
}

#[derive(Clone, Debug)]
struct NtPrediction {
    consensus_nt: Option<String>,
    celltype_predicted_nt: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SignPolicy {
    Positive,
    Negative,
    Zero,
    Error,
}

impl SignPolicy {
    fn parse(value: &str, flag: &str) -> Result<Self, Box<dyn Error>> {
        match value {
            "positive" => Ok(Self::Positive),
            "negative" => Ok(Self::Negative),
            "zero" => Ok(Self::Zero),
            "error" => Ok(Self::Error),
            _ => Err(format!(
                "invalid {flag}={value}; expected positive, negative, zero, or error"
            )
            .into()),
        }
    }

    fn apply(self, flag: &str, label: &str) -> Result<i32, Box<dyn Error>> {
        match self {
            Self::Positive => Ok(1),
            Self::Negative => Ok(-1),
            Self::Zero => Ok(0),
            Self::Error => Err(format!("{flag} rejects neurotransmitter {label}").into()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
            Self::Zero => "zero",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SignCounters {
    consensus_nt_edge_count: usize,
    celltype_predicted_nt_edge_count: usize,
    default_edge_count: usize,
    unknown_edge_count: usize,
    other_edge_count: usize,
    positive_edge_count: usize,
    negative_edge_count: usize,
    zero_edge_count: usize,
    acetylcholine_edge_count: usize,
    gaba_edge_count: usize,
    glutamate_edge_count: usize,
    histamine_edge_count: usize,
}

const RATE_SORT_CHUNK_RECORDS: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RateRawEdge {
    source: u64,
    target: u64,
    contact_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RateReverseEdge {
    target: u64,
    source: u64,
    contact_count: u64,
    edge_id: u32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RateRunHead<T: Ord> {
    record: T,
    run_index: usize,
}

#[derive(Default)]
struct RateConversionCounters {
    raw_edge_row_count: u64,
    retained_raw_edge_row_count: u64,
    connectome_row_group_count: u64,
    contact_count: u64,
    exclusion_reasons: BTreeMap<String, u64>,
    sign_counters: SignCounters,
}

impl RateConversionCounters {
    fn excluded(&mut self, reason: &str) {
        *self.exclusion_reasons.entry(reason.to_owned()).or_default() += 1;
    }
}

/// Converts the complete MaleCNS graph into the independent rate-model pack.
pub fn run_rate(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    let values = parse_values(&mut args)?;
    let annotations_path = required_path(&values, "--annotations")?;
    let neurotransmitters_path = required_path(&values, "--neurotransmitters")?;
    let connectome_path = required_path(&values, "--connectome")?;
    let source_annotations = required_path(&values, "--source-annotations")?;
    let source_neurotransmitters = required_path(&values, "--source-neurotransmitters")?;
    let source_connectome = required_path(&values, "--source-connectome")?;
    let output = required_path(&values, "--output")?;
    let generated_at = required_value(&values, "--generated-at")?;
    let max_row_groups = values
        .get("--max-row-groups")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("invalid --max-row-groups={value}: {error}"))
        })
        .transpose()?;
    if max_row_groups == Some(0) {
        return Err("--max-row-groups must be positive when supplied".into());
    }
    let peak_rss_bytes = values
        .get("--peak-rss-bytes")
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("invalid --peak-rss-bytes={value}: {error}"))
        })
        .transpose()?;
    let other_policy = SignPolicy::parse(
        values
            .get("--other-nt")
            .map(String::as_str)
            .unwrap_or("positive"),
        "--other-nt",
    )?;
    let unknown_policy = SignPolicy::parse(
        values
            .get("--unknown-nt")
            .map(String::as_str)
            .unwrap_or("positive"),
        "--unknown-nt",
    )?;
    let unknown = values.keys().find(|name| {
        !matches!(
            name.as_str(),
            "--annotations"
                | "--neurotransmitters"
                | "--connectome"
                | "--source-annotations"
                | "--source-neurotransmitters"
                | "--source-connectome"
                | "--output"
                | "--generated-at"
                | "--max-row-groups"
                | "--sort-dir"
                | "--peak-rss-bytes"
                | "--other-nt"
                | "--unknown-nt"
        )
    });
    if let Some(unknown) = unknown {
        return Err(format!("unknown argument {unknown}").into());
    }

    let started = Instant::now();
    let annotations = read_annotations(&annotations_path)?;
    let neurotransmitters = read_neurotransmitters(&neurotransmitters_path)?;
    let mut counters = RateConversionCounters::default();
    let sort_root = values
        .get("--sort-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            output
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(".rate-sort-work")
        });
    let temp_directory = sort_root.join(format!(
        "rate-sort-{}-{}",
        std::process::id(),
        started.elapsed().as_nanos()
    ));
    fs::create_dir_all(&temp_directory)?;
    let result = (|| -> Result<(), Box<dyn Error>> {
        let adopted_ids = annotations.keys().copied().collect::<BTreeSet<_>>();
        let mut endpoint_set = BTreeSet::new();
        let run_paths = write_rate_sort_runs(
            &connectome_path,
            &temp_directory,
            &neurotransmitters,
            &mut counters,
            other_policy,
            unknown_policy,
            &adopted_ids,
            &mut endpoint_set,
            max_row_groups,
        )?;
        if adopted_ids.is_empty() {
            return Err("MaleCNS rate conversion found no adopted annotation IDs".into());
        }
        let annotation_rows = annotations.len() as u64;
        let isolated_annotations = annotations
            .keys()
            .filter(|body_id| !endpoint_set.contains(body_id))
            .count() as u64;
        counters.exclusion_reasons.insert(
            "annotation_rows_without_connectome_endpoint".to_owned(),
            isolated_annotations,
        );
        counters
            .exclusion_reasons
            .insert("annotation_rows".to_owned(), annotation_rows);
        let root_ids = adopted_ids.into_iter().collect::<Vec<_>>();
        let root_to_index = root_ids
            .iter()
            .enumerate()
            .map(|(index, body_id)| u32::try_from(index).map(|index| (*body_id, index)))
            .collect::<Result<HashMap<_, _>, _>>()?;
        let aggregate_path = temp_directory.join("aggregated.bin");
        merge_rate_runs(&run_paths, &aggregate_path)?;
        let reverse_run_directory = temp_directory.join("reverse");
        fs::create_dir_all(&reverse_run_directory)?;
        let graph = materialize_rate_graph(
            &aggregate_path,
            &reverse_run_directory,
            &root_ids,
            &root_to_index,
            &annotations,
            &neurotransmitters,
            other_policy,
            unknown_policy,
            &mut counters,
        )?;
        let source_files = vec![
            source_file(
                &source_annotations,
                "body-annotations-male-cns-v1.0-minconf-0.5.feather",
                DATA_URL,
            )?,
            source_file(
                &source_neurotransmitters,
                "body-neurotransmitters-male-cns-v1.0.feather",
                DATA_URL,
            )?,
            source_file(
                &source_connectome,
                "connectome-weights-male-cns-v1.0-minconf-0.5.feather",
                DATA_URL,
            )?,
        ];
        let intermediate_files = vec![
            source_file(
                &annotations_path,
                "annotations.parquet",
                "local intermediate",
            )?,
            source_file(
                &neurotransmitters_path,
                "neurotransmitters.parquet",
                "local intermediate",
            )?,
            source_file(&connectome_path, "connectome.parquet", "local intermediate")?,
        ];
        let manifest = RatePackManifest {
            format_version: 0,
            dataset_version: DATASET_VERSION.to_owned(),
            source_commit: SOURCE_COMMIT.to_owned(),
            generated_at: generated_at.clone(),
            license: "CC-BY 4.0".to_owned(),
            neuron_count: 0,
            edge_count: 0,
            contact_count: 0,
            neurons_sha256: String::new(),
            outgoing_sha256: String::new(),
            incoming_sha256: String::new(),
            populations_sha256: String::new(),
            normalization: "D_i=max(1,sum_j contact_count[j->i]); w_ij=contact_count/D_i"
                .to_owned(),
            sign_rule: format!(
                "consensus_nt then celltype_predicted_nt; acetylcholine=+1, GABA=-1, glutamate=-1, histamine=-1; other={}; unknown={}",
                other_policy.as_str(),
                unknown_policy.as_str()
            ),
            exclusion_reasons: counters.exclusion_reasons.clone(),
            sign_edge_counts: sign_edge_counts(&counters.sign_counters),
            raw_edge_row_count: counters.raw_edge_row_count,
            retained_raw_edge_row_count: counters.retained_raw_edge_row_count,
            connectome_row_group_count: counters.connectome_row_group_count,
            row_group_limit: max_row_groups,
            external_sort_run_count: run_paths.len() as u64,
            conversion_elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
            peak_rss_bytes,
            source_files,
            intermediate_files,
        };
        graph.write_pack(&output, &manifest)?;
        println!(
            "wrote rate pack neurons={} edges={} contacts={} row_groups={} sort_runs={} elapsed_ms={:.3} to {}",
            graph.neuron_count(),
            graph.edge_count(),
            counters.contact_count,
            counters.connectome_row_group_count,
            run_paths.len(),
            started.elapsed().as_secs_f64() * 1_000.0,
            output.display()
        );
        Ok(())
    })();
    let _ = fs::remove_dir_all(&temp_directory);
    result
}

fn sign_edge_counts(counters: &SignCounters) -> BTreeMap<String, u64> {
    BTreeMap::from([
        (
            "consensus_nt".to_owned(),
            counters.consensus_nt_edge_count as u64,
        ),
        (
            "celltype_predicted_nt".to_owned(),
            counters.celltype_predicted_nt_edge_count as u64,
        ),
        ("default".to_owned(), counters.default_edge_count as u64),
        ("unknown".to_owned(), counters.unknown_edge_count as u64),
        ("other".to_owned(), counters.other_edge_count as u64),
        ("positive".to_owned(), counters.positive_edge_count as u64),
        ("negative".to_owned(), counters.negative_edge_count as u64),
        ("zero".to_owned(), counters.zero_edge_count as u64),
        (
            "acetylcholine".to_owned(),
            counters.acetylcholine_edge_count as u64,
        ),
        ("gaba".to_owned(), counters.gaba_edge_count as u64),
        ("glutamate".to_owned(), counters.glutamate_edge_count as u64),
        ("histamine".to_owned(), counters.histamine_edge_count as u64),
    ])
}

fn for_each_valid_rate_row(
    path: &Path,
    counters: &mut RateConversionCounters,
    max_row_groups: Option<u64>,
    mut callback: impl FnMut(u64, u64, u64) -> Result<(), Box<dyn Error>>,
) -> Result<(), Box<dyn Error>> {
    let reader = SerializedFileReader::new(File::open(path)?)?;
    let columns = column_indices(&reader, &["body_pre", "body_post", "weight"])?;
    let column_order = [columns["body_pre"], columns["body_post"], columns["weight"]];
    for row_group_index in 0..reader.num_row_groups() {
        if max_row_groups.is_some_and(|limit| counters.connectome_row_group_count >= limit) {
            counters.excluded("row_group_limit_reached");
            break;
        }
        counters.connectome_row_group_count += 1;
        let row_group = reader.get_row_group(row_group_index)?;
        let mut source_reader = row_group.get_column_reader(column_order[0])?;
        let mut target_reader = row_group.get_column_reader(column_order[1])?;
        let mut weight_reader = row_group.get_column_reader(column_order[2])?;
        loop {
            let sources = read_required_i64_chunk(&mut source_reader, column_order[0])?;
            let targets = read_required_i64_chunk(&mut target_reader, column_order[1])?;
            let weights = read_required_i64_chunk(&mut weight_reader, column_order[2])?;
            if sources.is_empty() && targets.is_empty() && weights.is_empty() {
                break;
            }
            if sources.len() != targets.len() || sources.len() != weights.len() {
                return Err(format!(
                    "edge Parquet row group {row_group_index} has mismatched chunk lengths"
                )
                .into());
            }
            for ((source, target), weight) in sources.iter().zip(&targets).zip(&weights) {
                counters.raw_edge_row_count += 1;
                let source = match u64::try_from(*source) {
                    Ok(value) => value,
                    Err(_) => {
                        counters.excluded("negative_body_id");
                        continue;
                    }
                };
                let target = match u64::try_from(*target) {
                    Ok(value) => value,
                    Err(_) => {
                        counters.excluded("negative_body_id");
                        continue;
                    }
                };
                let contact_count = match u64::try_from(*weight) {
                    Ok(value) if value > 0 => value,
                    Ok(_) => {
                        counters.excluded("zero_contact_weight");
                        continue;
                    }
                    Err(_) => {
                        counters.excluded("negative_contact_weight");
                        continue;
                    }
                };
                callback(source, target, contact_count)?;
            }
        }
    }
    Ok(())
}

fn read_required_i64_chunk(
    reader: &mut ColumnReader,
    column_index: usize,
) -> Result<Vec<i64>, Box<dyn Error>> {
    let mut values = Vec::new();
    let mut def_levels = Vec::new();
    let mut rep_levels = Vec::new();
    match reader {
        ColumnReader::Int64ColumnReader(reader) => {
            let (records_read, values_read, levels_read) = reader.read_records(
                100_000,
                Some(&mut def_levels),
                Some(&mut rep_levels),
                &mut values,
            )?;
            if records_read != values_read
                || (records_read == 0 && values_read == 0 && levels_read != 0)
            {
                return Err(format!(
                    "Parquet column {column_index} contains nullable or repeated values"
                )
                .into());
            }
        }
        _ => {
            return Err(format!("Parquet column {column_index} is not INT64").into());
        }
    }
    if rep_levels.iter().any(|level| *level != 0) {
        return Err(format!("Parquet column {column_index} is repeated").into());
    }
    Ok(values)
}

#[allow(clippy::too_many_arguments)]
fn write_rate_sort_runs(
    connectome_path: &Path,
    directory: &Path,
    neurotransmitters: &HashMap<u64, NtPrediction>,
    counters: &mut RateConversionCounters,
    other_policy: SignPolicy,
    unknown_policy: SignPolicy,
    adopted_ids: &BTreeSet<u64>,
    endpoint_set: &mut BTreeSet<u64>,
    max_row_groups: Option<u64>,
) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut records = Vec::with_capacity(RATE_SORT_CHUNK_RECORDS);
    let mut run_paths = Vec::new();
    let mut sign_counters = SignCounters::default();
    let mut excluded_unadopted = 0_u64;
    let mut accepted_raw_edge_row_count = 0_u64;
    let mut accepted_contact_count = 0_u64;
    for_each_valid_rate_row(
        connectome_path,
        counters,
        max_row_groups,
        |source, target, contact_count| {
            if !adopted_ids.contains(&source) || !adopted_ids.contains(&target) {
                excluded_unadopted += 1;
                return Ok(());
            }
            endpoint_set.insert(source);
            endpoint_set.insert(target);
            accepted_raw_edge_row_count = accepted_raw_edge_row_count
                .checked_add(1)
                .ok_or("accepted raw edge count overflowed")?;
            accepted_contact_count = accepted_contact_count
                .checked_add(contact_count)
                .ok_or("accepted contact count overflowed")?;
            resolve_sign(
                source,
                neurotransmitters,
                other_policy,
                unknown_policy,
                &mut sign_counters,
            )?;
            records.push(RateRawEdge {
                source,
                target,
                contact_count,
            });
            if records.len() == RATE_SORT_CHUNK_RECORDS {
                run_paths.push(flush_rate_run(directory, run_paths.len(), &mut records)?);
            }
            Ok(())
        },
    )?;
    if excluded_unadopted > 0 {
        counters.exclusion_reasons.insert(
            "endpoint_not_in_adopted_annotations".to_owned(),
            excluded_unadopted,
        );
    }
    counters.retained_raw_edge_row_count = accepted_raw_edge_row_count;
    counters.contact_count = accepted_contact_count;
    if !records.is_empty() {
        run_paths.push(flush_rate_run(directory, run_paths.len(), &mut records)?);
    }
    counters.sign_counters = sign_counters;
    Ok(run_paths)
}

fn flush_rate_run(
    directory: &Path,
    index: usize,
    records: &mut Vec<RateRawEdge>,
) -> Result<PathBuf, Box<dyn Error>> {
    records.sort_unstable();
    let mut aggregated: Vec<RateRawEdge> = Vec::with_capacity(records.len());
    for record in records.drain(..) {
        if let Some(previous) = aggregated.last_mut()
            && previous.source == record.source
            && previous.target == record.target
        {
            previous.contact_count = previous
                .contact_count
                .checked_add(record.contact_count)
                .ok_or("sort-run contact count overflowed")?;
        } else {
            aggregated.push(record);
        }
    }
    let path = directory.join(format!("rate-run-{index:06}.bin"));
    let file = File::create(&path)?;
    let mut writer = BufWriter::new(file);
    for record in aggregated {
        write_rate_raw_edge(&mut writer, record)?;
    }
    writer.flush()?;
    Ok(path)
}

fn merge_rate_runs(run_paths: &[PathBuf], output: &Path) -> Result<(), Box<dyn Error>> {
    let mut readers = run_paths
        .iter()
        .map(|path| File::open(path).map(BufReader::new))
        .collect::<Result<Vec<_>, _>>()?;
    let mut heap = BinaryHeap::new();
    for (run_index, reader) in readers.iter_mut().enumerate() {
        if let Some(record) = read_rate_raw_edge(reader)? {
            heap.push(Reverse(RateRunHead { record, run_index }));
        }
    }
    let mut writer = BufWriter::new(File::create(output)?);
    let mut current: Option<RateRawEdge> = None;
    while let Some(Reverse(head)) = heap.pop() {
        if let Some(next) = read_rate_raw_edge(&mut readers[head.run_index])? {
            heap.push(Reverse(RateRunHead {
                record: next,
                run_index: head.run_index,
            }));
        }
        if let Some(previous) = current.as_mut()
            && previous.source == head.record.source
            && previous.target == head.record.target
        {
            previous.contact_count = previous
                .contact_count
                .checked_add(head.record.contact_count)
                .ok_or("merged contact count overflowed")?;
        } else {
            if let Some(previous) = current.replace(head.record) {
                write_rate_raw_edge(&mut writer, previous)?;
            }
        }
    }
    if let Some(previous) = current {
        write_rate_raw_edge(&mut writer, previous)?;
    }
    writer.flush()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn materialize_rate_graph(
    aggregate_path: &Path,
    reverse_directory: &Path,
    root_ids: &[u64],
    root_to_index: &HashMap<u64, u32>,
    annotations: &BTreeMap<u64, Annotation>,
    neurotransmitters: &HashMap<u64, NtPrediction>,
    other_policy: SignPolicy,
    unknown_policy: SignPolicy,
    counters: &mut RateConversionCounters,
) -> Result<RateGraph, Box<dyn Error>> {
    let neuron_count = root_ids.len();
    let mut outgoing_counts = vec![0_u64; neuron_count];
    let mut incoming_counts = vec![0_u64; neuron_count];
    let mut aggregate_reader = BufReader::new(File::open(aggregate_path)?);
    while let Some(record) = read_rate_raw_edge(&mut aggregate_reader)? {
        let source = root_to_index
            .get(&record.source)
            .copied()
            .ok_or("aggregated source body ID is not in the selected IDs")?;
        let target = root_to_index
            .get(&record.target)
            .copied()
            .ok_or("aggregated target body ID is not in the selected IDs")?;
        outgoing_counts[source as usize] += 1;
        incoming_counts[target as usize] += 1;
    }
    let outgoing_offsets = prefix_offsets(&outgoing_counts)?;
    let incoming_offsets = prefix_offsets(&incoming_counts)?;
    let mut outgoing_edges = Vec::with_capacity(outgoing_offsets[neuron_count] as usize);
    let mut reverse_records = Vec::with_capacity(RATE_SORT_CHUNK_RECORDS);
    let mut reverse_runs = Vec::new();
    let mut aggregate_reader = BufReader::new(File::open(aggregate_path)?);
    let mut edge_id = 0_u32;
    while let Some(record) = read_rate_raw_edge(&mut aggregate_reader)? {
        let source = root_to_index[&record.source];
        let target = root_to_index[&record.target];
        outgoing_edges.push(RateOutgoingEdge {
            target,
            contact_count: record.contact_count,
        });
        reverse_records.push(RateReverseEdge {
            target: u64::from(target),
            source: u64::from(source),
            contact_count: record.contact_count,
            edge_id,
        });
        edge_id = edge_id.checked_add(1).ok_or("rate edge ID overflowed")?;
        if reverse_records.len() == RATE_SORT_CHUNK_RECORDS {
            reverse_runs.push(flush_reverse_run(
                reverse_directory,
                reverse_runs.len(),
                &mut reverse_records,
            )?);
        }
    }
    if !reverse_records.is_empty() {
        reverse_runs.push(flush_reverse_run(
            reverse_directory,
            reverse_runs.len(),
            &mut reverse_records,
        )?);
    }
    let incoming_edges = merge_reverse_runs(&reverse_runs)?;
    let mut source_sign = Vec::with_capacity(neuron_count);
    let mut metadata = Vec::with_capacity(neuron_count);
    for body_id in root_ids {
        let annotation = annotations
            .get(body_id)
            .ok_or("selected rate body ID has no annotation placeholder")?;
        let (selected_neurotransmitter, neurotransmitter_source) =
            selected_neurotransmitter(*body_id, neurotransmitters);
        let sign = sign_for_label(
            selected_neurotransmitter.as_deref(),
            other_policy,
            unknown_policy,
        )?;
        source_sign.push(sign as i8);
        metadata.push(RateNeuronMetadata {
            body_id: *body_id,
            type_name: annotation.type_name.clone(),
            class: annotation.class.clone(),
            superclass: annotation.superclass.clone(),
            subclass: annotation.subclass.clone(),
            soma_side: annotation.soma_side.clone(),
            root_side: annotation.root_side.clone(),
            assigned_ol_hex1: annotation.assigned_ol_hex1,
            assigned_ol_hex2: annotation.assigned_ol_hex2,
            selected_neurotransmitter,
            neurotransmitter_source,
            source_sign: sign as i8,
        });
    }
    let population_roots = resolve_populations(root_ids, annotations);
    let populations = population_roots
        .into_iter()
        .map(|(name, roots)| {
            let indices = roots
                .into_iter()
                .map(|root| {
                    root_to_index.get(&root).copied().ok_or_else(|| {
                        format!("rate population {name} references unknown body {root}")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok((name, indices))
        })
        .collect::<Result<BTreeMap<_, _>, Box<dyn Error>>>()?;
    let graph = RateGraph::new(
        root_ids.to_vec(),
        outgoing_offsets,
        outgoing_edges,
        incoming_offsets,
        incoming_edges,
        source_sign,
        metadata,
        populations,
    )?;
    counters.contact_count = graph
        .outgoing_edges
        .iter()
        .try_fold(0_u64, |sum, edge| sum.checked_add(edge.contact_count))
        .ok_or("aggregated contact count overflowed")?;
    Ok(graph)
}

fn prefix_offsets(counts: &[u64]) -> Result<Vec<u64>, Box<dyn Error>> {
    let mut offsets = Vec::with_capacity(counts.len() + 1);
    offsets.push(0);
    for count in counts {
        let next = offsets
            .last()
            .copied()
            .unwrap_or(0_u64)
            .checked_add(*count)
            .ok_or("rate CSR offset overflowed")?;
        offsets.push(next);
    }
    Ok(offsets)
}

fn flush_reverse_run(
    directory: &Path,
    index: usize,
    records: &mut Vec<RateReverseEdge>,
) -> Result<PathBuf, Box<dyn Error>> {
    records.sort_unstable();
    let path = directory.join(format!("rate-reverse-run-{index:06}.bin"));
    let mut writer = BufWriter::new(File::create(&path)?);
    for record in records.drain(..) {
        write_rate_reverse_edge(&mut writer, record)?;
    }
    writer.flush()?;
    Ok(path)
}

fn merge_reverse_runs(paths: &[PathBuf]) -> Result<Vec<RateIncomingEdge>, Box<dyn Error>> {
    let mut readers = paths
        .iter()
        .map(|path| File::open(path).map(BufReader::new))
        .collect::<Result<Vec<_>, _>>()?;
    let mut heap = BinaryHeap::new();
    for (run_index, reader) in readers.iter_mut().enumerate() {
        if let Some(record) = read_rate_reverse_edge(reader)? {
            heap.push(Reverse(RateRunHead { record, run_index }));
        }
    }
    let mut result = Vec::new();
    while let Some(Reverse(head)) = heap.pop() {
        result.push(RateIncomingEdge {
            source: u32::try_from(head.record.source).map_err(|_| "reverse source is too large")?,
            edge_id: head.record.edge_id,
        });
        if let Some(record) = read_rate_reverse_edge(&mut readers[head.run_index])? {
            heap.push(Reverse(RateRunHead {
                record,
                run_index: head.run_index,
            }));
        }
    }
    Ok(result)
}

fn write_rate_raw_edge(writer: &mut impl Write, record: RateRawEdge) -> Result<(), Box<dyn Error>> {
    writer.write_all(&record.source.to_le_bytes())?;
    writer.write_all(&record.target.to_le_bytes())?;
    writer.write_all(&record.contact_count.to_le_bytes())?;
    Ok(())
}

fn read_rate_raw_edge(reader: &mut impl Read) -> Result<Option<RateRawEdge>, Box<dyn Error>> {
    let Some(source) = read_optional_u64(reader)? else {
        return Ok(None);
    };
    let target = read_required_u64(reader)?;
    let contact_count = read_required_u64(reader)?;
    Ok(Some(RateRawEdge {
        source,
        target,
        contact_count,
    }))
}

fn write_rate_reverse_edge(
    writer: &mut impl Write,
    record: RateReverseEdge,
) -> Result<(), Box<dyn Error>> {
    writer.write_all(&record.target.to_le_bytes())?;
    writer.write_all(&record.source.to_le_bytes())?;
    writer.write_all(&record.contact_count.to_le_bytes())?;
    writer.write_all(&record.edge_id.to_le_bytes())?;
    Ok(())
}

fn read_rate_reverse_edge(
    reader: &mut impl Read,
) -> Result<Option<RateReverseEdge>, Box<dyn Error>> {
    let Some(target) = read_optional_u64(reader)? else {
        return Ok(None);
    };
    let source = read_required_u64(reader)?;
    let contact_count = read_required_u64(reader)?;
    let edge_id = read_required_u32(reader)?;
    Ok(Some(RateReverseEdge {
        target,
        source,
        contact_count,
        edge_id,
    }))
}

fn read_optional_u64(reader: &mut impl Read) -> Result<Option<u64>, Box<dyn Error>> {
    let mut bytes = [0_u8; 8];
    let mut read = 0;
    while read < bytes.len() {
        let count = reader.read(&mut bytes[read..])?;
        if count == 0 {
            if read == 0 {
                return Ok(None);
            }
            return Err("truncated external-sort record".into());
        }
        read += count;
    }
    Ok(Some(u64::from_le_bytes(bytes)))
}

fn read_required_u64(reader: &mut impl Read) -> Result<u64, Box<dyn Error>> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_required_u32(reader: &mut impl Read) -> Result<u32, Box<dyn Error>> {
    let mut bytes = [0_u8; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn parse_values(
    args: &mut impl Iterator<Item = String>,
) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let mut values = BTreeMap::new();
    while let Some(flag) = args.next() {
        if !flag.starts_with("--") {
            return Err(format!("unexpected argument {flag}").into());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("value is required for {flag}"))?;
        if value.starts_with('-') || values.insert(flag.clone(), value).is_some() {
            return Err(format!("invalid or duplicated argument {flag}").into());
        }
    }
    Ok(values)
}

fn required_value(values: &BTreeMap<String, String>, name: &str) -> Result<String, Box<dyn Error>> {
    values
        .get(name)
        .cloned()
        .ok_or_else(|| format!("required argument {name} is missing").into())
}

fn required_path(values: &BTreeMap<String, String>, name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let value = required_value(values, name)?;
    if value.is_empty() {
        return Err(format!("argument {name} must not be empty").into());
    }
    Ok(PathBuf::from(value))
}

fn read_annotations(path: &Path) -> Result<BTreeMap<u64, Annotation>, Box<dyn Error>> {
    let reader = SerializedFileReader::new(File::open(path)?)?;
    let columns = column_indices(
        &reader,
        &[
            "assigned_ol_hex1",
            "assigned_ol_hex2",
            "body_id",
            "type_name",
            "cell_class",
            "superclass",
            "subclass",
            "soma_side",
            "root_side",
        ],
    )?;
    let mut annotations = BTreeMap::new();
    for (row_number, row) in reader.get_row_iter(None)?.enumerate() {
        let row = row?;
        let fields = row.get_column_iter().collect::<Vec<_>>();
        let body_id = field_to_u64(fields[columns["body_id"]].1, "body_id", row_number)?;
        let annotation = Annotation {
            type_name: field_to_optional_string(
                fields[columns["type_name"]].1,
                "type_name",
                row_number,
            )?,
            class: field_to_optional_string(
                fields[columns["cell_class"]].1,
                "cell_class",
                row_number,
            )?,
            superclass: field_to_optional_string(
                fields[columns["superclass"]].1,
                "superclass",
                row_number,
            )?,
            subclass: field_to_optional_string(
                fields[columns["subclass"]].1,
                "subclass",
                row_number,
            )?,
            soma_side: field_to_optional_string(
                fields[columns["soma_side"]].1,
                "soma_side",
                row_number,
            )?,
            root_side: field_to_optional_string(
                fields[columns["root_side"]].1,
                "root_side",
                row_number,
            )?,
            assigned_ol_hex1: field_to_optional_f64(
                fields[columns["assigned_ol_hex1"]].1,
                "assigned_ol_hex1",
                row_number,
            )?,
            assigned_ol_hex2: field_to_optional_f64(
                fields[columns["assigned_ol_hex2"]].1,
                "assigned_ol_hex2",
                row_number,
            )?,
        };
        if annotations.insert(body_id, annotation).is_some() {
            return Err(format!("annotation table contains duplicate body ID {body_id}").into());
        }
    }
    Ok(annotations)
}

fn read_neurotransmitters(path: &Path) -> Result<HashMap<u64, NtPrediction>, Box<dyn Error>> {
    let reader = SerializedFileReader::new(File::open(path)?)?;
    let columns = column_indices(&reader, &["body", "consensus_nt", "celltype_predicted_nt"])?;
    let mut predictions = HashMap::new();
    for (row_number, row) in reader.get_row_iter(None)?.enumerate() {
        let row = row?;
        let fields = row.get_column_iter().collect::<Vec<_>>();
        let body_id = field_to_u64(fields[columns["body"]].1, "body", row_number)?;
        let prediction = NtPrediction {
            consensus_nt: field_to_optional_string(
                fields[columns["consensus_nt"]].1,
                "consensus_nt",
                row_number,
            )?,
            celltype_predicted_nt: field_to_optional_string(
                fields[columns["celltype_predicted_nt"]].1,
                "celltype_predicted_nt",
                row_number,
            )?,
        };
        if predictions.insert(body_id, prediction).is_some() {
            return Err(
                format!("neurotransmitter table contains duplicate body ID {body_id}").into(),
            );
        }
    }
    Ok(predictions)
}

fn column_indices(
    reader: &SerializedFileReader<File>,
    names: &[&str],
) -> Result<HashMap<String, usize>, Box<dyn Error>> {
    let columns = reader
        .metadata()
        .file_metadata()
        .schema_descr()
        .columns()
        .iter()
        .enumerate()
        .map(|(index, column)| (column.name().to_owned(), index))
        .collect::<HashMap<_, _>>();
    names
        .iter()
        .map(|name| {
            columns
                .get(*name)
                .copied()
                .map(|index| ((*name).to_owned(), index))
                .ok_or_else(|| format!("required Parquet column {name} is missing").into())
        })
        .collect()
}

fn field_to_u64(field: &Field, name: &str, row: usize) -> Result<u64, Box<dyn Error>> {
    let value = match field {
        Field::Byte(value) => i128::from(*value),
        Field::Short(value) => i128::from(*value),
        Field::Int(value) => i128::from(*value),
        Field::Long(value) => i128::from(*value),
        Field::UByte(value) => i128::from(*value),
        Field::UShort(value) => i128::from(*value),
        Field::UInt(value) => i128::from(*value),
        Field::ULong(value) => i128::from(*value),
        _ => return Err(format!("column {name} at row {row} is not an unsigned integer").into()),
    };
    u64::try_from(value).map_err(|error| format!("column {name} at row {row}: {error}").into())
}

fn field_to_optional_string(
    field: &Field,
    name: &str,
    row: usize,
) -> Result<Option<String>, Box<dyn Error>> {
    match field {
        Field::Null => Ok(None),
        Field::Str(value) => Ok((!value.trim().is_empty()).then(|| value.clone())),
        _ => Err(format!("column {name} at row {row} is not a string or null").into()),
    }
}

fn field_to_optional_f64(
    field: &Field,
    name: &str,
    row: usize,
) -> Result<Option<f64>, Box<dyn Error>> {
    match field {
        Field::Null => Ok(None),
        Field::Float(value) => Ok(Some(f64::from(*value))),
        Field::Double(value) => Ok(Some(*value)),
        _ => {
            Err(format!("column {name} at row {row} is not a floating-point value or null").into())
        }
    }
}

fn update_sign_total(counters: &mut SignCounters, sign: i32) {
    match sign {
        1 => counters.positive_edge_count += 1,
        -1 => counters.negative_edge_count += 1,
        0 => counters.zero_edge_count += 1,
        _ => unreachable!("sign resolution must return -1, 0, or 1"),
    }
}

fn resolve_sign(
    source: u64,
    neurotransmitters: &HashMap<u64, NtPrediction>,
    other_policy: SignPolicy,
    unknown_policy: SignPolicy,
    counters: &mut SignCounters,
) -> Result<i32, Box<dyn Error>> {
    let (label, source_rule) = selected_neurotransmitter(source, neurotransmitters);
    match source_rule.as_str() {
        "consensus_nt" => counters.consensus_nt_edge_count += 1,
        "celltype_predicted_nt" => counters.celltype_predicted_nt_edge_count += 1,
        _ => counters.default_edge_count += 1,
    }
    let Some(label) = label.as_deref() else {
        counters.unknown_edge_count += 1;
        let sign = unknown_policy.apply("--unknown-nt", "unknown")?;
        update_sign_total(counters, sign);
        return Ok(sign);
    };
    let normalized = label.trim().to_ascii_lowercase();
    let multiplier = match normalized.as_str() {
        "acetylcholine" => {
            counters.acetylcholine_edge_count += 1;
            1
        }
        "gaba" => {
            counters.gaba_edge_count += 1;
            -1
        }
        "glutamate" => {
            counters.glutamate_edge_count += 1;
            -1
        }
        "histamine" => {
            counters.histamine_edge_count += 1;
            -1
        }
        _ => {
            counters.other_edge_count += 1;
            other_policy.apply("--other-nt", &normalized)?
        }
    };
    update_sign_total(counters, multiplier);
    Ok(multiplier)
}

fn selected_neurotransmitter(
    source: u64,
    neurotransmitters: &HashMap<u64, NtPrediction>,
) -> (Option<String>, String) {
    let Some(prediction) = neurotransmitters.get(&source) else {
        return (None, "default".to_owned());
    };
    if usable_nt(prediction.consensus_nt.as_deref()) {
        return (prediction.consensus_nt.clone(), "consensus_nt".to_owned());
    }
    if usable_nt(prediction.celltype_predicted_nt.as_deref()) {
        return (
            prediction.celltype_predicted_nt.clone(),
            "celltype_predicted_nt".to_owned(),
        );
    }
    (None, "default".to_owned())
}

fn sign_for_label(
    label: Option<&str>,
    other_policy: SignPolicy,
    unknown_policy: SignPolicy,
) -> Result<i32, Box<dyn Error>> {
    let Some(label) = label else {
        return unknown_policy.apply("--unknown-nt", "unknown");
    };
    match label.trim().to_ascii_lowercase().as_str() {
        "acetylcholine" => Ok(1),
        "gaba" | "glutamate" | "histamine" => Ok(-1),
        other => other_policy.apply("--other-nt", other),
    }
}

fn usable_nt(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        let value = value.trim();
        !value.is_empty() && !value.eq_ignore_ascii_case("unclear")
    })
}

fn resolve_populations(
    root_ids: &[u64],
    annotations: &BTreeMap<u64, Annotation>,
) -> PopulationRoots {
    let mut populations = PopulationRoots::new();
    for body_id in root_ids {
        let annotation = annotations.get(body_id).expect("annotation exists");
        let Some(type_name) = annotation.type_name.as_deref() else {
            continue;
        };
        insert_population(&mut populations, type_name, *body_id);
        if is_kc(type_name) {
            insert_population(&mut populations, "kc", *body_id);
        }
        if type_name == "APL" {
            insert_population(&mut populations, "apl", *body_id);
        }
        if is_mbon(type_name) {
            insert_population(&mut populations, "mbon", *body_id);
        }
        if is_ppl1(type_name) {
            insert_population(&mut populations, "ppl1", *body_id);
        }
        if type_name.starts_with("PAM") {
            insert_population(&mut populations, "pam", *body_id);
        }
        if type_name.starts_with("ORN_") {
            insert_population(&mut populations, "orn", *body_id);
        }
        if is_pn(annotation) {
            insert_population(&mut populations, "pn", *body_id);
            if let Some(glomerulus) = type_name.split_once('_').map(|(name, _)| name)
                && !glomerulus.is_empty()
            {
                insert_population(
                    &mut populations,
                    &format!("pn/glomerulus/{glomerulus}"),
                    *body_id,
                );
            }
        }
        if is_photoreceptor(type_name) {
            insert_population(&mut populations, "photoreceptor", *body_id);
        }
        if type_name.starts_with("DN") {
            insert_population(&mut populations, "dn", *body_id);
        }
        if let Some(soma_side) = annotation.soma_side.as_deref() {
            insert_population(
                &mut populations,
                &format!("{type_name}/{soma_side}"),
                *body_id,
            );
            insert_population(&mut populations, &format!("soma/{soma_side}"), *body_id);
        }
    }
    populations
}

fn is_kc(type_name: &str) -> bool {
    type_name == "KC"
        || type_name.starts_with("KCab")
        || type_name.starts_with("KCa'b'")
        || type_name.starts_with("KCg")
}

fn is_mbon(type_name: &str) -> bool {
    type_name.starts_with("MBON")
}

fn is_ppl1(type_name: &str) -> bool {
    type_name.starts_with("PPL1")
}

fn is_photoreceptor(type_name: &str) -> bool {
    type_name == "R1-R6" || type_name.starts_with("R7") || type_name.starts_with("R8")
}

fn is_pn(annotation: &Annotation) -> bool {
    matches!(annotation.class.as_deref(), Some("ALPN" | "SEZPN"))
        || annotation
            .type_name
            .as_deref()
            .is_some_and(|type_name| type_name.ends_with("_lPN"))
}

fn insert_population(populations: &mut PopulationRoots, name: &str, body_id: u64) {
    populations
        .entry(name.to_owned())
        .or_default()
        .insert(body_id);
}

fn source_file(path: &Path, name: &str, url: &str) -> Result<SourceFile, Box<dyn Error>> {
    let metadata = fs::metadata(path)?;
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = std::io::Read::read(&mut file, &mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(SourceFile {
        name: name.to_owned(),
        url: url.to_owned(),
        commit: SOURCE_COMMIT.to_owned(),
        size_bytes: metadata.len(),
        sha256,
        license_source: LICENSE_URL.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_resolution_prefers_consensus_and_applies_histamine_inhibition() {
        let mut predictions = HashMap::new();
        predictions.insert(
            1,
            NtPrediction {
                consensus_nt: Some("GABA".to_owned()),
                celltype_predicted_nt: Some("acetylcholine".to_owned()),
            },
        );
        predictions.insert(
            2,
            NtPrediction {
                consensus_nt: Some("unclear".to_owned()),
                celltype_predicted_nt: Some("acetylcholine".to_owned()),
            },
        );
        predictions.insert(
            3,
            NtPrediction {
                consensus_nt: Some("histamine".to_owned()),
                celltype_predicted_nt: None,
            },
        );
        let mut counters = SignCounters::default();
        assert_eq!(
            resolve_sign(
                1,
                &predictions,
                SignPolicy::Zero,
                SignPolicy::Zero,
                &mut counters
            )
            .expect("consensus sign should resolve"),
            -1
        );
        assert_eq!(
            resolve_sign(
                2,
                &predictions,
                SignPolicy::Zero,
                SignPolicy::Zero,
                &mut counters
            )
            .expect("celltype fallback sign should resolve"),
            1
        );
        assert_eq!(
            resolve_sign(
                3,
                &predictions,
                SignPolicy::Positive,
                SignPolicy::Zero,
                &mut counters
            )
            .expect("histamine sign should resolve"),
            -1
        );
        assert_eq!(counters.consensus_nt_edge_count, 2);
        assert_eq!(counters.celltype_predicted_nt_edge_count, 1);
        assert_eq!(counters.histamine_edge_count, 1);
    }

    #[test]
    fn unknown_and_other_sign_policies_cover_all_configured_branches() {
        for (policy, expected) in [
            (SignPolicy::Zero, 0),
            (SignPolicy::Positive, 1),
            (SignPolicy::Negative, -1),
        ] {
            let mut counters = SignCounters::default();
            let mut predictions = HashMap::new();
            predictions.insert(
                1,
                NtPrediction {
                    consensus_nt: Some("dopamine".to_owned()),
                    celltype_predicted_nt: None,
                },
            );
            assert_eq!(
                resolve_sign(1, &predictions, policy, SignPolicy::Zero, &mut counters)
                    .expect("other policy should resolve"),
                expected
            );
            assert_eq!(
                resolve_sign(2, &predictions, policy, policy, &mut counters)
                    .expect("unknown policy should resolve"),
                expected
            );
        }
        let mut counters = SignCounters::default();
        let predictions = HashMap::from([(
            1,
            NtPrediction {
                consensus_nt: Some("dopamine".to_owned()),
                celltype_predicted_nt: None,
            },
        )]);
        assert!(
            resolve_sign(
                1,
                &predictions,
                SignPolicy::Error,
                SignPolicy::Zero,
                &mut counters
            )
            .is_err()
        );
        assert!(
            resolve_sign(
                2,
                &predictions,
                SignPolicy::Zero,
                SignPolicy::Error,
                &mut counters
            )
            .is_err()
        );
    }
}
