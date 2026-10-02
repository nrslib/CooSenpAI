#[path = "rate_activity_display.rs"]
mod rate_activity_display;

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use habitua_connectome::{
    FrozenReadoutHistoryEntry, RateGraph, RateHelperArtifact, RateHelperBrain, RateParameters,
    RgbImage, final_activity_with_parameters_profile,
    final_activity_with_parameters_sampled_profile, frozen_readout_augmented_features_with_expiry,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let result = match arguments.next().as_deref() {
        Some("rate") => run_rate_helper(arguments),
        Some(option) if option.starts_with("--") => {
            run_rate_helper(std::iter::once(option.to_owned()).chain(arguments))
        }
        Some(_) => {
            Err("usage: connectome-helper [rate --artifact <path> --pack <absolute-path>]".into())
        }
        None => run_rate_helper(std::iter::empty()),
    };
    if let Err(error) = &result
        && let Some(startup) = error.downcast_ref::<RateStartupError>()
    {
        write_rate_output(
            &mut io::stdout().lock(),
            &json!({
                "v": 1,
                "id": "",
                "session": null,
                "schema": "coosenpai-observation-v1",
                "ok": false,
                "error": {"code": startup.code, "message": startup.message},
            }),
        )?;
    }
    result
}

#[derive(Debug)]
struct RateStartupError {
    code: &'static str,
    message: &'static str,
}

impl std::fmt::Display for RateStartupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for RateStartupError {}

fn startup_error(code: &'static str, message: &'static str) -> Box<dyn Error> {
    Box::new(RateStartupError { code, message })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RateObservationPayload {
    frame_id: String,
    at_ms: u64,
    app: Option<String>,
    kind: String,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    image_path: Option<String>,
    #[serde(default)]
    ocr_text: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RateRequest {
    v: u16,
    id: String,
    #[serde(default)]
    session: Option<String>,
    op: String,
    params: Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RateObserveParams {
    input_id: String,
    image_path: String,
    stream_id: String,
    at_ms: u64,
    timestamp_s: f64,
    feature_schema: String,
    features: Vec<f32>,
    observation: Value,
    #[serde(default)]
    context: Option<Value>,
    #[serde(default)]
    image_sha256: Option<String>,
    #[serde(default)]
    position: Option<u64>,
    #[serde(default)]
    shadow: bool,
    #[serde(default)]
    repeat_index: u32,
    #[serde(default)]
    throttle_every: Option<u32>,
}

fn validate_observation_payload(
    observation: &RateObservationPayload,
    params: &RateObserveParams,
) -> Result<(), String> {
    ensure_bounded_string(
        &observation.frame_id,
        "observation frame_id",
        RATE_HELPER_MAX_IDENTIFIER_BYTES,
    )?;
    for (name, value) in [
        ("observation kind", observation.kind.as_str()),
        (
            "observation app",
            observation.app.as_deref().unwrap_or_default(),
        ),
        (
            "observation target",
            observation.target.as_deref().unwrap_or_default(),
        ),
        (
            "observation source",
            observation.source.as_deref().unwrap_or_default(),
        ),
    ] {
        ensure_bounded_string(value, name, RATE_HELPER_MAX_IDENTIFIER_BYTES)?;
    }
    for (name, value) in [
        (
            "observation image_path",
            observation.image_path.as_deref().unwrap_or_default(),
        ),
        (
            "observation text",
            observation.text.as_deref().unwrap_or_default(),
        ),
        (
            "observation ocr_text",
            observation.ocr_text.as_deref().unwrap_or_default(),
        ),
    ] {
        ensure_bounded_string(value, name, RATE_HELPER_MAX_CONTEXT_BYTES)?;
    }
    if observation.frame_id.is_empty()
        || observation.frame_id != params.input_id
        || observation.at_ms != params.at_ms
    {
        return Err("observation frame_id and at_ms must match the request".to_owned());
    }
    if !params.timestamp_s.is_finite()
        || (params.timestamp_s * 1_000.0 - params.at_ms as f64).abs() > 1.0e-6
    {
        return Err("timestamp_s must match at_ms to the nearest millisecond".to_owned());
    }
    match observation.kind.as_str() {
        "visual" => {
            if observation.target.as_deref().is_none_or(str::is_empty) {
                return Err("visual observation requires target".to_owned());
            }
            if let Some(observation_image_path) = &observation.image_path
                && !observation_image_path.is_empty()
                && observation_image_path != &params.image_path
            {
                return Err("observation image_path does not match the request".to_owned());
            }
        }
        "audio" => {
            if observation.app.is_some()
                || observation.source.as_deref().is_none_or(str::is_empty)
                || observation.text.is_none()
            {
                return Err("audio observation requires app=null, source, and text".to_owned());
            }
            if !params.image_path.is_empty() {
                return Err("audio observation must not provide image_path".to_owned());
            }
        }
        other => return Err(format!("unsupported observation kind: {other}")),
    }
    if let Some(context) = &params.context {
        let context = context
            .as_object()
            .ok_or_else(|| "context must be a JSON object".to_owned())?;
        for field in ["frames", "audio"] {
            let Some(entries) = context.get(field) else {
                continue;
            };
            let entries = entries
                .as_array()
                .ok_or_else(|| format!("context.{field} must be an array"))?;
            if entries.iter().any(|entry| !entry.is_object()) {
                return Err(format!("context.{field} entries must be JSON objects"));
            }
        }
    }
    Ok(())
}

fn canonicalize_json_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json_value).collect()),
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort();
            let mut canonical = serde_json::Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonicalize_json_value(&object[key]));
            }
            Value::Object(canonical)
        }
        scalar => scalar.clone(),
    }
}

fn observation_identity_digest(
    params: &RateObserveParams,
    observation: &RateObservationPayload,
    resolved_image_sha256: &str,
) -> Result<String, String> {
    let identity = json!({
        "schema": "rate-observation-identity-v1",
        "input_id": params.input_id,
        "image_path": params.image_path,
        "provided_image_sha256": params.image_sha256,
        "resolved_image_sha256": resolved_image_sha256,
        "stream_id": params.stream_id,
        "at_ms": params.at_ms,
        "timestamp_s": params.timestamp_s,
        "feature_schema": params.feature_schema,
        "features": params.features,
        "observation": params.observation,
        "context": params.context,
        "position": params.position,
        "shadow": params.shadow,
        "repeat_index": params.repeat_index,
        "throttle_every": params.throttle_every,
        "observation_kind": observation.kind,
    });
    let canonical = canonicalize_json_value(&identity);
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|error| format!("could not serialize observation identity: {error}"))?;
    Ok(sha256_hex(&bytes))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RateFeedParams {
    event_id: String,
    input_id: String,
    sign: String,
    strength: f64,
    event_time_s: f64,
    received_at_s: f64,
    revision: u64,
    cancelled: bool,
    source: String,
}

fn ensure_bounded_string(value: &str, name: &str, limit: usize) -> Result<(), String> {
    if value.len() > limit {
        return Err(format!("{name} exceeds the {limit}-byte limit"));
    }
    Ok(())
}

fn ensure_json_size(value: &Value, name: &str, limit: usize) -> Result<(), String> {
    let size = serde_json::to_vec(value)
        .map_err(|error| format!("{name} cannot be serialized: {error}"))?
        .len();
    if size > limit {
        return Err(format!("{name} exceeds the {limit}-byte limit"));
    }
    Ok(())
}

fn validate_rate_request_metadata(request: &RateRequest) -> Result<(), String> {
    ensure_bounded_string(&request.id, "request id", RATE_HELPER_MAX_IDENTIFIER_BYTES)?;
    if let Some(session) = &request.session {
        ensure_bounded_string(session, "request session", RATE_HELPER_MAX_IDENTIFIER_BYTES)?;
    }
    Ok(())
}

fn validate_rate_observe_params(params: &RateObserveParams) -> Result<(), String> {
    ensure_bounded_string(
        &params.input_id,
        "observation input_id",
        RATE_HELPER_MAX_IDENTIFIER_BYTES,
    )?;
    ensure_bounded_string(
        &params.stream_id,
        "observation stream_id",
        RATE_HELPER_MAX_IDENTIFIER_BYTES,
    )?;
    ensure_bounded_string(
        &params.image_path,
        "observation image_path",
        RATE_HELPER_MAX_PATH_BYTES,
    )?;
    ensure_bounded_string(
        &params.feature_schema,
        "observation feature_schema",
        RATE_HELPER_MAX_IDENTIFIER_BYTES,
    )?;
    if let Some(image_sha256) = &params.image_sha256 {
        ensure_bounded_string(
            image_sha256,
            "observation image_sha256",
            RATE_HELPER_MAX_IDENTIFIER_BYTES,
        )?;
    }
    if params.features.len() > RATE_HELPER_MAX_FEATURE_VALUES {
        return Err(format!(
            "observation features exceeds the {RATE_HELPER_MAX_FEATURE_VALUES}-value limit"
        ));
    }
    ensure_json_size(
        &params.observation,
        "observation payload",
        RATE_HELPER_MAX_OBSERVATION_BYTES,
    )?;
    if let Some(context) = &params.context {
        ensure_json_size(
            context,
            "observation context",
            RATE_HELPER_MAX_CONTEXT_BYTES,
        )?;
    }
    Ok(())
}

fn validate_rate_feed_params(params: &RateFeedParams) -> Result<(), String> {
    ensure_bounded_string(
        &params.event_id,
        "feedback event_id",
        RATE_HELPER_MAX_IDENTIFIER_BYTES,
    )?;
    ensure_bounded_string(
        &params.input_id,
        "feedback input_id",
        RATE_HELPER_MAX_IDENTIFIER_BYTES,
    )?;
    ensure_bounded_string(&params.sign, "feedback sign", RATE_HELPER_MAX_REASON_BYTES)?;
    ensure_bounded_string(
        &params.source,
        "feedback source",
        RATE_HELPER_MAX_SOURCE_BYTES,
    )?;
    Ok(())
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct RateHelperObservation {
    input_id: String,
    image_sha256: String,
    features: Vec<f32>,
    timestamp_s: f64,
    stream_id: String,
    position: u64,
    #[serde(default)]
    decision: Option<RateHelperObservationDecision>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum RateHelperObservationBodyStatus {
    Present,
    Expired,
    NotFeedable,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct RateHelperObservationLedgerEntry {
    input_id: String,
    digest: String,
    decision: RateHelperObservationDecision,
    #[serde(default)]
    body_status: Option<RateHelperObservationBodyStatus>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct RateHelperObservationDecision {
    readiness: String,
    diagnostic_readiness: String,
    novelty: Option<f64>,
    relevance: Option<f64>,
    action: String,
    diagnostic_action: String,
    external_action: String,
    hold_reason: Option<String>,
    brains: Vec<Value>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct RateHelperFeedback {
    event_id: String,
    input_id: String,
    image_sha256: String,
    sign: String,
    strength: f64,
    event_time_s: f64,
    received_at_s: f64,
    applied_at_s: f64,
    revision: u64,
    cancelled: bool,
    source: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct RateHelperFeedbackAudit {
    event_id: String,
    input_id: String,
    #[serde(default)]
    sign: String,
    #[serde(default)]
    strength: f64,
    #[serde(default)]
    event_time_s: f64,
    revision: u64,
    cancelled: bool,
    source: String,
    received_at_s: f64,
    applied_at_s: f64,
    applied: bool,
    reason: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct RateHelperCase {
    image_sha256: String,
    features: Vec<f32>,
    event_ids: Vec<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct RateHelperPersistedState {
    state_version: u32,
    artifact_sha256: String,
    observations: BTreeMap<String, RateHelperObservation>,
    histories: BTreeMap<String, Vec<FrozenReadoutHistoryEntry>>,
    feedback: BTreeMap<String, RateHelperFeedback>,
    feedback_history: Vec<RateHelperFeedbackAudit>,
    cases: BTreeMap<String, RateHelperCase>,
    #[serde(default)]
    observation_ledger: BTreeMap<String, RateHelperObservationLedgerEntry>,
}

impl Default for RateHelperPersistedState {
    fn default() -> Self {
        Self {
            state_version: RATE_HELPER_STATE_VERSION,
            artifact_sha256: String::new(),
            observations: BTreeMap::new(),
            histories: BTreeMap::new(),
            feedback: BTreeMap::new(),
            feedback_history: Vec::new(),
            cases: BTreeMap::new(),
            observation_ledger: BTreeMap::new(),
        }
    }
}

const RATE_HELPER_STATE_VERSION: u32 = 2;
const RATE_HELPER_TIMEOUT_MESSAGE: &str = "rate helper request exceeded timeout";
const RATE_HELPER_MAX_AUDIT_ENTRIES: usize = 4_096;
const RATE_HELPER_MAX_OBSERVATIONS: usize = 4_096;
const RATE_HELPER_MAX_HISTORY_ENTRIES_PER_STREAM: usize = 16;
const RATE_HELPER_MAX_HISTORY_STREAMS: usize = 1_024;
const RATE_HELPER_MAX_FEEDBACK_ENTRIES: usize = 4_096;
const RATE_HELPER_MAX_OBSERVATION_LEDGER_ENTRIES: usize = 4_096;
const RATE_HELPER_MAX_IDENTIFIER_BYTES: usize = 256;
const RATE_HELPER_MAX_PATH_BYTES: usize = 4_096;
const RATE_HELPER_MAX_SOURCE_BYTES: usize = 64;
const RATE_HELPER_MAX_REASON_BYTES: usize = 256;
const RATE_HELPER_MAX_FEATURE_VALUES: usize = 4_096;
const RATE_HELPER_MAX_CONTEXT_BYTES: usize = 64 * 1_024;
const RATE_HELPER_MAX_OBSERVATION_BYTES: usize = 64 * 1_024;
const RATE_HELPER_MAX_STATE_BYTES: usize = 16 * 1_024 * 1_024;

type ParentDirectorySync = fn(&Path) -> io::Result<()>;

struct RateHelperRuntime {
    artifact: RateHelperArtifact,
    artifact_sha256: String,
    graph: RateGraph,
    parameters: RateParameters,
    rest_activity: Vec<f64>,
    activity_display: rate_activity_display::ActivityDisplay,
    response_cache: BTreeMap<String, RateHelperCachedResponse>,
    state_path: PathBuf,
    state: RateHelperPersistedState,
    persistence_error: Option<String>,
    parent_directory_sync: ParentDirectorySync,
}

#[derive(Clone, Debug)]
struct RateHelperCachedResponse {
    features: Vec<f32>,
    csr_traversal_ms: f64,
    neuron_update_ms: f64,
    other_ms: f64,
}

fn run_rate_helper(mut arguments: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    let mut artifact_path = None;
    let mut pack_path = None;
    let mut state_path = None;
    let mut max_in_flight = 4_usize;
    let mut timeout_ms = 5_000_u64;
    while let Some(argument) = arguments.next() {
        let (name, value) = if let Some((name, value)) = argument.split_once('=') {
            (name.to_owned(), value.to_owned())
        } else {
            let value = arguments
                .next()
                .ok_or_else(|| format!("{argument} requires a value"))?;
            (argument, value)
        };
        match name.as_str() {
            "--artifact" => artifact_path = Some(PathBuf::from(value)),
            "--pack" => pack_path = Some(PathBuf::from(value)),
            "--state" => state_path = Some(PathBuf::from(value)),
            "--max-in-flight" => max_in_flight = value.parse()?,
            "--timeout-ms" => timeout_ms = value.parse()?,
            _ => return Err("unknown rate-helper option".into()),
        }
    }
    if max_in_flight == 0 || timeout_ms == 0 {
        return Err("--max-in-flight and --timeout-ms must be positive".into());
    }
    let artifact_path = artifact_path
        .or_else(|| env::var_os("HABITUA_RATE_ARTIFACT").map(PathBuf::from))
        .ok_or("rate helper requires --artifact or HABITUA_RATE_ARTIFACT")?;
    let state_path = state_path
        .or_else(|| env::var_os("HABITUA_RATE_STATE").map(PathBuf::from))
        .ok_or_else(|| {
            startup_error(
                "state-path-required",
                "HABITUA_RATE_STATE or --state is required",
            )
        })?;
    let pack_path = pack_path.or_else(|| env::var_os("HABITUA_RATE_PACK").map(PathBuf::from));
    if pack_path.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err(startup_error(
            "pack-path-invalid",
            "--pack and HABITUA_RATE_PACK require an absolute path",
        ));
    }
    let mut runtime = RateHelperRuntime::load(&artifact_path, state_path, pack_path.as_deref())?;
    // The protocol accepts max-in-flight for compatibility, but stateful rate
    // requests are intentionally processed by one resident loop. There is no
    // queue or per-request worker because all state changes are ordered.
    let _ = max_in_flight;
    let stdin = io::stdin();
    let stdout = io::stdout();
    run_rate_helper_loop(
        &mut runtime,
        stdin.lock(),
        io::BufWriter::new(stdout.lock()),
        timeout_ms,
    )
}

fn run_rate_helper_loop<R: BufRead, W: Write>(
    runtime: &mut RateHelperRuntime,
    input: R,
    mut output: W,
    timeout_ms: u64,
) -> Result<(), Box<dyn Error>> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<RateRequest>(&line) {
            Ok(request) => request,
            Err(error) => {
                let response = rate_parse_error_response(&line, &error.to_string());
                write_rate_output(&mut output, &response)?;
                continue;
            }
        };
        let should_shutdown = request.op == "shutdown";
        let response = process_rate_request(runtime, request, timeout_ms);
        let persistence_uncertain = response
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            == Some("persistence_uncertain");
        write_rate_output(&mut output, &response)?;
        if persistence_uncertain {
            return Err(
                "rate helper stopped after a committed state became durability-uncertain".into(),
            );
        }
        if should_shutdown {
            break;
        }
    }
    Ok(())
}

fn process_rate_request(
    runtime: &mut RateHelperRuntime,
    request: RateRequest,
    timeout_ms: u64,
) -> Value {
    let started = Instant::now();
    let deadline = started + Duration::from_millis(timeout_ms);
    if request.v != 1 {
        rate_error_response(
            &request,
            "unsupported_version",
            "unsupported protocol version",
        )
    } else if request.id.is_empty() || request.session.as_deref().is_none_or(str::is_empty) {
        rate_error_response(
            &request,
            "invalid_request",
            "rate requests require non-empty id and session",
        )
    } else if let Err(error) = validate_rate_request_metadata(&request) {
        rate_error_response(&request, "invalid_request", &error)
    } else if deadline_exceeded(Some(deadline)) {
        rate_error_response(&request, "timeout", RATE_HELPER_TIMEOUT_MESSAGE)
    } else {
        match runtime.handle_with_deadline(&request.op, request.params.clone(), Some(deadline)) {
            Ok(result) => json!({
                "v": 1,
                "id": request.id,
                "session": request.session,
                "schema": "coosenpai-observation-v1",
                "ok": true,
                "result": result,
                "elapsed_ms": started.elapsed().as_secs_f64() * 1_000.0,
            }),
            Err(error) if error == RATE_HELPER_TIMEOUT_MESSAGE => {
                rate_error_response(&request, "timeout", RATE_HELPER_TIMEOUT_MESSAGE)
            }
            Err(error) if error.starts_with("state committed; durability unknown:") => {
                rate_error_response(&request, "persistence_uncertain", &error)
            }
            Err(error) => rate_error_response(&request, rate_error_code(&error), &error),
        }
    }
}

fn rate_error_code(error: &str) -> &str {
    if error.starts_with("observation ledger capacity exceeded") {
        "observation_ledger_capacity_exceeded"
    } else if error.starts_with("feedback capacity exceeded") {
        "feedback_capacity_exceeded"
    } else if error.starts_with("history stream capacity exceeded") {
        "history_stream_capacity_exceeded"
    } else if error.starts_with("observation body capacity reached") {
        "observation_body_capacity_exceeded"
    } else if error.contains("persisted state exceeds") {
        "state_size_exceeded"
    } else if error.starts_with("observation body has expired") {
        "observation_expired"
    } else if error.starts_with("observation is not eligible for feed") {
        "observation_not_feedable"
    } else if error.contains("canonical observation digest") {
        "observation_replay_mismatch"
    } else if error.contains("no canonical replay ledger entry") {
        "observation_replay_unverifiable"
    } else {
        "request_error"
    }
}

fn write_rate_output(output: &mut impl Write, value: &Value) -> Result<(), Box<dyn Error>> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn rate_error_response(request: &RateRequest, code: &str, message: &str) -> Value {
    json!({
        "v": 1,
        "id": request.id,
        "session": request.session,
        "schema": "coosenpai-observation-v1",
        "ok": false,
        "error": {"code": code, "message": message},
    })
}

fn rate_parse_error_response(line: &str, message: &str) -> Value {
    let envelope = serde_json::from_str::<Value>(line).ok();
    let id = envelope
        .as_ref()
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let session = envelope
        .as_ref()
        .and_then(|value| value.get("session"))
        .and_then(Value::as_str);
    json!({
        "v": 1,
        "id": id,
        "session": session,
        "schema": "coosenpai-observation-v1",
        "ok": false,
        "error": {"code": "invalid_request", "message": message},
    })
}

fn deadline_exceeded(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| Instant::now() >= deadline)
}

impl RateHelperRuntime {
    fn load(
        artifact_path: &Path,
        state_path: PathBuf,
        pack_override: Option<&Path>,
    ) -> Result<Self, Box<dyn Error>> {
        let artifact_bytes = fs::read(artifact_path)?;
        let artifact: RateHelperArtifact = serde_json::from_slice(&artifact_bytes)?;
        let artifact_sha256 = sha256_hex(&artifact_bytes);
        artifact.validate_shape(None)?;
        let pack_path = resolve_pack_path(artifact_path, &artifact.pack_path, pack_override)?;
        match fs::metadata(&pack_path) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(startup_error(
                    "pack-invalid",
                    "rate pack path is not a directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(startup_error(
                    "pack-missing",
                    "rate pack directory is missing",
                ));
            }
            Err(_) => {
                return Err(startup_error(
                    "pack-unreadable",
                    "rate pack directory cannot be read",
                ));
            }
        }
        let pack_manifest_sha256 = sha256_hex(
            &fs::read(pack_path.join("rate_manifest.json")).map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    startup_error("pack-missing", "rate pack manifest is missing")
                } else {
                    startup_error("pack-unreadable", "rate pack manifest cannot be read")
                }
            })?,
        );
        if pack_manifest_sha256 != artifact.pack_manifest_sha256 {
            return Err(startup_error(
                "pack-manifest-mismatch",
                "rate pack manifest SHA-256 does not match artifact",
            ));
        }
        let graph = RateGraph::load(&pack_path).map_err(|_| {
            startup_error(
                "pack-invalid",
                "rate pack graph cannot be loaded or verified",
            )
        })?;
        if artifact.graph_fingerprint != graph.fingerprint() {
            return Err(startup_error(
                "graph-fingerprint-mismatch",
                "rate graph fingerprint does not match artifact",
            ));
        }
        artifact.validate_shape(Some(&graph))?;
        let mut parameters = RateParameters::initial(&graph);
        parameters.hmax = artifact.hmax;
        let zero_input = vec![0.0; graph.neuron_count()];
        let (rest_activity, _) = final_activity_with_parameters_profile(
            &graph,
            &parameters,
            &zero_input,
            artifact.rate_steps_per_observation,
            None,
        )?;
        let state_exists = state_path.exists();
        let mut state = if state_exists {
            load_persisted_state(&state_path)?
        } else {
            RateHelperPersistedState::default()
        };
        if state_exists {
            validate_state_artifact_binding(&state, &artifact_sha256)?;
            validate_state_history_capacity(&state, RATE_HELPER_MAX_HISTORY_ENTRIES_PER_STREAM)?;
        }
        state.artifact_sha256 = artifact_sha256.clone();
        let mut runtime = Self {
            artifact,
            artifact_sha256,
            activity_display: rate_activity_display::ActivityDisplay::for_graph(&graph),
            graph,
            parameters,
            rest_activity,
            response_cache: BTreeMap::new(),
            state_path,
            state,
            persistence_error: None,
            parent_directory_sync: sync_parent_directory,
        };
        runtime.rebuild_cases()?;
        Ok(runtime)
    }

    fn handle_with_deadline(
        &mut self,
        operation: &str,
        params: Value,
        deadline: Option<Instant>,
    ) -> Result<Value, String> {
        if operation != "shutdown"
            && let Some(error) = &self.persistence_error
        {
            return Err(format!(
                "rate helper state persistence is unavailable until restart: {error}"
            ));
        }
        match operation {
            "health" => Ok(json!({
                "status": "ready",
                "model": "rate-full-cns-readout",
                "observation_schema": "coosenpai-observation-v1",
                "artifact_version": self.artifact.artifact_version,
                "artifact_sha256": self.artifact_sha256,
                "graph_neuron_count": self.graph.neuron_count(),
                "graph_edge_count": self.graph.edge_count(),
                "case_match_policy": self.artifact.case_memory.match_policy,
                "case_count": self.state.cases.len(),
                "observation_count": self.state.observations.len(),
                "observation_capacity": RATE_HELPER_MAX_OBSERVATIONS,
                "feedback_history_count": self.state.feedback_history.len(),
                "feedback_history_capacity": RATE_HELPER_MAX_AUDIT_ENTRIES,
                "feedback_capacity": RATE_HELPER_MAX_FEEDBACK_ENTRIES,
                "observation_ledger_count": self.state.observation_ledger.len(),
                "observation_ledger_capacity": RATE_HELPER_MAX_OBSERVATION_LEDGER_ENTRIES,
                "history_stream_count": self.state.histories.len(),
                "history_stream_capacity": RATE_HELPER_MAX_HISTORY_STREAMS,
                "history_capacity_per_stream": RATE_HELPER_MAX_HISTORY_ENTRIES_PER_STREAM,
                "state_file_bytes": self.state_file_size_bytes(),
                "resident": true,
                "request_processing": "single_resident_loop",
                "stateful_requests_ordered": true,
                "timeout_scope": "compute_before_persistence_commit",
                "persistence_commit_semantics": "rename_is_commit_point; post-rename-sync-failure_stops_with_durability-unknown",
                "shadow_consensus_module": "two_role_consensus",
            })),
            "observe" => self.observe_with_deadline(params, deadline),
            "evaluate" => self.observe_with_deadline(params, deadline),
            "feed" => self.feed_with_deadline(params, deadline),
            "shutdown" => Ok(json!({"shutdown": true})),
            _ => Err(format!("unsupported rate-helper operation: {operation}")),
        }
    }

    fn observe_with_deadline(
        &mut self,
        params: Value,
        deadline: Option<Instant>,
    ) -> Result<Value, String> {
        self.activity_display.clear();
        let params: RateObserveParams =
            serde_json::from_value(params).map_err(|error| error.to_string())?;
        validate_rate_observe_params(&params)?;
        if params.input_id.is_empty() || params.stream_id.is_empty() {
            return Err("evaluate requires non-empty input_id and stream_id".to_owned());
        }
        if params.feature_schema != habitua_connectome::RATE_HELPER_OBSERVATION_SCHEMA {
            return Err(format!(
                "feature_schema must be {}",
                habitua_connectome::RATE_HELPER_OBSERVATION_SCHEMA
            ));
        }
        if !params.timestamp_s.is_finite() || params.timestamp_s < 0.0 {
            return Err("evaluate timestamp_s must be a finite non-negative value".to_owned());
        }
        if params.features.iter().any(|value| !value.is_finite()) {
            return Err("evaluate features must be finite".to_owned());
        }
        let observation: RateObservationPayload =
            serde_json::from_value(params.observation.clone())
                .map_err(|error| format!("observation payload is invalid: {error}"))?;
        validate_observation_payload(&observation, &params)?;
        let observation_has_ocr_text = observation.ocr_text.is_some();
        let context_present = params.context.is_some();
        if let Some(every) = params.throttle_every {
            if every == 0 {
                return Err("observe throttle_every must be positive".to_owned());
            }
            if !params.repeat_index.is_multiple_of(every) {
                let decision = RateHelperObservationDecision {
                    readiness: "insufficient-history".to_owned(),
                    diagnostic_readiness: "throttled".to_owned(),
                    novelty: None,
                    relevance: None,
                    action: "hold".to_owned(),
                    diagnostic_action: "hold".to_owned(),
                    external_action: "hold".to_owned(),
                    hold_reason: Some("throttled".to_owned()),
                    brains: Vec::new(),
                };
                let digest = observation_identity_digest(&params, &observation, "")?;
                return self.persist_early_observation(
                    &params,
                    &observation,
                    &decision,
                    &digest,
                    Some(every),
                    observation_has_ocr_text,
                    context_present,
                    deadline,
                );
            }
        }
        if observation.kind == "audio" {
            let decision = RateHelperObservationDecision {
                readiness: "unsupported-input".to_owned(),
                diagnostic_readiness: "audio_not_supported_by_retina_rate_model".to_owned(),
                novelty: None,
                relevance: None,
                action: "hold".to_owned(),
                diagnostic_action: "hold".to_owned(),
                external_action: "hold".to_owned(),
                hold_reason: Some("unsupported-input".to_owned()),
                brains: Vec::new(),
            };
            let digest = observation_identity_digest(&params, &observation, "")?;
            return self.persist_early_observation(
                &params,
                &observation,
                &decision,
                &digest,
                None,
                observation_has_ocr_text,
                context_present,
                deadline,
            );
        }
        if observation.kind == "visual" && observation.app.is_none() {
            let decision = RateHelperObservationDecision {
                readiness: "input-insufficient".to_owned(),
                diagnostic_readiness: "visual_app_missing".to_owned(),
                novelty: None,
                relevance: None,
                action: "hold".to_owned(),
                diagnostic_action: "hold".to_owned(),
                external_action: "hold".to_owned(),
                hold_reason: Some("input-insufficient".to_owned()),
                brains: Vec::new(),
            };
            let digest = observation_identity_digest(&params, &observation, "")?;
            return self.persist_early_observation(
                &params,
                &observation,
                &decision,
                &digest,
                None,
                observation_has_ocr_text,
                context_present,
                deadline,
            );
        }
        if params.image_path.is_empty() {
            return Err("visual evaluate requires image_path".to_owned());
        }
        let image = read_rate_image(std::path::Path::new(&params.image_path))
            .map_err(|error| error.to_string())?;
        let image_sha256 = habitua_connectome::image_sha256(&image);
        if params
            .image_sha256
            .as_deref()
            .is_some_and(|expected| expected != image_sha256)
        {
            return Err("observe image_sha256 does not match image bytes".to_owned());
        }
        let digest = observation_identity_digest(&params, &observation, &image_sha256)?;
        if let Some(entry) = self.state.observation_ledger.get(&params.input_id) {
            if entry.digest != digest {
                return Err(
                    "input_id replay does not match the canonical observation digest".to_owned(),
                );
            }
            let position = self.state.observations.get(&params.input_id).map_or_else(
                || params.position.unwrap_or(0),
                |observation| observation.position,
            );
            let body_status = entry.body_status.clone().unwrap_or_else(|| {
                if self.state.observations.contains_key(&params.input_id) {
                    RateHelperObservationBodyStatus::Present
                } else {
                    RateHelperObservationBodyStatus::Expired
                }
            });
            let (feedable, feed_rejection_code) = match body_status {
                RateHelperObservationBodyStatus::Present => (true, None),
                RateHelperObservationBodyStatus::Expired => (false, Some("observation_expired")),
                RateHelperObservationBodyStatus::NotFeedable => {
                    (false, Some("observation_not_feedable"))
                }
            };
            return Ok(self.observation_response(
                &params,
                &observation,
                &image_sha256,
                &entry.decision,
                position,
                true,
                feedable,
                feed_rejection_code,
                None,
                observation_has_ocr_text,
                context_present,
                params.shadow,
                params.repeat_index,
                &entry.decision.brains,
                &(0.0, 0.0, 0.0),
                true,
                self.state
                    .cases
                    .get(&image_sha256)
                    .map_or(0, |case| case.event_ids.len()),
                self.case_adjustment(&image_sha256, params.timestamp_s).0,
            ));
        }
        if self.state.observations.contains_key(&params.input_id) {
            return Err(
                "existing observation has no canonical replay ledger entry; refusing to reprocess it"
                    .to_owned(),
            );
        }
        self.ensure_observation_ledger_capacity(&params.input_id)?;
        self.ensure_history_stream_capacity(&params.stream_id)?;
        let previous_observation = self.state.observations.get(&params.input_id).cloned();
        let (response_features, timing, response_cache_hit) =
            if let Some(cached) = self.response_cache.get(&image_sha256).cloned() {
                (
                    cached.features,
                    (
                        cached.csr_traversal_ms,
                        cached.neuron_update_ms,
                        cached.other_ms,
                    ),
                    true,
                )
            } else {
                let resized = image
                    .resize_bilinear(
                        self.artifact.retina_map.config.width,
                        self.artifact.retina_map.config.height,
                    )
                    .map_err(|error| error.to_string())?;
                let retina_input = self
                    .artifact
                    .retina_map
                    .encode(&resized, self.graph.neuron_count())
                    .map_err(|error| error.to_string())?;
                let (activity, timing) = final_activity_with_parameters_sampled_profile(
                    &self.graph,
                    &self.parameters,
                    &retina_input.input,
                    self.artifact.rate_steps_per_observation,
                    &mut self.activity_display.samples,
                )
                .map_err(|error| error.to_string())?;
                self.activity_display.capture(
                    &image_sha256,
                    self.artifact.rate_steps_per_observation,
                    self.parameters.hmax,
                );
                let response_features =
                    helper_response_features(&activity, &self.rest_activity, &self.artifact)?;
                if deadline_exceeded(deadline) {
                    return Err(RATE_HELPER_TIMEOUT_MESSAGE.to_owned());
                }
                self.response_cache.insert(
                    image_sha256.clone(),
                    RateHelperCachedResponse {
                        features: response_features.clone(),
                        csr_traversal_ms: timing.csr_traversal_ms,
                        neuron_update_ms: timing.neuron_update_ms,
                        other_ms: timing.other_ms,
                    },
                );
                while self.response_cache.len() > self.artifact.case_memory.max_cases {
                    if let Some(oldest) = self.response_cache.keys().next().cloned() {
                        self.response_cache.remove(&oldest);
                    }
                }
                (
                    response_features,
                    (
                        timing.csr_traversal_ms,
                        timing.neuron_update_ms,
                        timing.other_ms,
                    ),
                    false,
                )
            };
        let history = self
            .state
            .histories
            .get(&params.stream_id)
            .cloned()
            .unwrap_or_default();
        let (history_features, augmented_features) = frozen_readout_augmented_features_with_expiry(
            &response_features,
            &history,
            params.timestamp_s,
            self.artifact.history_recent_observations,
            self.artifact.history_distance_threshold as f32,
            self.artifact.history_previous_absolute_difference_scale as f32,
            self.artifact.history_elapsed_time_scale_seconds as f32,
            self.artifact.history_max_age_seconds as f32,
        );
        let (readiness, diagnostic_readiness) = if history_features.distance.is_some() {
            ("ready", "evaluated")
        } else {
            ("insufficient-history", "baseline_insufficient")
        };
        let (case_adjustment, case_match_count) =
            self.case_adjustment(&image_sha256, params.timestamp_s);
        let brains = self
            .artifact
            .brains
            .iter()
            .map(|brain| {
                let base_probability = helper_brain_probability(brain, &augmented_features)?;
                let adjustment = if brain.role == "change" {
                    case_adjustment
                } else if brain.role == "no_change" {
                    -case_adjustment
                } else {
                    0.0
                };
                let adjusted_probability = sigmoid(logit(base_probability) + adjustment);
                Ok::<_, String>(json!({
                    "id": brain.id,
                    "role": brain.role,
                    "base_probability": base_probability,
                    "probability": adjusted_probability,
                    "adjustment": adjustment,
                    "threshold": brain.reaction_threshold,
                }))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let change = self
            .artifact
            .brains
            .iter()
            .find(|brain| brain.role == "change")
            .ok_or_else(|| "artifact has no change readout".to_owned())?;
        self.artifact
            .brains
            .iter()
            .find(|brain| brain.role == "no_change")
            .ok_or_else(|| "artifact has no no_change readout".to_owned())?;
        let change_probability = brains
            .iter()
            .find(|value| value["role"] == "change")
            .and_then(|value| value["probability"].as_f64())
            .ok_or_else(|| "change readout result is missing".to_owned())?;
        let no_change_probability = brains
            .iter()
            .find(|value| value["role"] == "no_change")
            .and_then(|value| value["probability"].as_f64())
            .ok_or_else(|| "no_change readout result is missing".to_owned())?;
        let threshold = change.reaction_threshold;
        let margin = self.artifact.consensus_margin;
        let diagnostic_action = if readiness != "ready" {
            "hold"
        } else if change_probability >= threshold + margin
            && no_change_probability <= threshold - margin
        {
            "notify_change"
        } else if change_probability <= threshold - margin
            && no_change_probability >= threshold + margin
        {
            "suppress_no_change"
        } else {
            "hold"
        };
        let action = match diagnostic_action {
            "notify_change" => "notify",
            "suppress_no_change" => "silence",
            "hold" => "hold",
            _ => unreachable!("diagnostic action is normalized above"),
        };
        let external_action = if params.shadow { "hold" } else { action };
        let hold_reason = if readiness != "ready" {
            Some("insufficient-history")
        } else if params.shadow {
            Some("shadow")
        } else if action == "hold" {
            Some("consensus-pending")
        } else {
            None
        };
        if deadline_exceeded(deadline) {
            return Err(RATE_HELPER_TIMEOUT_MESSAGE.to_owned());
        }
        let position = previous_observation.as_ref().map_or_else(
            || {
                params.position.unwrap_or_else(|| {
                    self.state
                        .observations
                        .values()
                        .map(|observation| observation.position)
                        .max()
                        .unwrap_or(0)
                        .saturating_add(1)
                })
            },
            |previous| previous.position,
        );
        let decision = RateHelperObservationDecision {
            readiness: readiness.to_owned(),
            diagnostic_readiness: diagnostic_readiness.to_owned(),
            novelty: (readiness == "ready").then_some(change_probability),
            relevance: (readiness == "ready")
                .then_some(change_probability.max(no_change_probability)),
            action: action.to_owned(),
            diagnostic_action: diagnostic_action.to_owned(),
            external_action: external_action.to_owned(),
            hold_reason: hold_reason.map(str::to_owned),
            brains: brains.clone(),
        };
        if previous_observation.is_none() {
            let previous_state = self.state.clone();
            self.ensure_observation_capacity_for_insert()?;
            self.state.observations.insert(
                params.input_id.clone(),
                RateHelperObservation {
                    input_id: params.input_id.clone(),
                    image_sha256: image_sha256.clone(),
                    features: response_features.clone(),
                    timestamp_s: params.timestamp_s,
                    stream_id: params.stream_id.clone(),
                    position,
                    decision: None,
                },
            );
            self.state.observation_ledger.insert(
                params.input_id.clone(),
                RateHelperObservationLedgerEntry {
                    input_id: params.input_id.clone(),
                    digest,
                    decision: decision.clone(),
                    body_status: Some(RateHelperObservationBodyStatus::Present),
                },
            );
            let max_history = self.history_capacity();
            let history = self
                .state
                .histories
                .entry(params.stream_id.clone())
                .or_default();
            history.push(FrozenReadoutHistoryEntry {
                features: response_features,
                timestamp_s: params.timestamp_s,
            });
            if history.len() > max_history {
                let remove_count = history.len() - max_history;
                history.drain(..remove_count);
            }
            if deadline_exceeded(deadline) {
                self.state = previous_state;
                return Err(RATE_HELPER_TIMEOUT_MESSAGE.to_owned());
            }
            let persistence_started = Instant::now();
            if let Err(error) = self.persist_state() {
                let message = error.to_string();
                if !error.committed {
                    self.state = previous_state;
                }
                self.persistence_error = Some(message.clone());
                return Err(message);
            }
            let persistence_ms = persistence_started.elapsed().as_secs_f64() * 1_000.0;
            return Ok(self.observation_response(
                &params,
                &observation,
                &image_sha256,
                &decision,
                position,
                false,
                true,
                None,
                Some(persistence_ms),
                observation_has_ocr_text,
                context_present,
                params.shadow,
                params.repeat_index,
                &brains,
                &timing,
                response_cache_hit,
                case_match_count,
                case_adjustment,
            ));
        }
        Ok(self.observation_response(
            &params,
            &observation,
            &image_sha256,
            &decision,
            position,
            true,
            true,
            None,
            None,
            observation_has_ocr_text,
            context_present,
            params.shadow,
            params.repeat_index,
            &brains,
            &timing,
            response_cache_hit,
            case_match_count,
            case_adjustment,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_early_observation(
        &mut self,
        params: &RateObserveParams,
        observation: &RateObservationPayload,
        decision: &RateHelperObservationDecision,
        digest: &str,
        throttle_every: Option<u32>,
        observation_has_ocr_text: bool,
        context_present: bool,
        deadline: Option<Instant>,
    ) -> Result<Value, String> {
        if let Some(previous) = self.state.observation_ledger.get(&params.input_id) {
            if previous.digest != digest {
                return Err(
                    "input_id replay does not match the canonical observation digest".to_owned(),
                );
            }
            return Ok(self.early_observation_response(
                params,
                observation,
                &previous.decision,
                digest,
                throttle_every,
                observation_has_ocr_text,
                context_present,
                true,
                None,
            ));
        }
        if self.state.observations.contains_key(&params.input_id) {
            return Err(
                "existing observation has no canonical replay ledger entry; refusing to reprocess it"
                    .to_owned(),
            );
        }
        self.ensure_observation_ledger_capacity(&params.input_id)?;
        let previous_state = self.state.clone();
        self.state.observation_ledger.insert(
            params.input_id.clone(),
            RateHelperObservationLedgerEntry {
                input_id: params.input_id.clone(),
                digest: digest.to_owned(),
                decision: decision.clone(),
                body_status: Some(RateHelperObservationBodyStatus::NotFeedable),
            },
        );
        if deadline_exceeded(deadline) {
            self.state = previous_state;
            return Err(RATE_HELPER_TIMEOUT_MESSAGE.to_owned());
        }
        let persistence_started = Instant::now();
        if let Err(error) = self.persist_state() {
            let message = error.to_string();
            if !error.committed {
                self.state = previous_state;
            }
            self.persistence_error = Some(message.clone());
            return Err(message);
        }
        let persistence_ms = persistence_started.elapsed().as_secs_f64() * 1_000.0;
        Ok(self.early_observation_response(
            params,
            observation,
            decision,
            digest,
            throttle_every,
            observation_has_ocr_text,
            context_present,
            false,
            Some(persistence_ms),
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn early_observation_response(
        &self,
        params: &RateObserveParams,
        observation: &RateObservationPayload,
        decision: &RateHelperObservationDecision,
        digest: &str,
        throttle_every: Option<u32>,
        observation_has_ocr_text: bool,
        context_present: bool,
        replayed: bool,
        persistence_ms: Option<f64>,
    ) -> Value {
        let mut response = json!({
            "input_id": params.input_id,
            "image_sha256": params.image_sha256.clone().unwrap_or_default(),
            "observation_digest": digest,
            "readiness": decision.readiness,
            "diagnostic_readiness": decision.diagnostic_readiness,
            "novelty": decision.novelty,
            "relevance": decision.relevance,
            "action": decision.action,
            "diagnostic_action": decision.diagnostic_action,
            "external_action": decision.external_action,
            "hold_reason": decision.hold_reason,
            "feature_schema": habitua_connectome::RATE_HELPER_OBSERVATION_SCHEMA,
            "observation_kind": observation.kind,
            "observation_has_ocr_text": observation_has_ocr_text,
            "context_present": context_present,
            "shadow": params.shadow,
            "repeat_index": params.repeat_index,
            "stream_id": params.stream_id,
            "replayed": replayed,
            "brains": decision.brains,
            "response_groups": self.response_group_contract(),
            "response_feature_dimension": self.artifact.brains[0].feature_dimension,
            "history_feature_dimension": 4,
            "consensus_module": "two_role_consensus",
            "persistence_ms": persistence_ms,
            "artifact_version": self.artifact.artifact_version,
            "artifact_sha256": self.artifact_sha256,
            "feedable": false,
            "feed_rejection_code": "observation_not_feedable",
        });
        if let Some(every) = throttle_every {
            response["throttle_every"] = json!(every);
            response["skipped"] = json!(true);
            response["reason"] = json!("repeat_input_thinned_before_cns");
        } else if observation.kind == "audio" {
            response["reason"] = json!("audio_not_supported_by_retina_rate_model");
        } else if observation.kind == "visual" && observation.app.is_none() {
            response["reason"] = json!("visual_app_missing");
        }
        response
    }

    fn ensure_observation_ledger_capacity(&self, input_id: &str) -> Result<(), String> {
        if self.state.observation_ledger.contains_key(input_id)
            || self.state.observation_ledger.len() < RATE_HELPER_MAX_OBSERVATION_LEDGER_ENTRIES
        {
            Ok(())
        } else {
            Err(format!(
                "observation ledger capacity exceeded; new input_id {input_id} is rejected"
            ))
        }
    }

    fn ensure_observation_capacity_for_insert(&mut self) -> Result<(), String> {
        if self.state.observations.len() < RATE_HELPER_MAX_OBSERVATIONS {
            return Ok(());
        }
        let required_removals = self
            .state
            .observations
            .len()
            .saturating_sub(RATE_HELPER_MAX_OBSERVATIONS - 1);
        let feedback_input_ids = self
            .state
            .feedback
            .values()
            .map(|feedback| feedback.input_id.as_str())
            .collect::<BTreeSet<_>>();
        let candidates = self
            .state
            .observations
            .iter()
            .filter(|(_, observation)| !feedback_input_ids.contains(observation.input_id.as_str()))
            .min_by(|(_, left), (_, right)| {
                left.position
                    .cmp(&right.position)
                    .then_with(|| left.timestamp_s.total_cmp(&right.timestamp_s))
                    .then_with(|| left.input_id.cmp(&right.input_id))
            })
            .map(|(input_id, _)| input_id.clone());
        if required_removals > 0 {
            let Some(input_id) = candidates else {
                return Err(
                    "observation body capacity reached and every retained observation has feedback; new observation is rejected"
                        .to_owned(),
                );
            };
            self.state.observations.remove(&input_id);
            if let Some(entry) = self.state.observation_ledger.get_mut(&input_id) {
                entry.body_status = Some(RateHelperObservationBodyStatus::Expired);
            }
        }
        Ok(())
    }

    fn history_capacity(&self) -> usize {
        self.artifact
            .history_recent_observations
            .saturating_mul(4)
            .max(self.artifact.history_recent_observations)
            .min(RATE_HELPER_MAX_HISTORY_ENTRIES_PER_STREAM)
    }

    fn ensure_history_stream_capacity(&self, stream_id: &str) -> Result<(), String> {
        if self.state.histories.contains_key(stream_id)
            || self.state.histories.len() < RATE_HELPER_MAX_HISTORY_STREAMS
        {
            Ok(())
        } else {
            Err(format!(
                "history stream capacity exceeded; new stream_id {stream_id} is rejected"
            ))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn observation_response(
        &self,
        params: &RateObserveParams,
        observation: &RateObservationPayload,
        image_sha256: &str,
        decision: &RateHelperObservationDecision,
        position: u64,
        replayed: bool,
        feedable: bool,
        feed_rejection_code: Option<&str>,
        persistence_ms: Option<f64>,
        observation_has_ocr_text: bool,
        context_present: bool,
        shadow: bool,
        repeat_index: u32,
        brains: &[Value],
        timing: &(f64, f64, f64),
        response_cache_hit: bool,
        case_match_count: usize,
        case_adjustment: f64,
    ) -> Value {
        let mut response = json!({
            "input_id": params.input_id,
            "activity": self.activity_display.response(&params.input_id, image_sha256, response_cache_hit, replayed),
            "image_sha256": image_sha256,
            "readiness": decision.readiness,
            "diagnostic_readiness": decision.diagnostic_readiness,
            "novelty": decision.novelty,
            "relevance": decision.relevance,
            "action": decision.action,
            "diagnostic_action": decision.diagnostic_action,
            "external_action": decision.external_action,
            "hold_reason": decision.hold_reason,
            "feature_schema": habitua_connectome::RATE_HELPER_OBSERVATION_SCHEMA,
            "observation_kind": observation.kind,
            "observation_has_ocr_text": observation_has_ocr_text,
            "context_present": context_present,
            "shadow": shadow,
            "repeat_index": repeat_index,
            "stream_id": params.stream_id,
            "position": position,
            "brains": brains,
            "replayed": replayed,
            "response_groups": self.response_group_contract(),
            "response_feature_dimension": self.artifact.brains[0].feature_dimension,
            "history_feature_dimension": 4,
            "case_memory": {
                "match_policy": self.artifact.case_memory.match_policy,
                "exact_hash_match_count": case_match_count,
                "near_match_used": false,
                "near_match_rejected": true,
                "adjustment_change_logit": case_adjustment,
                "adjustment_no_change_logit": -case_adjustment,
                "time_origin": "feedback.event_time_s",
            },
            "rate_timing_ms": {
                "csr_traversal": timing.0,
                "neuron_update": timing.1,
                "other": timing.2,
            },
            "persistence_ms": persistence_ms,
            "state_file_bytes": self.state_file_size_bytes(),
            "response_cache": {
                "key": "image_sha256",
                "hit": response_cache_hit,
                "size": self.response_cache.len(),
            },
            "artifact_version": self.artifact.artifact_version,
            "artifact_sha256": self.artifact_sha256,
            "consensus_module": "two_role_consensus",
            "feedable": feedable,
        });
        if feedable {
            response["feed_target"] = json!("exact_image_sha256_case_memory");
        } else if let Some(code) = feed_rejection_code {
            response["feed_rejection_code"] = json!(code);
        }
        response
    }

    fn feed_with_deadline(
        &mut self,
        params: Value,
        deadline: Option<Instant>,
    ) -> Result<Value, String> {
        let params: RateFeedParams =
            serde_json::from_value(params).map_err(|error| error.to_string())?;
        validate_rate_feed_params(&params)?;
        if params.event_id.is_empty() || params.input_id.is_empty() {
            return Err("feed requires event_id and input_id".to_owned());
        }
        if self
            .state
            .feedback
            .get(&params.event_id)
            .is_some_and(|previous| previous.input_id != params.input_id)
        {
            return Err("event_id cannot be reassigned to another input_id".to_owned());
        }
        let sign = params.sign.to_ascii_lowercase();
        if sign != "reward" && sign != "punish" {
            return Err("feed sign must be reward or punish".to_owned());
        }
        if !params.strength.is_finite() || !(0.0..=1.0).contains(&params.strength) {
            return Err("feed strength must be finite and in [0, 1]".to_owned());
        }
        if !params.event_time_s.is_finite()
            || params.event_time_s < 0.0
            || !params.received_at_s.is_finite()
            || params.received_at_s < 0.0
        {
            return Err("feed timestamps must be finite".to_owned());
        }
        if params.source != "automatic" && params.source != "explicit" {
            return Err("feed source must be automatic or explicit".to_owned());
        }
        if let Some(entry) = self.state.observation_ledger.get(&params.input_id) {
            let status = entry.body_status.clone().unwrap_or_else(|| {
                if self.state.observations.contains_key(&params.input_id) {
                    RateHelperObservationBodyStatus::Present
                } else {
                    RateHelperObservationBodyStatus::Expired
                }
            });
            match status {
                RateHelperObservationBodyStatus::Present => {}
                RateHelperObservationBodyStatus::Expired => {
                    return Err(format!(
                        "observation body has expired; delayed feed for input_id {} is rejected",
                        params.input_id
                    ));
                }
                RateHelperObservationBodyStatus::NotFeedable => {
                    return Err(format!(
                        "observation is not eligible for feed; input_id {} used an early-return path",
                        params.input_id
                    ));
                }
            }
        }
        let observation = self
            .state
            .observations
            .get(&params.input_id)
            .ok_or_else(|| {
                if self.state.observation_ledger.contains_key(&params.input_id) {
                    "observation body has expired; delayed feed has no retained observation body"
                        .to_owned()
                } else {
                    "feed input_id has not been observed".to_owned()
                }
            })?
            .clone();
        if !self.state.feedback.contains_key(&params.event_id)
            && self.state.feedback.len() >= RATE_HELPER_MAX_FEEDBACK_ENTRIES
        {
            return Err(format!(
                "feedback capacity exceeded; new event_id {} is rejected",
                params.event_id
            ));
        }
        if deadline_exceeded(deadline) {
            return Err(RATE_HELPER_TIMEOUT_MESSAGE.to_owned());
        }
        let previous_state = self.state.clone();
        if !feedback_revision_is_newer(self.state.feedback.get(&params.event_id), params.revision) {
            let applied_at_s = unix_time_seconds()?;
            self.push_feedback_audit(RateHelperFeedbackAudit {
                event_id: params.event_id.clone(),
                input_id: params.input_id.clone(),
                sign: sign.clone(),
                strength: params.strength,
                event_time_s: params.event_time_s,
                revision: params.revision,
                cancelled: params.cancelled,
                source: params.source.clone(),
                received_at_s: params.received_at_s,
                applied_at_s,
                applied: false,
                reason: "duplicate_or_older_revision".to_owned(),
            });
            if deadline_exceeded(deadline) {
                self.state = previous_state;
                return Err(RATE_HELPER_TIMEOUT_MESSAGE.to_owned());
            }
            let persistence_started = Instant::now();
            if let Err(error) = self.persist_state() {
                let message = error.to_string();
                if !error.committed {
                    self.state = previous_state;
                }
                self.persistence_error = Some(message.clone());
                return Err(message);
            }
            return Ok(json!({
                "event_id": params.event_id,
                "input_id": params.input_id,
                "revision": params.revision,
                "cancelled": params.cancelled,
                "applied": false,
                "reason": "duplicate_or_older_revision",
                "source": params.source,
                "match_policy": self.artifact.case_memory.match_policy,
                "received_at_s": params.received_at_s,
                "applied_at_s": applied_at_s,
                "persistence_ms": persistence_started.elapsed().as_secs_f64() * 1_000.0,
                "state_file_bytes": self.state_file_size_bytes(),
            }));
        }
        let applied_at_s = unix_time_seconds()?;
        let feedback = RateHelperFeedback {
            event_id: params.event_id.clone(),
            input_id: params.input_id.clone(),
            image_sha256: observation.image_sha256.clone(),
            sign: sign.clone(),
            strength: if params.cancelled {
                0.0
            } else {
                params.strength
            },
            event_time_s: params.event_time_s,
            received_at_s: params.received_at_s,
            applied_at_s,
            revision: params.revision,
            cancelled: params.cancelled,
            source: params.source.clone(),
        };
        let previous = self
            .state
            .feedback
            .insert(params.event_id.clone(), feedback);
        if let Err(error) = self.rebuild_cases() {
            if let Some(previous) = previous {
                self.state.feedback.insert(params.event_id, previous);
            } else {
                self.state.feedback.remove(&params.event_id);
            }
            return Err(error.to_string());
        }
        self.push_feedback_audit(RateHelperFeedbackAudit {
            event_id: params.event_id.clone(),
            input_id: params.input_id.clone(),
            sign: sign.clone(),
            strength: params.strength,
            event_time_s: params.event_time_s,
            revision: params.revision,
            cancelled: params.cancelled,
            source: params.source.clone(),
            received_at_s: params.received_at_s,
            applied_at_s,
            applied: true,
            reason: "applied".to_owned(),
        });
        if deadline_exceeded(deadline) {
            self.state = previous_state;
            return Err(RATE_HELPER_TIMEOUT_MESSAGE.to_owned());
        }
        let persistence_started = Instant::now();
        if let Err(error) = self.persist_state() {
            let message = error.to_string();
            if !error.committed {
                self.state = previous_state;
            }
            self.persistence_error = Some(message.clone());
            return Err(message);
        }
        let case = self.state.cases.get(&observation.image_sha256);
        Ok(json!({
            "event_id": params.event_id,
            "input_id": params.input_id,
            "revision": params.revision,
            "cancelled": params.cancelled,
            "applied": true,
            "source": params.source,
            "match_policy": self.artifact.case_memory.match_policy,
            "case_image_sha256": observation.image_sha256,
            "case_event_count": case.map_or(0, |case| case.event_ids.len()),
            "case_memory_updated": !params.cancelled,
            "decay_origin": "event_time_s",
            "received_at_s": params.received_at_s,
            "applied_at_s": applied_at_s,
            "persistence_ms": persistence_started.elapsed().as_secs_f64() * 1_000.0,
            "state_file_bytes": self.state_file_size_bytes(),
        }))
    }

    fn rebuild_cases(&mut self) -> Result<(), Box<dyn Error>> {
        let mut cases = BTreeMap::<String, RateHelperCase>::new();
        for feedback in self.state.feedback.values() {
            if feedback.cancelled {
                continue;
            }
            let observation = self
                .state
                .observations
                .get(&feedback.input_id)
                .ok_or_else(|| format!("feedback {} has no observation", feedback.event_id))?;
            if observation.image_sha256 != feedback.image_sha256 {
                return Err(format!(
                    "feedback {} is not bound to the observed image hash",
                    feedback.event_id
                )
                .into());
            }
            let case = cases
                .entry(feedback.image_sha256.clone())
                .or_insert_with(|| RateHelperCase {
                    image_sha256: feedback.image_sha256.clone(),
                    features: observation.features.clone(),
                    event_ids: Vec::new(),
                });
            case.event_ids.push(feedback.event_id.clone());
        }
        for case in cases.values_mut() {
            case.event_ids.sort();
            case.event_ids.dedup();
        }
        if cases.len() > self.artifact.case_memory.max_cases {
            return Err("case memory max_cases would be exceeded".into());
        }
        self.state.cases = cases;
        Ok(())
    }

    fn push_feedback_audit(&mut self, audit: RateHelperFeedbackAudit) {
        self.state.feedback_history.push(audit);
        let excess = self
            .state
            .feedback_history
            .len()
            .saturating_sub(RATE_HELPER_MAX_AUDIT_ENTRIES);
        if excess > 0 {
            self.state.feedback_history.drain(..excess);
        }
    }

    fn state_file_size_bytes(&self) -> Option<u64> {
        fs::metadata(&self.state_path)
            .ok()
            .map(|metadata| metadata.len())
    }

    fn case_adjustment(&self, image_sha256: &str, now_s: f64) -> (f64, usize) {
        let Some(case) = self.state.cases.get(image_sha256) else {
            return (0.0, 0);
        };
        let mut adjustment = 0.0;
        for event_id in &case.event_ids {
            let Some(event) = self.state.feedback.get(event_id) else {
                continue;
            };
            if event.cancelled {
                continue;
            }
            let elapsed = (now_s - event.event_time_s).max(0.0);
            let sign = if event.sign == "reward" { 1.0 } else { -1.0 };
            adjustment += sign
                * event.strength
                * (-elapsed / self.artifact.case_memory.time_constant_seconds).exp();
        }
        (
            adjustment * self.artifact.case_memory.logit_scale,
            case.event_ids.len(),
        )
    }

    fn persist_state(&self) -> Result<(), StatePersistenceError> {
        if let Some(parent) = self
            .state_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            create_state_parent_directory(parent)
                .map_err(|error| StatePersistenceError::before_commit(error.to_string()))?;
        }
        let bytes = serde_json::to_vec_pretty(&self.state)
            .map_err(|error| StatePersistenceError::before_commit(error.to_string()))?;
        if bytes.len() > RATE_HELPER_MAX_STATE_BYTES {
            return Err(StatePersistenceError::before_commit(format!(
                "persisted state exceeds the {RATE_HELPER_MAX_STATE_BYTES}-byte limit"
            )));
        }
        let parent = self
            .state_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        atomic_replace_file_with_sync(&self.state_path, parent, &bytes, self.parent_directory_sync)
    }

    fn response_group_contract(&self) -> Vec<Value> {
        self.artifact
            .response_groups
            .iter()
            .map(|group| {
                json!({
                    "name": group.name,
                    "cell_count": group.neuron_indices.len(),
                    "rest_scale": group.scale,
                    "components": [
                        "mean_scaled_delta",
                        "rms_scaled_delta",
                        "absolute_mean_scaled_delta",
                        "active_fraction",
                    ],
                    "projection_dimension": self.artifact.response_projection_dimension,
                })
            })
            .collect()
    }
}

fn resolve_pack_path(
    artifact_path: &Path,
    artifact_pack_path: &str,
    pack_override: Option<&Path>,
) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(path) = pack_override {
        return Ok(path.to_path_buf());
    }
    let path = Path::new(artifact_pack_path);
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        let artifact_absolute = std::path::absolute(artifact_path)?;
        Ok(artifact_absolute
            .parent()
            .expect("absolute path has parent")
            .join(path))
    }
}

#[derive(Debug)]
struct StatePersistenceError {
    message: String,
    committed: bool,
}

impl StatePersistenceError {
    fn before_commit(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            committed: false,
        }
    }

    fn after_commit(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            committed: true,
        }
    }
}

impl std::fmt::Display for StatePersistenceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let phase = if self.committed {
            "state committed; durability unknown"
        } else {
            "state not committed"
        };
        write!(formatter, "{phase}: {}", self.message)
    }
}

impl Error for StatePersistenceError {}

fn create_state_parent_directory(parent: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        match fs::DirBuilder::new().mode(0o700).create(parent) {
            Ok(()) => fs::set_permissions(parent, fs::Permissions::from_mode(0o700)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && parent.is_dir() => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let ancestor = parent
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                create_state_parent_directory(ancestor)?;
                create_state_parent_directory(parent)
            }
            Err(error) => Err(error),
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(parent)
    }
}

fn create_state_temporary_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let temporary = options.open(path)?;
    #[cfg(unix)]
    temporary.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(temporary)
}

#[cfg(test)]
fn atomic_replace_file(
    path: &Path,
    parent: &Path,
    bytes: &[u8],
) -> Result<(), StatePersistenceError> {
    atomic_replace_file_with_sync(path, parent, bytes, sync_parent_directory)
}

fn sync_parent_directory(parent: &Path) -> io::Result<()> {
    File::open(parent).and_then(|file| file.sync_all())
}

fn atomic_replace_file_with_sync(
    path: &Path,
    parent: &Path,
    bytes: &[u8],
    parent_directory_sync: ParentDirectorySync,
) -> Result<(), StatePersistenceError> {
    let file_name = path
        .file_name()
        .ok_or_else(|| {
            StatePersistenceError::before_commit("rate helper state path has no file name")
        })?
        .to_string_lossy();
    let temporary_path = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        unix_time_nanos().map_err(StatePersistenceError::before_commit)?
    ));
    let write_result = (|| -> Result<(), StatePersistenceError> {
        let mut temporary = create_state_temporary_file(&temporary_path)
            .map_err(|error| StatePersistenceError::before_commit(error.to_string()))?;
        temporary
            .write_all(bytes)
            .map_err(|error| StatePersistenceError::before_commit(error.to_string()))?;
        temporary
            .sync_all()
            .map_err(|error| StatePersistenceError::before_commit(error.to_string()))?;
        fs::rename(&temporary_path, path)
            .map_err(|error| StatePersistenceError::before_commit(error.to_string()))?;
        parent_directory_sync(parent)
            .map_err(|error| StatePersistenceError::after_commit(error.to_string()))?;
        Ok(())
    })();
    if matches!(&write_result, Err(error) if !error.committed) {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result
}

fn validate_state_artifact_binding(
    state: &RateHelperPersistedState,
    artifact_sha256: &str,
) -> Result<(), Box<dyn Error>> {
    if state.artifact_sha256.is_empty() {
        return Err(
            "rate helper persisted state has an empty artifact hash; refusing to rebind it".into(),
        );
    }
    if state.artifact_sha256 != artifact_sha256 {
        return Err("rate helper state belongs to a different artifact".into());
    }
    Ok(())
}

fn load_persisted_state(
    path: &std::path::Path,
) -> Result<RateHelperPersistedState, Box<dyn Error>> {
    let bytes = fs::read(path)?;
    if bytes.len() > RATE_HELPER_MAX_STATE_BYTES {
        return Err(format!(
            "persisted state exceeds the {RATE_HELPER_MAX_STATE_BYTES}-byte limit"
        )
        .into());
    }
    let mut state = serde_json::from_slice::<RateHelperPersistedState>(&bytes).map_err(|error| {
        format!(
            "rate helper state is corrupt or from an incompatible version; refusing to continue: {error}"
        )
    })?;
    if state.state_version != RATE_HELPER_STATE_VERSION {
        return Err(format!(
            "rate helper state version {} is unsupported; refusing to continue",
            state.state_version
        )
        .into());
    }
    normalize_legacy_observation_body_status(&mut state);
    validate_state_retention(&state)?;
    Ok(state)
}

fn normalize_legacy_observation_body_status(state: &mut RateHelperPersistedState) {
    for (input_id, entry) in &mut state.observation_ledger {
        if entry.body_status.is_none() {
            entry.body_status = Some(if state.observations.contains_key(input_id) {
                RateHelperObservationBodyStatus::Present
            } else {
                RateHelperObservationBodyStatus::Expired
            });
        }
    }
}

fn validate_state_retention(state: &RateHelperPersistedState) -> Result<(), Box<dyn Error>> {
    if state.artifact_sha256.is_empty() {
        return Err("persisted state has an empty artifact hash".into());
    }
    if !is_sha256_hex(&state.artifact_sha256) {
        return Err("persisted state artifact hash is not a canonical SHA-256".into());
    }
    if state.histories.len() > RATE_HELPER_MAX_HISTORY_STREAMS {
        return Err(format!(
            "persisted history stream count {} exceeds capacity {}",
            state.histories.len(),
            RATE_HELPER_MAX_HISTORY_STREAMS
        )
        .into());
    }
    if state.observations.len() > RATE_HELPER_MAX_OBSERVATIONS {
        return Err(format!(
            "persisted observation body count {} exceeds capacity {}",
            state.observations.len(),
            RATE_HELPER_MAX_OBSERVATIONS
        )
        .into());
    }
    if state.observation_ledger.len() > RATE_HELPER_MAX_OBSERVATION_LEDGER_ENTRIES {
        return Err(format!(
            "persisted observation ledger count {} exceeds capacity {}",
            state.observation_ledger.len(),
            RATE_HELPER_MAX_OBSERVATION_LEDGER_ENTRIES
        )
        .into());
    }
    if state.feedback.len() > RATE_HELPER_MAX_FEEDBACK_ENTRIES {
        return Err(format!(
            "persisted feedback count {} exceeds capacity {}",
            state.feedback.len(),
            RATE_HELPER_MAX_FEEDBACK_ENTRIES
        )
        .into());
    }
    if state.feedback_history.len() > RATE_HELPER_MAX_AUDIT_ENTRIES {
        return Err(format!(
            "persisted feedback audit count {} exceeds capacity {}",
            state.feedback_history.len(),
            RATE_HELPER_MAX_AUDIT_ENTRIES
        )
        .into());
    }
    validate_state_history_capacity(state, RATE_HELPER_MAX_HISTORY_ENTRIES_PER_STREAM)?;
    validate_state_semantics(state)
}

fn validate_state_history_capacity(
    state: &RateHelperPersistedState,
    capacity: usize,
) -> Result<(), Box<dyn Error>> {
    if let Some((stream_id, history)) = state
        .histories
        .iter()
        .find(|(_, history)| history.len() > capacity)
    {
        return Err(format!(
            "persisted history for {stream_id} has {} entries; capacity is {capacity}",
            history.len()
        )
        .into());
    }
    Ok(())
}

fn state_validation_error(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::new(io::ErrorKind::InvalidData, message.into()))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_state_decision(decision: &RateHelperObservationDecision) -> Result<(), String> {
    for (name, value) in [
        ("persisted decision readiness", decision.readiness.as_str()),
        (
            "persisted decision diagnostic_readiness",
            decision.diagnostic_readiness.as_str(),
        ),
        ("persisted decision action", decision.action.as_str()),
        (
            "persisted decision diagnostic_action",
            decision.diagnostic_action.as_str(),
        ),
        (
            "persisted decision external_action",
            decision.external_action.as_str(),
        ),
    ] {
        ensure_bounded_string(value, name, RATE_HELPER_MAX_IDENTIFIER_BYTES)?;
    }
    if let Some(reason) = &decision.hold_reason {
        ensure_bounded_string(
            reason,
            "persisted decision hold_reason",
            RATE_HELPER_MAX_REASON_BYTES,
        )?;
    }
    if !matches!(
        decision.readiness.as_str(),
        "ready" | "insufficient-history" | "unsupported-input" | "input-insufficient"
    ) {
        return Err(format!(
            "persisted decision has unsupported readiness {}",
            decision.readiness
        ));
    }
    if !matches!(decision.action.as_str(), "notify" | "silence" | "hold") {
        return Err(format!(
            "persisted decision has unsupported action {}",
            decision.action
        ));
    }
    if !matches!(
        decision.diagnostic_action.as_str(),
        "notify_change" | "suppress_no_change" | "hold"
    ) {
        return Err(format!(
            "persisted decision has unsupported diagnostic_action {}",
            decision.diagnostic_action
        ));
    }
    if !matches!(
        decision.external_action.as_str(),
        "notify" | "silence" | "hold"
    ) {
        return Err(format!(
            "persisted decision has unsupported external_action {}",
            decision.external_action
        ));
    }
    if decision.action == "hold" && decision.hold_reason.is_none() {
        return Err("persisted hold decision has no hold_reason".to_owned());
    }
    // Shadow keeps the internal verdict while holding external delivery.
    let shadow_hold = decision.readiness == "ready"
        && decision.diagnostic_readiness == "evaluated"
        && decision.novelty.is_some()
        && decision.relevance.is_some()
        && decision.external_action == "hold"
        && decision.hold_reason.as_deref() == Some("shadow")
        && matches!(
            (
                decision.action.as_str(),
                decision.diagnostic_action.as_str()
            ),
            ("notify", "notify_change") | ("silence", "suppress_no_change")
        );
    if decision.action != "hold" && decision.hold_reason.is_some() && !shadow_hold {
        return Err("persisted non-hold decision has a hold_reason".to_owned());
    }
    if decision.brains.len() > 16 {
        return Err("persisted decision has too many brains".to_owned());
    }
    for (name, value) in [
        ("novelty", decision.novelty),
        ("relevance", decision.relevance),
    ] {
        if let Some(value) = value
            && (!value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(format!("persisted decision {name} is not in [0, 1]"));
        }
    }
    for brain in &decision.brains {
        let Some(object) = brain.as_object() else {
            return Err("persisted brain decision is not a JSON object".to_owned());
        };
        for field in ["id", "role"] {
            if let Some(value) = object.get(field) {
                let Some(value) = value.as_str() else {
                    return Err(format!("persisted brain {field} is not a string"));
                };
                ensure_bounded_string(
                    value,
                    &format!("persisted brain {field}"),
                    RATE_HELPER_MAX_IDENTIFIER_BYTES,
                )?;
            }
        }
        for field in ["base_probability", "probability", "threshold"] {
            if let Some(value) = object.get(field) {
                let Some(value) = value.as_f64() else {
                    return Err(format!("persisted brain {field} is not numeric"));
                };
                if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                    return Err(format!("persisted brain {field} is not in [0, 1]"));
                }
            }
        }
        if let Some(value) = object.get("adjustment") {
            let Some(value) = value.as_f64() else {
                return Err("persisted brain adjustment is not numeric".to_owned());
            };
            if !value.is_finite() {
                return Err("persisted brain adjustment is not finite".to_owned());
            }
        }
    }
    Ok(())
}

fn validate_state_semantics(state: &RateHelperPersistedState) -> Result<(), Box<dyn Error>> {
    ensure_bounded_string(
        &state.artifact_sha256,
        "persisted artifact_sha256",
        RATE_HELPER_MAX_IDENTIFIER_BYTES,
    )
    .map_err(state_validation_error)?;
    for (key, observation) in &state.observations {
        ensure_bounded_string(
            key,
            "persisted observation key",
            RATE_HELPER_MAX_IDENTIFIER_BYTES,
        )
        .map_err(state_validation_error)?;
        ensure_bounded_string(
            &observation.input_id,
            "persisted observation input_id",
            RATE_HELPER_MAX_IDENTIFIER_BYTES,
        )
        .map_err(state_validation_error)?;
        ensure_bounded_string(
            &observation.image_sha256,
            "persisted observation image_sha256",
            RATE_HELPER_MAX_IDENTIFIER_BYTES,
        )
        .map_err(state_validation_error)?;
        ensure_bounded_string(
            &observation.stream_id,
            "persisted observation stream_id",
            RATE_HELPER_MAX_IDENTIFIER_BYTES,
        )
        .map_err(state_validation_error)?;
        if key != &observation.input_id {
            return Err(state_validation_error(format!(
                "persisted observation key {key} does not match input_id {}",
                observation.input_id
            )));
        }
        if observation.features.len() > RATE_HELPER_MAX_FEATURE_VALUES
            || observation.features.iter().any(|value| !value.is_finite())
            || !observation.timestamp_s.is_finite()
            || observation.timestamp_s < 0.0
        {
            return Err(state_validation_error(format!(
                "persisted observation {key} has invalid numeric data"
            )));
        }
        if let Some(decision) = &observation.decision {
            validate_state_decision(decision).map_err(state_validation_error)?;
        }
        if let Some(entry) = state.observation_ledger.get(key)
            && entry
                .body_status
                .as_ref()
                .is_some_and(|status| !matches!(status, RateHelperObservationBodyStatus::Present))
        {
            return Err(state_validation_error(format!(
                "persisted observation {key} has a non-present ledger body status"
            )));
        }
    }
    for (key, entry) in &state.observation_ledger {
        ensure_bounded_string(
            key,
            "persisted ledger key",
            RATE_HELPER_MAX_IDENTIFIER_BYTES,
        )
        .map_err(state_validation_error)?;
        ensure_bounded_string(
            &entry.input_id,
            "persisted ledger input_id",
            RATE_HELPER_MAX_IDENTIFIER_BYTES,
        )
        .map_err(state_validation_error)?;
        if key != &entry.input_id {
            return Err(state_validation_error(format!(
                "persisted ledger key {key} does not match input_id {}",
                entry.input_id
            )));
        }
        if !is_sha256_hex(&entry.digest) {
            return Err(state_validation_error(format!(
                "persisted ledger entry {key} has a non-canonical digest"
            )));
        }
        validate_state_decision(&entry.decision).map_err(state_validation_error)?;
        let body_exists = state.observations.contains_key(key);
        match entry.body_status.as_ref() {
            Some(RateHelperObservationBodyStatus::Present) if !body_exists => {
                return Err(state_validation_error(format!(
                    "persisted ledger entry {key} says body is present but it is missing"
                )));
            }
            Some(RateHelperObservationBodyStatus::Expired)
            | Some(RateHelperObservationBodyStatus::NotFeedable)
                if body_exists =>
            {
                return Err(state_validation_error(format!(
                    "persisted ledger entry {key} has a missing-body status but a body is present"
                )));
            }
            _ => {}
        }
    }
    for (key, history) in &state.histories {
        ensure_bounded_string(
            key,
            "persisted history stream_id",
            RATE_HELPER_MAX_IDENTIFIER_BYTES,
        )
        .map_err(state_validation_error)?;
        for entry in history {
            if entry.features.len() > RATE_HELPER_MAX_FEATURE_VALUES
                || entry.features.iter().any(|value| !value.is_finite())
                || !entry.timestamp_s.is_finite()
                || entry.timestamp_s < 0.0
            {
                return Err(state_validation_error(format!(
                    "persisted history {key} has invalid numeric data"
                )));
            }
        }
    }
    for (key, feedback) in &state.feedback {
        ensure_bounded_string(
            key,
            "persisted feedback key",
            RATE_HELPER_MAX_IDENTIFIER_BYTES,
        )
        .map_err(state_validation_error)?;
        for (name, value) in [
            ("persisted feedback event_id", feedback.event_id.as_str()),
            ("persisted feedback input_id", feedback.input_id.as_str()),
            (
                "persisted feedback image_sha256",
                feedback.image_sha256.as_str(),
            ),
        ] {
            ensure_bounded_string(value, name, RATE_HELPER_MAX_IDENTIFIER_BYTES)
                .map_err(state_validation_error)?;
        }
        ensure_bounded_string(
            &feedback.source,
            "persisted feedback source",
            RATE_HELPER_MAX_SOURCE_BYTES,
        )
        .map_err(state_validation_error)?;
        ensure_bounded_string(
            &feedback.sign,
            "persisted feedback sign",
            RATE_HELPER_MAX_REASON_BYTES,
        )
        .map_err(state_validation_error)?;
        if key != &feedback.event_id {
            return Err(state_validation_error(format!(
                "persisted feedback key {key} does not match event_id {}",
                feedback.event_id
            )));
        }
        if !feedback.strength.is_finite()
            || !(0.0..=1.0).contains(&feedback.strength)
            || !feedback.event_time_s.is_finite()
            || feedback.event_time_s < 0.0
            || !feedback.received_at_s.is_finite()
            || feedback.received_at_s < 0.0
            || !feedback.applied_at_s.is_finite()
            || feedback.applied_at_s < 0.0
        {
            return Err(state_validation_error(format!(
                "persisted feedback {key} has invalid numeric data"
            )));
        }
        let observation = state.observations.get(&feedback.input_id).ok_or_else(|| {
            state_validation_error(format!("persisted feedback {key} has no observation body"))
        })?;
        if observation.image_sha256 != feedback.image_sha256 {
            return Err(state_validation_error(format!(
                "persisted feedback {key} image hash does not match its observation"
            )));
        }
        if let Some(entry) = state.observation_ledger.get(&feedback.input_id)
            && !matches!(
                entry.body_status,
                None | Some(RateHelperObservationBodyStatus::Present)
            )
        {
            return Err(state_validation_error(format!(
                "persisted feedback {key} references an unavailable observation body"
            )));
        }
    }
    for audit in &state.feedback_history {
        for (name, value) in [
            ("persisted audit event_id", audit.event_id.as_str()),
            ("persisted audit input_id", audit.input_id.as_str()),
            ("persisted audit sign", audit.sign.as_str()),
        ] {
            ensure_bounded_string(value, name, RATE_HELPER_MAX_IDENTIFIER_BYTES)
                .map_err(state_validation_error)?;
        }
        ensure_bounded_string(
            &audit.source,
            "persisted audit source",
            RATE_HELPER_MAX_SOURCE_BYTES,
        )
        .map_err(state_validation_error)?;
        ensure_bounded_string(
            &audit.reason,
            "persisted audit reason",
            RATE_HELPER_MAX_REASON_BYTES,
        )
        .map_err(state_validation_error)?;
        if !audit.strength.is_finite()
            || !(0.0..=1.0).contains(&audit.strength)
            || !audit.event_time_s.is_finite()
            || audit.event_time_s < 0.0
            || !audit.received_at_s.is_finite()
            || audit.received_at_s < 0.0
            || !audit.applied_at_s.is_finite()
            || audit.applied_at_s < 0.0
        {
            return Err(state_validation_error(
                "persisted feedback audit has invalid numeric data",
            ));
        }
    }
    for (key, case) in &state.cases {
        ensure_bounded_string(
            key,
            "persisted case image_sha256",
            RATE_HELPER_MAX_IDENTIFIER_BYTES,
        )
        .map_err(state_validation_error)?;
        if key != &case.image_sha256 {
            return Err(state_validation_error(format!(
                "persisted case key {key} does not match image_sha256 {}",
                case.image_sha256
            )));
        }
        if case.features.len() > RATE_HELPER_MAX_FEATURE_VALUES
            || case.features.iter().any(|value| !value.is_finite())
        {
            return Err(state_validation_error(format!(
                "persisted case {key} has invalid feature data"
            )));
        }
        for event_id in &case.event_ids {
            ensure_bounded_string(
                event_id,
                "persisted case event_id",
                RATE_HELPER_MAX_IDENTIFIER_BYTES,
            )
            .map_err(state_validation_error)?;
            let feedback = state.feedback.get(event_id).ok_or_else(|| {
                state_validation_error(format!(
                    "persisted case {key} references unknown feedback {event_id}"
                ))
            })?;
            if feedback.image_sha256 != *key {
                return Err(state_validation_error(format!(
                    "persisted case {key} references feedback for another image"
                )));
            }
        }
    }
    Ok(())
}

fn unix_time_seconds() -> Result<f64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .map_err(|error| format!("system clock is before UNIX epoch: {error}"))
}

fn unix_time_nanos() -> Result<u128, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .map_err(|error| format!("system clock is before UNIX epoch: {error}"))
}

fn helper_response_features(
    activity: &[f64],
    baseline: &[f64],
    artifact: &RateHelperArtifact,
) -> Result<Vec<f32>, String> {
    if activity.len() != baseline.len() {
        return Err("rate activity and rest activity dimensions differ".to_owned());
    }
    let mut features = Vec::with_capacity(
        artifact.response_groups.len() * (4 + artifact.response_projection_dimension),
    );
    for group in &artifact.response_groups {
        let count = group.neuron_indices.len().max(1) as f64;
        let mut scaled_sum = 0.0;
        let mut scaled_squared = 0.0;
        let mut absolute_sum = 0.0;
        let mut active_count = 0_usize;
        for neuron in &group.neuron_indices {
            let delta = activity[*neuron as usize] - baseline[*neuron as usize];
            let scaled = delta / group.scale;
            scaled_sum += scaled;
            scaled_squared += scaled * scaled;
            absolute_sum += scaled.abs();
            active_count += usize::from(delta.abs() > artifact.zero_response_epsilon);
        }
        features.extend([
            (scaled_sum / count) as f32,
            (scaled_squared / count).sqrt() as f32,
            (absolute_sum / count) as f32,
            active_count as f32 / count as f32,
        ]);
        for bucket in 0..artifact.response_projection_dimension {
            let start =
                bucket * group.neuron_indices.len() / artifact.response_projection_dimension;
            let end =
                (bucket + 1) * group.neuron_indices.len() / artifact.response_projection_dimension;
            let bucket_count = end.saturating_sub(start).max(1) as f64;
            let bucket_mean = group.neuron_indices[start..end]
                .iter()
                .map(|neuron| {
                    (activity[*neuron as usize] - baseline[*neuron as usize]) / group.scale
                })
                .sum::<f64>()
                / bucket_count;
            features.push(bucket_mean as f32);
        }
    }
    if features.iter().any(|value| !value.is_finite()) {
        return Err("rate response features contain a non-finite value".to_owned());
    }
    Ok(features)
}

fn helper_brain_probability(
    brain: &RateHelperBrain,
    augmented_features: &[f32],
) -> Result<f64, String> {
    if brain.weights.len() != augmented_features.len()
        || brain.standardization_mean.len() != augmented_features.len()
        || brain.standardization_scale.len() != augmented_features.len()
    {
        return Err(format!("readout {} feature dimension mismatch", brain.id));
    }
    let logit = brain.bias
        + brain
            .weights
            .iter()
            .zip(augmented_features)
            .zip(&brain.standardization_mean)
            .zip(&brain.standardization_scale)
            .map(|(((weight, feature), mean), scale)| weight * (f64::from(*feature) - mean) / scale)
            .sum::<f64>();
    Ok(sigmoid(logit))
}

fn sigmoid(logit: f64) -> f64 {
    if logit >= 0.0 {
        let e = (-logit).exp();
        1.0 / (1.0 + e)
    } else {
        let e = logit.exp();
        e / (1.0 + e)
    }
}

fn logit(probability: f64) -> f64 {
    probability.clamp(1.0e-12, 1.0 - 1.0e-12).ln()
        - (1.0 - probability.clamp(1.0e-12, 1.0 - 1.0e-12)).ln()
}

fn read_rate_image(path: &std::path::Path) -> Result<RgbImage, Box<dyn Error>> {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("png") => Ok(RgbImage::read_png(path)?),
        _ => Ok(RgbImage::read_ppm(path)?),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn feedback_revision_is_newer(previous: Option<&RateHelperFeedback>, revision: u64) -> bool {
    previous.is_none_or(|event| revision > event.revision)
}

#[cfg(test)]
mod tests {
    use super::*;
    use habitua_connectome::{RateHelperResponseGroup, RateNeuronMetadata, RetinaMapConfig};

    #[test]
    fn pack_resolution_uses_override_and_artifact_directory() {
        let artifact = Path::new("/tmp/connectome-distribution/artifact.json");
        assert_eq!(
            resolve_pack_path(artifact, "rate-full", None).unwrap(),
            Path::new("/tmp/connectome-distribution/rate-full")
        );
        assert_eq!(
            resolve_pack_path(artifact, "rate-full", Some(Path::new("/tmp/chosen-pack"))).unwrap(),
            Path::new("/tmp/chosen-pack")
        );
    }

    #[test]
    fn pack_validation_rejects_missing_and_mismatched_manifest_before_graph() {
        let root = env::temp_dir().join(format!(
            "habitua-pack-startup-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let artifact_path = root.join("artifact.json");
        let mut artifact = test_runtime().artifact;
        artifact.pack_path = "rate-full".to_owned();
        fs::write(&artifact_path, serde_json::to_vec(&artifact).unwrap()).unwrap();
        let pack = root.join("rate-full");
        let state = root.join("state.json");
        let error = RateHelperRuntime::load(&artifact_path, state.clone(), None)
            .err()
            .unwrap();
        assert_eq!(
            error.downcast_ref::<RateStartupError>().unwrap().code,
            "pack-missing"
        );
        fs::create_dir_all(&pack).unwrap();
        fs::write(pack.join("rate_manifest.json"), b"wrong manifest").unwrap();
        let error = RateHelperRuntime::load(&artifact_path, state, None)
            .err()
            .unwrap();
        assert_eq!(
            error.downcast_ref::<RateStartupError>().unwrap().code,
            "pack-manifest-mismatch"
        );
        fs::remove_dir_all(root).unwrap();
    }

    fn test_runtime() -> RateHelperRuntime {
        let graph = RateGraph::new(
            vec![1],
            vec![0, 0],
            Vec::new(),
            vec![0, 0],
            Vec::new(),
            vec![0],
            vec![RateNeuronMetadata {
                body_id: 1,
                type_name: None,
                class: None,
                superclass: None,
                subclass: None,
                soma_side: None,
                root_side: None,
                assigned_ol_hex1: None,
                assigned_ol_hex2: None,
                selected_neurotransmitter: None,
                neurotransmitter_source: "test".to_owned(),
                source_sign: 0,
            }],
            BTreeMap::new(),
        )
        .expect("graph");
        let artifact = RateHelperArtifact {
            observation_schema: "coosenpai-observation-v1".to_owned(),
            artifact_version: 1,
            pack_path: "test-pack".to_owned(),
            pack_manifest_sha256: "0".repeat(64),
            graph_fingerprint: graph.fingerprint(),
            retina_map: habitua_connectome::RetinaMap::from_graph(
                &graph,
                RetinaMapConfig::default(),
            )
            .expect("retina"),
            rate_steps_per_observation: 1,
            hmax: 10.0,
            response_projection_dimension: 8,
            history_recent_observations: 4,
            history_distance_threshold: 1.0,
            history_previous_absolute_difference_scale: 1.0,
            history_elapsed_time_scale_seconds: 60.0,
            history_max_age_seconds: 300.0,
            zero_response_epsilon: 1.0e-6,
            response_groups: vec![RateHelperResponseGroup {
                name: "test".to_owned(),
                neuron_indices: vec![0],
                scale: 1.0,
            }],
            brains: vec![
                RateHelperBrain {
                    id: "change".to_owned(),
                    role: "change".to_owned(),
                    feature_dimension: 12,
                    standardization_mean: vec![0.0; 16],
                    standardization_scale: vec![1.0; 16],
                    weights: vec![0.0; 16],
                    bias: 0.0,
                    reaction_threshold: 0.5,
                },
                RateHelperBrain {
                    id: "no-change".to_owned(),
                    role: "no_change".to_owned(),
                    feature_dimension: 12,
                    standardization_mean: vec![0.0; 16],
                    standardization_scale: vec![1.0; 16],
                    weights: vec![0.0; 16],
                    bias: 0.0,
                    reaction_threshold: 0.5,
                },
            ],
            consensus_margin: 0.1,
            case_memory: habitua_connectome::RateHelperCaseMemory {
                match_policy: "exact_image_sha256".to_owned(),
                time_constant_seconds: 10.0,
                logit_scale: 2.0,
                max_cases: 8,
            },
        };
        let artifact_sha256 = "a".repeat(64);
        let state = RateHelperPersistedState {
            artifact_sha256: artifact_sha256.clone(),
            ..RateHelperPersistedState::default()
        };
        RateHelperRuntime {
            artifact,
            artifact_sha256,
            activity_display: rate_activity_display::ActivityDisplay::for_graph(&graph),
            graph: graph.clone(),
            parameters: RateParameters::initial(&graph),
            rest_activity: vec![0.0],
            response_cache: BTreeMap::new(),
            state_path: PathBuf::from("target/connectome-helper-test-state.json"),
            state,
            persistence_error: None,
            parent_directory_sync: sync_parent_directory,
        }
    }

    fn retention_measurement_p95_ms(samples: &mut [f64]) -> f64 {
        samples.sort_by(f64::total_cmp);
        let index = ((samples.len() as f64 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(samples.len().saturating_sub(1));
        samples[index]
    }

    #[test]
    fn release_retention_state_persistence_measurement() {
        let root = PathBuf::from(format!(
            "target/connectome-helper-retention-measurement-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("retention measurement directory");
        type StatePopulate = fn(&mut RateHelperPersistedState);
        let cases: [(&str, StatePopulate); 6] = [
            ("normal", |_| {}),
            ("observations_near_cap", |state| {
                for index in 0..RATE_HELPER_MAX_OBSERVATIONS.saturating_sub(1) {
                    let input_id = format!("observation-{index}");
                    state.observations.insert(
                        input_id.clone(),
                        RateHelperObservation {
                            input_id,
                            image_sha256: "image".to_owned(),
                            features: vec![0.0; 12],
                            timestamp_s: index as f64,
                            stream_id: "stream".to_owned(),
                            position: index as u64,
                            decision: None,
                        },
                    );
                }
            }),
            ("ledger_near_cap", |state| {
                for index in 0..RATE_HELPER_MAX_OBSERVATION_LEDGER_ENTRIES.saturating_sub(1) {
                    let input_id = format!("input-{index}");
                    state.observation_ledger.insert(
                        input_id.clone(),
                        RateHelperObservationLedgerEntry {
                            input_id,
                            digest: "a".repeat(64),
                            decision: test_decision(),
                            body_status: Some(RateHelperObservationBodyStatus::NotFeedable),
                        },
                    );
                }
            }),
            ("histories_near_cap", |state| {
                for index in 0..RATE_HELPER_MAX_HISTORY_STREAMS.saturating_sub(1) {
                    state.histories.insert(
                        format!("stream-{index}"),
                        vec![FrozenReadoutHistoryEntry {
                            features: vec![0.0; 4],
                            timestamp_s: index as f64,
                        }],
                    );
                }
            }),
            ("feedback_near_cap", |state| {
                for index in 0..RATE_HELPER_MAX_FEEDBACK_ENTRIES.saturating_sub(1) {
                    let event_id = format!("event-{index}");
                    state.feedback.insert(
                        event_id.clone(),
                        RateHelperFeedback {
                            event_id,
                            input_id: "input".to_owned(),
                            image_sha256: "image".to_owned(),
                            sign: "reward".to_owned(),
                            strength: 0.5,
                            event_time_s: index as f64,
                            received_at_s: index as f64,
                            applied_at_s: index as f64,
                            revision: index as u64,
                            cancelled: false,
                            source: "automatic".to_owned(),
                        },
                    );
                }
            }),
            ("feedback_audit_near_cap", |state| {
                state.feedback_history = (0..RATE_HELPER_MAX_AUDIT_ENTRIES.saturating_sub(1))
                    .map(|index| RateHelperFeedbackAudit {
                        event_id: format!("event-{index}"),
                        input_id: "input".to_owned(),
                        sign: "reward".to_owned(),
                        strength: 0.5,
                        event_time_s: index as f64,
                        revision: index as u64,
                        cancelled: false,
                        source: "automatic".to_owned(),
                        received_at_s: index as f64,
                        applied_at_s: index as f64,
                        applied: true,
                        reason: "measurement".to_owned(),
                    })
                    .collect();
            }),
        ];
        for (name, populate) in cases {
            let mut runtime = test_runtime();
            runtime.state_path = root.join(format!("{name}.json"));
            populate(&mut runtime.state);
            let mut times = Vec::new();
            for _ in 0..8 {
                let started = Instant::now();
                runtime
                    .persist_state()
                    .expect("persist retention measurement state");
                times.push(started.elapsed().as_secs_f64() * 1_000.0);
            }
            let state_file_bytes = fs::metadata(&runtime.state_path)
                .expect("retention measurement state metadata")
                .len();
            println!(
                "RATE_HELPER_RETENTION_METRIC name={name} p95_persist_ms={:.3} state_file_bytes={state_file_bytes}",
                retention_measurement_p95_ms(&mut times)
            );
        }
        fs::remove_dir_all(root).expect("remove retention measurement directory");
    }

    fn fail_parent_directory_sync(_: &Path) -> io::Result<()> {
        Err(io::Error::other("injected parent directory sync failure"))
    }

    #[test]
    fn feedback_revision_correction_and_cancel_are_exact_hash_bound() {
        let mut runtime = test_runtime();
        runtime.state_path = PathBuf::from(format!(
            "target/connectome-helper-test-state-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&runtime.state_path);
        let image_sha256 = "image-a".to_owned();
        runtime.state.observations.insert(
            "input-a".to_owned(),
            RateHelperObservation {
                input_id: "input-a".to_owned(),
                image_sha256: image_sha256.clone(),
                features: vec![0.0; 12],
                timestamp_s: 1.0,
                stream_id: "stream".to_owned(),
                position: 1,
                decision: None,
            },
        );
        runtime.state.feedback.insert(
            "event-a".to_owned(),
            RateHelperFeedback {
                event_id: "event-a".to_owned(),
                input_id: "input-a".to_owned(),
                image_sha256: image_sha256.clone(),
                sign: "reward".to_owned(),
                strength: 1.0,
                event_time_s: 1.0,
                received_at_s: 2.0,
                applied_at_s: 3.0,
                revision: 0,
                cancelled: false,
                source: "explicit".to_owned(),
            },
        );
        runtime.rebuild_cases().expect("initial case");
        assert!(runtime.case_adjustment(&image_sha256, 1.0).0 > 0.0);
        assert!(feedback_revision_is_newer(
            runtime.state.feedback.get("event-a"),
            1
        ));
        assert!(!feedback_revision_is_newer(
            runtime.state.feedback.get("event-a"),
            0
        ));
        let duplicate = runtime
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-a",
                    "sign": "reward",
                    "strength": 1.0,
                    "event_time_s": 1.0,
                    "received_at_s": 2.5,
                    "revision": 0,
                    "cancelled": false,
                    "source": "automatic"
                }),
                None,
            )
            .expect("duplicate feed audit");
        assert_eq!(duplicate["applied"], false);
        assert_eq!(duplicate["source"], "automatic");
        assert_eq!(runtime.state.feedback_history.len(), 1);
        assert!(!runtime.state.feedback_history[0].applied);
        assert_eq!(runtime.state.feedback_history[0].sign, "reward");
        assert_eq!(runtime.state.feedback_history[0].strength, 1.0);
        assert_eq!(runtime.state.feedback_history[0].event_time_s, 1.0);
        let reassignment = runtime
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-b",
                    "sign": "punish",
                    "strength": 1.0,
                    "event_time_s": 1.0,
                    "received_at_s": 2.0,
                    "revision": 1
                    ,"cancelled": false
                    ,"source": "explicit"
                }),
                None,
            )
            .expect_err("an event must not move to another input");
        assert!(reassignment.contains("reassigned"));
        let correction = runtime
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-a",
                    "sign": "punish",
                    "strength": 1.0,
                    "event_time_s": 1.0,
                    "received_at_s": 3.0,
                    "revision": 1,
                    "cancelled": false,
                    "source": "explicit"
                }),
                None,
            )
            .expect("correction feed");
        assert_eq!(correction["applied"], true);
        runtime.rebuild_cases().expect("corrected case");
        assert!(runtime.case_adjustment(&image_sha256, 1.0).0 < 0.0);
        let cancellation = runtime
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-a",
                    "sign": "punish",
                    "strength": 0.0,
                    "event_time_s": 1.0,
                    "received_at_s": 4.0,
                    "revision": 2,
                    "cancelled": true,
                    "source": "explicit"
                }),
                None,
            )
            .expect("cancel feed");
        assert_eq!(cancellation["applied"], true);
        assert_eq!(runtime.case_adjustment(&image_sha256, 1.0), (0.0, 0));
        let restored = load_persisted_state(&runtime.state_path).expect("state restart load");
        assert_eq!(restored.feedback["event-a"].revision, 2);
        assert_eq!(restored.feedback_history.len(), 3);
        assert_eq!(restored.feedback["event-a"].source, "explicit");
        assert!(restored.feedback_history.iter().any(|audit| !audit.applied));
        fs::write(&runtime.state_path, b"incomplete state").expect("corrupt state fixture");
        let error = load_persisted_state(&runtime.state_path).expect_err("corrupt state rejected");
        assert!(error.to_string().contains("corrupt"));
        let mut old_state = runtime.state.clone();
        old_state.state_version = 1;
        fs::write(
            &runtime.state_path,
            serde_json::to_vec(&old_state).expect("old state bytes"),
        )
        .expect("old state fixture");
        let error = load_persisted_state(&runtime.state_path).expect_err("old state rejected");
        assert!(error.to_string().contains("version 1 is unsupported"));
        let _ = fs::remove_file(&runtime.state_path);
    }

    #[test]
    fn empty_state_artifact_hash_is_rejected_instead_of_rebound() {
        let state = RateHelperPersistedState::default();
        let error = validate_state_artifact_binding(&state, &"a".repeat(64))
            .expect_err("empty persisted artifact hash must be rejected");
        assert!(error.to_string().contains("empty artifact hash"));
    }

    #[cfg(unix)]
    #[test]
    fn persisted_state_and_created_directories_are_private() {
        const CHILD_ENV: &str = "HABITUA_TEST_STATE_STRICT_UMASK";
        if env::var_os(CHILD_ENV).is_none() {
            let output = std::process::Command::new(env::current_exe().expect("test executable"))
                .args([
                    "--exact",
                    "tests::persisted_state_and_created_directories_are_private",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .output()
                .expect("run strict umask test child");
            assert!(
                output.status.success(),
                "strict umask child failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        // SAFETY: this process runs only the selected test and exits after it finishes.
        unsafe { libc::umask(0o277) };

        let root = env::temp_dir().join(format!(
            "habitua-state-mode-{}-{}",
            std::process::id(),
            unix_time_nanos().expect("current time")
        ));
        fs::create_dir(&root).expect("existing state directory");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755))
            .expect("existing directory fixture");
        let nested = root.join("nested").join("deeper");
        let state_path = nested.join("state.json");
        let mut runtime = test_runtime();
        runtime.state_path = state_path.clone();

        runtime.persist_state().expect("initial state write");
        assert_eq!(
            fs::metadata(&root)
                .expect("existing directory")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        for directory in [root.join("nested"), nested.clone()] {
            assert_eq!(
                fs::metadata(&directory)
                    .expect("created directory")
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let temporary_path = nested.join("probe.tmp");
        let temporary = create_state_temporary_file(&temporary_path).expect("temporary file");
        assert_eq!(
            temporary
                .metadata()
                .expect("temporary file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(temporary);
        fs::remove_file(temporary_path).expect("remove temporary file fixture");
        assert_eq!(
            fs::metadata(&state_path)
                .expect("state file")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o644))
            .expect("simulate existing readable state");
        runtime.persist_state().expect("replace existing state");
        assert_eq!(
            fs::metadata(&state_path)
                .expect("replaced state")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(root).expect("remove state mode test directory");
    }

    #[test]
    fn rename_before_commit_failure_keeps_the_old_target_untouched() {
        let root = PathBuf::from(format!(
            "target/connectome-helper-persistence-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("persistence test directory");
        let target = root.join("state");
        fs::create_dir(&target).expect("old target directory");
        let error = atomic_replace_file(&target, &root, b"new state")
            .expect_err("rename onto an existing directory must fail before commit");
        assert!(!error.committed);
        assert!(target.is_dir());
        assert_eq!(
            fs::read_dir(&root)
                .expect("persistence test entries")
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp"))
                .count(),
            0
        );
        fs::remove_dir_all(root).expect("remove persistence test directory");
    }

    #[test]
    fn feedback_audit_history_is_bounded() {
        let mut runtime = test_runtime();
        for index in 0..(RATE_HELPER_MAX_AUDIT_ENTRIES + 3) {
            runtime.push_feedback_audit(RateHelperFeedbackAudit {
                event_id: format!("event-{index}"),
                input_id: "input".to_owned(),
                sign: "reward".to_owned(),
                strength: 1.0,
                event_time_s: index as f64,
                revision: index as u64,
                cancelled: false,
                source: "automatic".to_owned(),
                received_at_s: index as f64,
                applied_at_s: index as f64,
                applied: true,
                reason: "test".to_owned(),
            });
        }
        assert_eq!(
            runtime.state.feedback_history.len(),
            RATE_HELPER_MAX_AUDIT_ENTRIES
        );
        assert_eq!(runtime.state.feedback_history[0].event_id, "event-3");
    }

    #[test]
    fn feedback_revision_state_survives_audit_eviction_and_restart() {
        let root = PathBuf::from(format!(
            "target/connectome-helper-feedback-restart-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("feedback restart test directory");
        let mut runtime = test_runtime();
        runtime.state_path = root.join("state.json");
        runtime.state.observations.insert(
            "input-a".to_owned(),
            RateHelperObservation {
                input_id: "input-a".to_owned(),
                image_sha256: "image-a".to_owned(),
                features: vec![0.0; 12],
                timestamp_s: 1.0,
                stream_id: "stream".to_owned(),
                position: 1,
                decision: None,
            },
        );
        let first = runtime
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-a",
                    "sign": "reward",
                    "strength": 1.0,
                    "event_time_s": 1.0,
                    "received_at_s": 2.0,
                    "revision": 1,
                    "cancelled": false,
                    "source": "explicit"
                }),
                None,
            )
            .expect("initial feedback");
        assert_eq!(first["applied"], true);
        for index in 0..RATE_HELPER_MAX_AUDIT_ENTRIES {
            runtime.push_feedback_audit(RateHelperFeedbackAudit {
                event_id: format!("audit-{index}"),
                input_id: "input-a".to_owned(),
                sign: "reward".to_owned(),
                strength: 0.5,
                event_time_s: index as f64,
                revision: index as u64,
                cancelled: false,
                source: "test".to_owned(),
                received_at_s: index as f64,
                applied_at_s: index as f64,
                applied: true,
                reason: "eviction-fixture".to_owned(),
            });
        }
        let old_revision = runtime
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-a",
                    "sign": "reward",
                    "strength": 1.0,
                    "event_time_s": 1.0,
                    "received_at_s": 3.0,
                    "revision": 1,
                    "cancelled": false,
                    "source": "automatic"
                }),
                None,
            )
            .expect("old revision after audit eviction");
        let new_revision = runtime
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-a",
                    "sign": "punish",
                    "strength": 0.5,
                    "event_time_s": 1.0,
                    "received_at_s": 4.0,
                    "revision": 2,
                    "cancelled": false,
                    "source": "explicit"
                }),
                None,
            )
            .expect("new revision after audit eviction");
        let replay = runtime
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-a",
                    "sign": "punish",
                    "strength": 0.5,
                    "event_time_s": 1.0,
                    "received_at_s": 5.0,
                    "revision": 2,
                    "cancelled": false,
                    "source": "explicit"
                }),
                None,
            )
            .expect("replayed revision after audit eviction");
        assert_eq!(old_revision["applied"], false);
        assert_eq!(new_revision["applied"], true);
        assert_eq!(replay["applied"], false);
        assert_eq!(
            runtime.state.feedback_history.len(),
            RATE_HELPER_MAX_AUDIT_ENTRIES
        );
        let restored = load_persisted_state(&runtime.state_path).expect("restart state");
        let mut restarted = test_runtime();
        restarted.state_path = runtime.state_path.clone();
        restarted.state = restored;
        restarted.rebuild_cases().expect("restart cases");
        let old_after_restart = restarted
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-a",
                    "sign": "reward",
                    "strength": 1.0,
                    "event_time_s": 1.0,
                    "received_at_s": 6.0,
                    "revision": 1,
                    "cancelled": false,
                    "source": "automatic"
                }),
                None,
            )
            .expect("old revision after restart");
        let new_after_restart = restarted
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-a",
                    "sign": "reward",
                    "strength": 0.25,
                    "event_time_s": 1.0,
                    "received_at_s": 7.0,
                    "revision": 3,
                    "cancelled": false,
                    "source": "explicit"
                }),
                None,
            )
            .expect("new revision after restart");
        let replay_after_restart = restarted
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-a",
                    "sign": "reward",
                    "strength": 0.25,
                    "event_time_s": 1.0,
                    "received_at_s": 8.0,
                    "revision": 3,
                    "cancelled": false,
                    "source": "explicit"
                }),
                None,
            )
            .expect("replayed new revision after restart");
        assert_eq!(old_after_restart["applied"], false);
        assert_eq!(new_after_restart["applied"], true);
        assert_eq!(replay_after_restart["applied"], false);
        fs::remove_dir_all(root).expect("remove feedback restart test directory");
    }

    #[test]
    fn expired_feed_deadline_does_not_change_state() {
        let mut runtime = test_runtime();
        runtime.state.observations.insert(
            "input-a".to_owned(),
            RateHelperObservation {
                input_id: "input-a".to_owned(),
                image_sha256: "image-a".to_owned(),
                features: vec![0.0; 12],
                timestamp_s: 1.0,
                stream_id: "stream".to_owned(),
                position: 1,
                decision: None,
            },
        );
        let before = runtime.state.clone();
        let error = runtime
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-a",
                    "sign": "reward",
                    "strength": 1.0,
                    "event_time_s": 1.0,
                    "received_at_s": 2.0,
                    "revision": 1,
                    "cancelled": false,
                    "source": "explicit"
                }),
                Some(Instant::now() - Duration::from_secs(1)),
            )
            .expect_err("an expired request must not mutate state");
        assert_eq!(error, RATE_HELPER_TIMEOUT_MESSAGE);
        assert_eq!(
            serde_json::to_vec(&runtime.state.observations).expect("observations JSON"),
            serde_json::to_vec(&before.observations).expect("previous observations JSON")
        );
        assert_eq!(
            serde_json::to_vec(&runtime.state.feedback).expect("feedback JSON"),
            serde_json::to_vec(&before.feedback).expect("previous feedback JSON")
        );
        assert_eq!(
            serde_json::to_vec(&runtime.state.feedback_history).expect("feedback history JSON"),
            serde_json::to_vec(&before.feedback_history).expect("previous feedback history JSON")
        );
        assert_eq!(
            serde_json::to_vec(&runtime.state.cases).expect("cases JSON"),
            serde_json::to_vec(&before.cases).expect("previous cases JSON")
        );
    }

    #[test]
    fn cancelled_feed_still_validates_strength_range() {
        let mut runtime = test_runtime();
        runtime.state.observations.insert(
            "input-a".to_owned(),
            RateHelperObservation {
                input_id: "input-a".to_owned(),
                image_sha256: "image-a".to_owned(),
                features: vec![0.0; 12],
                timestamp_s: 1.0,
                stream_id: "stream".to_owned(),
                position: 1,
                decision: None,
            },
        );
        let error = runtime
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "input-a",
                    "sign": "reward",
                    "strength": 1.1,
                    "event_time_s": 1.0,
                    "received_at_s": 2.0,
                    "revision": 1,
                    "cancelled": true,
                    "source": "explicit"
                }),
                None,
            )
            .expect_err("cancelled feed strength is still bounded");
        assert!(error.contains("strength"));
        assert!(runtime.state.feedback.is_empty());
    }

    fn test_root(name: &str) -> PathBuf {
        PathBuf::from(format!(
            "target/connectome-helper-r20-{name}-{}",
            std::process::id()
        ))
    }

    fn write_test_ppm(root: &Path) -> PathBuf {
        let path = root.join("frame.ppm");
        fs::write(&path, b"P6\n1 1\n255\n\x33\x4c\x66").expect("test image");
        path
    }

    fn audio_request(input_id: &str) -> RateRequest {
        RateRequest {
            v: 1,
            id: format!("request-{input_id}"),
            session: Some("r20-test-session".to_owned()),
            op: "evaluate".to_owned(),
            params: serde_json::json!({
                "input_id": input_id,
                "image_path": "",
                "stream_id": "audio/main",
                "at_ms": 1000,
                "timestamp_s": 1.0,
                "feature_schema": habitua_connectome::RATE_HELPER_OBSERVATION_SCHEMA,
                "features": [],
                "observation": {
                    "frame_id": input_id,
                    "at_ms": 1000,
                    "app": null,
                    "kind": "audio",
                    "source": "microphone",
                    "text": "test"
                }
            }),
        }
    }

    fn visual_request(root: &Path, input_id: &str) -> RateRequest {
        let image_path = write_test_ppm(root);
        let image_path = image_path.to_string_lossy().into_owned();
        RateRequest {
            v: 1,
            id: format!("request-{input_id}"),
            session: Some("r20-test-session".to_owned()),
            op: "evaluate".to_owned(),
            params: serde_json::json!({
                "input_id": input_id,
                "image_path": image_path,
                "stream_id": "visual/main",
                "at_ms": 1000,
                "timestamp_s": 1.0,
                "feature_schema": habitua_connectome::RATE_HELPER_OBSERVATION_SCHEMA,
                "features": [],
                "observation": {
                    "frame_id": input_id,
                    "at_ms": 1000,
                    "app": "Editor",
                    "kind": "visual",
                    "target": "window",
                    "image_path": image_path
                }
            }),
        }
    }

    fn feed_request(input_id: &str, event_id: &str) -> RateRequest {
        RateRequest {
            v: 1,
            id: format!("request-{event_id}"),
            session: Some("r20-test-session".to_owned()),
            op: "feed".to_owned(),
            params: serde_json::json!({
                "event_id": event_id,
                "input_id": input_id,
                "sign": "reward",
                "strength": 0.5,
                "event_time_s": 1.0,
                "received_at_s": 2.0,
                "revision": 1,
                "cancelled": false,
                "source": "explicit"
            }),
        }
    }

    fn response_is_retryable(response: &Value) -> bool {
        response["error"]["code"].as_str() == Some("timeout")
    }

    #[test]
    fn expired_observation_replay_is_not_feedable_at_response_time() {
        let root = test_root("expired-replay");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("expired replay test directory");
        let mut runtime = test_runtime();
        runtime.state_path = root.join("state.json");
        let request = visual_request(&root, "expired-replay-input");
        let params: RateObserveParams =
            serde_json::from_value(request.params.clone()).expect("observe params");
        let observation: RateObservationPayload =
            serde_json::from_value(params.observation.clone()).expect("observation");
        let image = read_rate_image(Path::new(&params.image_path)).expect("test image read");
        let image_sha256 = habitua_connectome::image_sha256(&image);
        let digest = observation_identity_digest(&params, &observation, &image_sha256)
            .expect("observation digest");
        runtime.state.observation_ledger.insert(
            params.input_id.clone(),
            RateHelperObservationLedgerEntry {
                input_id: params.input_id,
                digest,
                decision: test_decision(),
                body_status: Some(RateHelperObservationBodyStatus::Expired),
            },
        );

        let response = process_rate_request(&mut runtime, request, 60_000);
        assert_eq!(response["ok"], true);
        assert_eq!(response["result"]["replayed"], true);
        assert_eq!(response["result"]["feedable"], false);
        assert_eq!(
            response["result"]["feed_rejection_code"],
            "observation_expired"
        );
        fs::remove_dir_all(root).expect("remove expired replay test directory");
    }

    #[test]
    fn request_processing_fixes_all_non_retryable_rate_error_codes() {
        let cases = [
            (
                "ledger-capacity",
                "observation_ledger_capacity_exceeded",
                false,
            ),
            ("body-capacity", "observation_body_capacity_exceeded", false),
            ("feedback-capacity", "feedback_capacity_exceeded", false),
            (
                "history-capacity",
                "history_stream_capacity_exceeded",
                false,
            ),
            ("state-size", "state_size_exceeded", false),
            ("expired", "observation_expired", false),
            ("not-feedable", "observation_not_feedable", false),
            ("replay-mismatch", "observation_replay_mismatch", false),
            (
                "replay-unverifiable",
                "observation_replay_unverifiable",
                false,
            ),
            ("persistence-uncertain", "persistence_uncertain", false),
        ];
        for (fixture, expected_code, expected_retryable) in cases {
            let root = test_root(fixture);
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("error code test directory");
            let mut runtime = test_runtime();
            runtime.state_path = root.join("state.json");
            let request = match fixture {
                "ledger-capacity" => {
                    for index in 0..RATE_HELPER_MAX_OBSERVATION_LEDGER_ENTRIES {
                        let input_id = format!("ledger-input-{index}");
                        runtime.state.observation_ledger.insert(
                            input_id.clone(),
                            RateHelperObservationLedgerEntry {
                                input_id,
                                digest: "a".repeat(64),
                                decision: test_decision(),
                                body_status: Some(RateHelperObservationBodyStatus::NotFeedable),
                            },
                        );
                    }
                    audio_request("new-ledger-input")
                }
                "body-capacity" => {
                    for index in 0..RATE_HELPER_MAX_OBSERVATIONS {
                        let input_id = format!("body-input-{index}");
                        runtime.state.observations.insert(
                            input_id.clone(),
                            RateHelperObservation {
                                input_id: input_id.clone(),
                                image_sha256: "image".to_owned(),
                                features: vec![0.0; 12],
                                timestamp_s: index as f64,
                                stream_id: "visual/main".to_owned(),
                                position: index as u64,
                                decision: None,
                            },
                        );
                        runtime.state.feedback.insert(
                            format!("body-event-{index}"),
                            RateHelperFeedback {
                                event_id: format!("body-event-{index}"),
                                input_id,
                                image_sha256: "image".to_owned(),
                                sign: "reward".to_owned(),
                                strength: 0.5,
                                event_time_s: index as f64,
                                received_at_s: index as f64,
                                applied_at_s: index as f64,
                                revision: 1,
                                cancelled: false,
                                source: "test".to_owned(),
                            },
                        );
                    }
                    visual_request(&root, "new-body-input")
                }
                "feedback-capacity" => {
                    runtime.state.observations.insert(
                        "feed-input".to_owned(),
                        RateHelperObservation {
                            input_id: "feed-input".to_owned(),
                            image_sha256: "image".to_owned(),
                            features: vec![0.0; 12],
                            timestamp_s: 1.0,
                            stream_id: "visual/main".to_owned(),
                            position: 0,
                            decision: None,
                        },
                    );
                    for index in 0..RATE_HELPER_MAX_FEEDBACK_ENTRIES {
                        let event_id = format!("existing-event-{index}");
                        runtime.state.feedback.insert(
                            event_id.clone(),
                            RateHelperFeedback {
                                event_id,
                                input_id: "feed-input".to_owned(),
                                image_sha256: "image".to_owned(),
                                sign: "reward".to_owned(),
                                strength: 0.5,
                                event_time_s: 1.0,
                                received_at_s: 1.0,
                                applied_at_s: 1.0,
                                revision: 1,
                                cancelled: false,
                                source: "test".to_owned(),
                            },
                        );
                    }
                    feed_request("feed-input", "new-event")
                }
                "history-capacity" => {
                    for index in 0..RATE_HELPER_MAX_HISTORY_STREAMS {
                        runtime
                            .state
                            .histories
                            .insert(format!("stream-{index}"), Vec::new());
                    }
                    visual_request(&root, "new-history-input")
                }
                "state-size" => {
                    for index in 0..RATE_HELPER_MAX_OBSERVATIONS {
                        let input_id = format!("large-input-{index}");
                        runtime.state.observations.insert(
                            input_id.clone(),
                            RateHelperObservation {
                                input_id,
                                image_sha256: "image".to_owned(),
                                features: vec![0.0; 1_024],
                                timestamp_s: index as f64,
                                stream_id: "stream".to_owned(),
                                position: index as u64,
                                decision: None,
                            },
                        );
                    }
                    audio_request("state-size-input")
                }
                "expired" => {
                    runtime.state.observation_ledger.insert(
                        "expired-input".to_owned(),
                        RateHelperObservationLedgerEntry {
                            input_id: "expired-input".to_owned(),
                            digest: "a".repeat(64),
                            decision: test_decision(),
                            body_status: Some(RateHelperObservationBodyStatus::Expired),
                        },
                    );
                    feed_request("expired-input", "expired-event")
                }
                "not-feedable" => {
                    runtime.state.observation_ledger.insert(
                        "not-feedable-input".to_owned(),
                        RateHelperObservationLedgerEntry {
                            input_id: "not-feedable-input".to_owned(),
                            digest: "a".repeat(64),
                            decision: test_decision(),
                            body_status: Some(RateHelperObservationBodyStatus::NotFeedable),
                        },
                    );
                    feed_request("not-feedable-input", "not-feedable-event")
                }
                "replay-mismatch" => {
                    runtime.state.observation_ledger.insert(
                        "mismatch-input".to_owned(),
                        RateHelperObservationLedgerEntry {
                            input_id: "mismatch-input".to_owned(),
                            digest: "d".repeat(64),
                            decision: test_decision(),
                            body_status: Some(RateHelperObservationBodyStatus::NotFeedable),
                        },
                    );
                    audio_request("mismatch-input")
                }
                "replay-unverifiable" => {
                    runtime.state.observations.insert(
                        "unverifiable-input".to_owned(),
                        RateHelperObservation {
                            input_id: "unverifiable-input".to_owned(),
                            image_sha256: "image".to_owned(),
                            features: vec![0.0; 12],
                            timestamp_s: 1.0,
                            stream_id: "audio/main".to_owned(),
                            position: 0,
                            decision: None,
                        },
                    );
                    audio_request("unverifiable-input")
                }
                "persistence-uncertain" => {
                    runtime.parent_directory_sync = fail_parent_directory_sync;
                    audio_request("persistence-input")
                }
                _ => unreachable!("fixture is listed in the table"),
            };
            let response = process_rate_request(&mut runtime, request, 60_000);
            assert_eq!(response["ok"], false, "fixture={fixture}");
            assert_eq!(
                response["error"]["code"], expected_code,
                "fixture={fixture}: {response}"
            );
            assert_eq!(
                response_is_retryable(&response),
                expected_retryable,
                "fixture={fixture}: {response}"
            );
            fs::remove_dir_all(root).expect("remove error code test directory");
        }
    }

    #[test]
    fn legacy_v2_missing_body_status_is_normalized_to_expired() {
        let root = test_root("legacy-v2");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("legacy state test directory");
        let path = root.join("state.json");
        let mut state = RateHelperPersistedState {
            artifact_sha256: "a".repeat(64),
            ..RateHelperPersistedState::default()
        };
        state.observation_ledger.insert(
            "legacy-input".to_owned(),
            RateHelperObservationLedgerEntry {
                input_id: "legacy-input".to_owned(),
                digest: "b".repeat(64),
                decision: test_decision(),
                body_status: None,
            },
        );
        let mut fixture = serde_json::to_value(&state).expect("legacy state JSON");
        fixture["observation_ledger"]["legacy-input"]
            .as_object_mut()
            .expect("legacy ledger object")
            .remove("body_status");
        fs::write(
            &path,
            serde_json::to_vec(&fixture).expect("legacy state fixture bytes"),
        )
        .expect("legacy state fixture");

        let restored = load_persisted_state(&path).expect("legacy v2 state loads");
        assert_eq!(
            restored.observation_ledger["legacy-input"].body_status,
            Some(RateHelperObservationBodyStatus::Expired)
        );
        fs::remove_dir_all(root).expect("remove legacy state test directory");
    }

    #[test]
    fn shadow_verdict_state_round_trip_preserves_decisions_and_replay() {
        let root = test_root("shadow-state-round-trip");
        fs::create_dir_all(&root).expect("test directory");
        for (action, diagnostic_action, change_bias) in [
            ("notify", "notify_change", 8.0),
            ("silence", "suppress_no_change", -8.0),
        ] {
            let mut runtime = test_runtime();
            runtime.state_path = root.join(format!("{action}.json"));
            runtime.artifact.brains[0].bias = change_bias;
            runtime.artifact.brains[1].bias = -change_bias;
            let mut warmup = visual_request(&root, "warmup");
            warmup.params["shadow"] = json!(true);
            let response = runtime
                .observe_with_deadline(warmup.params, None)
                .expect("warmup");
            assert_eq!(response["readiness"], "insufficient-history");
            let mut request = visual_request(&root, "shadow-input");
            request.params["shadow"] = json!(true);
            let response = runtime
                .observe_with_deadline(request.params.clone(), None)
                .expect("shadow observation");
            assert_eq!(response["readiness"], "ready");
            assert_eq!(response["diagnostic_readiness"], "evaluated");
            assert!(response["novelty"].is_number());
            assert!(response["relevance"].is_number());
            assert_eq!(response["action"], action);
            assert_eq!(response["diagnostic_action"], diagnostic_action);
            assert_eq!(response["external_action"], "hold");
            assert_eq!(response["hold_reason"], "shadow");
            let expected = serde_json::to_value(&runtime.state).expect("state value");
            runtime.state =
                load_persisted_state(&runtime.state_path).expect("restart saved shadow state");
            assert_eq!(serde_json::to_value(&runtime.state).unwrap(), expected);
            let saved_bytes = fs::read(&runtime.state_path).expect("saved state");
            let replay = runtime
                .observe_with_deadline(request.params, None)
                .expect("replay after restart");
            assert_eq!(replay["replayed"], true);
            for field in [
                "action",
                "diagnostic_action",
                "external_action",
                "hold_reason",
                "novelty",
                "relevance",
                "brains",
            ] {
                assert_eq!(replay[field], response[field]);
            }
            assert_eq!(fs::read(&runtime.state_path).unwrap(), saved_bytes);
            // Earlier v2 writers also copied decisions into observation bodies.
            runtime
                .state
                .observations
                .get_mut("shadow-input")
                .unwrap()
                .decision = Some(
                runtime.state.observation_ledger["shadow-input"]
                    .decision
                    .clone(),
            );
            runtime.persist_state().expect("legacy decision fixture");
            let expected = serde_json::to_value(&runtime.state).unwrap();
            let restored = load_persisted_state(&runtime.state_path)
                .expect("legacy shadow decision locations");
            assert_eq!(serde_json::to_value(restored).unwrap(), expected);
        }
        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn invalid_shadow_hold_reason_action_and_readiness_remain_rejected() {
        let root = test_root("invalid-shadow-state");
        fs::create_dir_all(&root).expect("test directory");
        let mut decision = test_decision();
        decision.action = "notify".into();
        decision.diagnostic_action = "notify_change".into();
        decision.hold_reason = Some("shadow".into());
        assert!(validate_state_decision(&decision).is_ok());
        for (field, value) in [
            ("hold_reason", json!("consensus-pending")),
            ("external_action", json!("notify")),
            ("readiness", json!("insufficient-history")),
            ("diagnostic_action", json!("suppress_no_change")),
            ("diagnostic_action", json!("hold")),
            ("action", json!("silence")),
            ("diagnostic_readiness", json!("baseline_insufficient")),
            ("novelty", Value::Null),
            ("relevance", Value::Null),
            ("novelty", json!(1.1)),
            ("relevance", json!(-0.1)),
        ] {
            let mut invalid = serde_json::to_value(&decision).unwrap();
            invalid[field] = json!(value);
            let invalid = serde_json::from_value(invalid).unwrap();
            assert!(validate_state_decision(&invalid).is_err(), "field={field}");
            let mut state = RateHelperPersistedState {
                artifact_sha256: "a".repeat(64),
                ..RateHelperPersistedState::default()
            };
            state.observation_ledger.insert(
                "invalid-input".into(),
                RateHelperObservationLedgerEntry {
                    input_id: "invalid-input".into(),
                    digest: "b".repeat(64),
                    decision: invalid,
                    body_status: Some(RateHelperObservationBodyStatus::NotFeedable),
                },
            );
            let path = root.join(format!("{field}.json"));
            fs::write(&path, serde_json::to_vec(&state).unwrap()).expect("invalid state fixture");
            assert!(load_persisted_state(&path).is_err(), "field={field}");
            if value.is_null() {
                let mut omitted = serde_json::to_value(&state).unwrap();
                omitted["observation_ledger"]["invalid-input"]["decision"]
                    .as_object_mut()
                    .unwrap()
                    .remove(field);
                fs::write(&path, serde_json::to_vec(&omitted).unwrap())
                    .expect("omitted score state");
                assert!(
                    load_persisted_state(&path).is_err(),
                    "omitted field={field}"
                );
            }
        }
        let mut missing_reason = test_decision();
        missing_reason.hold_reason = None;
        assert!(validate_state_decision(&missing_reason).is_err());
        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn observation_digest_covers_all_early_return_and_consensus_fields() {
        let base = serde_json::json!({
            "input_id": "input",
            "image_path": "frame.ppm",
            "stream_id": "stream",
            "at_ms": 1000,
            "timestamp_s": 1.0,
            "feature_schema": habitua_connectome::RATE_HELPER_OBSERVATION_SCHEMA,
            "features": [0.25, 0.5],
            "image_sha256": "provided",
            "observation": {
                "frame_id": "input",
                "at_ms": 1000,
                "app": "Editor",
                "kind": "visual",
                "target": "fullscreen",
                "image_path": "frame.ppm"
            },
            "context": {"frames": [], "audio": []},
            "position": 1,
            "shadow": true,
            "repeat_index": 1,
            "throttle_every": 2
        });
        let digest = |value: &Value| {
            let params: RateObserveParams =
                serde_json::from_value(value.clone()).expect("observe params");
            let observation: RateObservationPayload =
                serde_json::from_value(params.observation.clone()).expect("observation");
            observation_identity_digest(&params, &observation, "image").expect("identity digest")
        };
        let base_digest = digest(&base);
        let mut variants = Vec::new();
        for (field, value) in [
            ("input_id", serde_json::json!("other-input")),
            ("image_path", serde_json::json!("other-frame.ppm")),
            ("stream_id", serde_json::json!("other-stream")),
            ("at_ms", serde_json::json!(2000)),
            ("timestamp_s", serde_json::json!(2.0)),
            (
                "feature_schema",
                serde_json::json!("coosenpai-observation-v1-alt"),
            ),
            ("features", serde_json::json!([0.75, 0.5])),
            ("image_sha256", serde_json::json!("other-image")),
            ("position", serde_json::json!(2)),
            ("shadow", serde_json::json!(false)),
            ("repeat_index", serde_json::json!(3)),
            ("throttle_every", serde_json::json!(4)),
        ] {
            let mut variant = base.clone();
            variant[field] = value;
            variants.push(variant);
        }
        let mut changed_context = base.clone();
        changed_context["context"]["frames"] = serde_json::json!([{"id": "other"}]);
        variants.push(changed_context);
        let mut changed_kind = base.clone();
        changed_kind["observation"]["kind"] = serde_json::json!("audio");
        variants.push(changed_kind);
        let mut changed_app = base.clone();
        changed_app["observation"]["app"] = Value::Null;
        variants.push(changed_app);
        for (field, value) in [
            ("target", serde_json::json!("window")),
            ("image_path", serde_json::json!("other-frame.ppm")),
            ("ocr_text", serde_json::json!("changed")),
            ("source", serde_json::json!("changed-source")),
            ("text", serde_json::json!("changed-text")),
        ] {
            let mut changed_observation = base.clone();
            changed_observation["observation"][field] = value;
            variants.push(changed_observation);
        }
        let resolved_image_variant = {
            let params: RateObserveParams =
                serde_json::from_value(base.clone()).expect("observe params");
            let observation: RateObservationPayload =
                serde_json::from_value(params.observation.clone()).expect("observation");
            observation_identity_digest(&params, &observation, "other-image")
                .expect("different resolved image digest")
        };
        assert!(
            variants
                .iter()
                .all(|variant| digest(variant) != base_digest)
        );
        assert_ne!(resolved_image_variant, base_digest);
    }

    #[test]
    fn early_return_observation_is_explicitly_not_feedable() {
        let root = PathBuf::from(format!(
            "target/connectome-helper-early-feed-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("early feed test directory");
        let mut runtime = test_runtime();
        runtime.state_path = root.join("state.json");
        let params = RateObserveParams {
            input_id: "audio-input".to_owned(),
            image_path: String::new(),
            stream_id: "audio/main".to_owned(),
            at_ms: 1_000,
            timestamp_s: 1.0,
            feature_schema: habitua_connectome::RATE_HELPER_OBSERVATION_SCHEMA.to_owned(),
            features: Vec::new(),
            observation: serde_json::json!({}),
            context: None,
            image_sha256: None,
            position: None,
            shadow: false,
            repeat_index: 0,
            throttle_every: None,
        };
        let observation = RateObservationPayload {
            frame_id: "audio-input".to_owned(),
            at_ms: 1_000,
            app: None,
            kind: "audio".to_owned(),
            target: None,
            image_path: None,
            ocr_text: None,
            source: Some("microphone".to_owned()),
            text: Some("test".to_owned()),
        };
        let decision = RateHelperObservationDecision {
            readiness: "unsupported-input".to_owned(),
            diagnostic_readiness: "audio_not_supported_by_retina_rate_model".to_owned(),
            novelty: None,
            relevance: None,
            action: "hold".to_owned(),
            diagnostic_action: "hold".to_owned(),
            external_action: "hold".to_owned(),
            hold_reason: Some("unsupported-input".to_owned()),
            brains: Vec::new(),
        };
        let digest = "b".repeat(64);
        let response = runtime
            .persist_early_observation(
                &params,
                &observation,
                &decision,
                &digest,
                None,
                false,
                false,
                None,
            )
            .expect("early observation");
        assert_eq!(response["feedable"], false);
        assert_eq!(response["feed_rejection_code"], "observation_not_feedable");
        assert_eq!(
            runtime.state.observation_ledger["audio-input"].body_status,
            Some(RateHelperObservationBodyStatus::NotFeedable)
        );
        let error = runtime
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "event-a",
                    "input_id": "audio-input",
                    "sign": "reward",
                    "strength": 0.5,
                    "event_time_s": 1.0,
                    "received_at_s": 2.0,
                    "revision": 1,
                    "cancelled": true,
                    "source": "explicit"
                }),
                None,
            )
            .expect_err("early observations must reject feedback");
        assert_eq!(rate_error_code(&error), "observation_not_feedable");
        fs::remove_dir_all(root).expect("remove early feed test directory");
    }

    #[test]
    fn evicted_observation_feed_is_explicitly_expired_after_restart() {
        let root = PathBuf::from(format!(
            "target/connectome-helper-expired-feed-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("expired feed test directory");
        let mut runtime = test_runtime();
        runtime.state_path = root.join("state.json");
        let digest = "c".repeat(64);
        for index in 0..RATE_HELPER_MAX_OBSERVATIONS {
            let input_id = format!("input-{index}");
            runtime.state.observations.insert(
                input_id.clone(),
                RateHelperObservation {
                    input_id: input_id.clone(),
                    image_sha256: "image".to_owned(),
                    features: vec![0.0; 12],
                    timestamp_s: index as f64,
                    stream_id: "stream".to_owned(),
                    position: index as u64,
                    decision: None,
                },
            );
            runtime.state.observation_ledger.insert(
                input_id.clone(),
                RateHelperObservationLedgerEntry {
                    input_id,
                    digest: digest.clone(),
                    decision: test_decision(),
                    body_status: Some(RateHelperObservationBodyStatus::Present),
                },
            );
        }
        runtime
            .ensure_observation_capacity_for_insert()
            .expect("old unreferenced body is evicted");
        assert!(!runtime.state.observations.contains_key("input-0"));
        assert_eq!(
            runtime.state.observation_ledger["input-0"].body_status,
            Some(RateHelperObservationBodyStatus::Expired)
        );
        runtime.persist_state().expect("persist expired tombstone");
        let restored = load_persisted_state(&runtime.state_path).expect("load expired tombstone");
        assert_eq!(
            restored.observation_ledger["input-0"].body_status,
            Some(RateHelperObservationBodyStatus::Expired)
        );
        let mut restarted = test_runtime();
        restarted.state_path = runtime.state_path.clone();
        restarted.state = restored;
        let error = restarted
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "late-event",
                    "input_id": "input-0",
                    "sign": "reward",
                    "strength": 0.5,
                    "event_time_s": 0.0,
                    "received_at_s": 10.0,
                    "revision": 1,
                    "cancelled": false,
                    "source": "automatic"
                }),
                None,
            )
            .expect_err("evicted observation must reject delayed feedback");
        assert_eq!(rate_error_code(&error), "observation_expired");
        fs::remove_dir_all(root).expect("remove expired feed test directory");
    }

    #[test]
    fn persisted_state_cross_references_and_contract_values_are_validated() {
        let root = PathBuf::from(format!(
            "target/connectome-helper-state-validation-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("state validation test directory");
        let write_and_reject = |name: &str, state: RateHelperPersistedState| {
            let path = root.join(format!("{name}.json"));
            fs::write(&path, serde_json::to_vec(&state).expect("state JSON"))
                .expect("state fixture");
            load_persisted_state(&path).expect_err("invalid state must be rejected")
        };
        let mut key_mismatch = RateHelperPersistedState {
            artifact_sha256: "a".repeat(64),
            ..RateHelperPersistedState::default()
        };
        key_mismatch.observation_ledger.insert(
            "ledger-key".to_owned(),
            RateHelperObservationLedgerEntry {
                input_id: "other-input".to_owned(),
                digest: "d".repeat(64),
                decision: test_decision(),
                body_status: Some(RateHelperObservationBodyStatus::NotFeedable),
            },
        );
        assert!(
            write_and_reject("key", key_mismatch)
                .to_string()
                .contains("does not match")
        );

        let mut bad_digest = RateHelperPersistedState {
            artifact_sha256: "a".repeat(64),
            ..RateHelperPersistedState::default()
        };
        bad_digest.observation_ledger.insert(
            "input".to_owned(),
            RateHelperObservationLedgerEntry {
                input_id: "input".to_owned(),
                digest: "not-a-digest".to_owned(),
                decision: test_decision(),
                body_status: Some(RateHelperObservationBodyStatus::NotFeedable),
            },
        );
        assert!(
            write_and_reject("digest", bad_digest)
                .to_string()
                .contains("canonical digest")
        );

        let mut bad_decision = RateHelperPersistedState {
            artifact_sha256: "a".repeat(64),
            ..RateHelperPersistedState::default()
        };
        let mut decision = test_decision();
        decision.action = "invalid".to_owned();
        bad_decision.observation_ledger.insert(
            "input".to_owned(),
            RateHelperObservationLedgerEntry {
                input_id: "input".to_owned(),
                digest: "e".repeat(64),
                decision,
                body_status: Some(RateHelperObservationBodyStatus::NotFeedable),
            },
        );
        assert!(
            write_and_reject("decision", bad_decision)
                .to_string()
                .contains("unsupported action")
        );

        let mut body_mismatch = RateHelperPersistedState {
            artifact_sha256: "a".repeat(64),
            ..RateHelperPersistedState::default()
        };
        body_mismatch.observation_ledger.insert(
            "input".to_owned(),
            RateHelperObservationLedgerEntry {
                input_id: "input".to_owned(),
                digest: "f".repeat(64),
                decision: test_decision(),
                body_status: Some(RateHelperObservationBodyStatus::Present),
            },
        );
        assert!(
            write_and_reject("body", body_mismatch)
                .to_string()
                .contains("body is present but it is missing")
        );

        let mut feedback_mismatch = RateHelperPersistedState {
            artifact_sha256: "a".repeat(64),
            ..RateHelperPersistedState::default()
        };
        feedback_mismatch.observations.insert(
            "input".to_owned(),
            RateHelperObservation {
                input_id: "input".to_owned(),
                image_sha256: "image".to_owned(),
                features: vec![0.0],
                timestamp_s: 1.0,
                stream_id: "stream".to_owned(),
                position: 0,
                decision: None,
            },
        );
        feedback_mismatch.observation_ledger.insert(
            "input".to_owned(),
            RateHelperObservationLedgerEntry {
                input_id: "input".to_owned(),
                digest: "1".repeat(64),
                decision: test_decision(),
                body_status: Some(RateHelperObservationBodyStatus::Present),
            },
        );
        feedback_mismatch.feedback.insert(
            "event".to_owned(),
            RateHelperFeedback {
                event_id: "event".to_owned(),
                input_id: "missing".to_owned(),
                image_sha256: "image".to_owned(),
                sign: "reward".to_owned(),
                strength: 0.5,
                event_time_s: 1.0,
                received_at_s: 1.0,
                applied_at_s: 1.0,
                revision: 1,
                cancelled: false,
                source: "automatic".to_owned(),
            },
        );
        assert!(
            write_and_reject("feedback", feedback_mismatch)
                .to_string()
                .contains("no observation body")
        );
        fs::remove_dir_all(root).expect("remove state validation test directory");
    }

    #[test]
    fn persisted_string_and_total_state_limits_are_rejected() {
        let root = PathBuf::from(format!(
            "target/connectome-helper-state-limit-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("state limit test directory");
        let write_and_reject = |name: &str, state: RateHelperPersistedState| {
            let path = root.join(format!("{name}.json"));
            fs::write(&path, serde_json::to_vec(&state).expect("state JSON"))
                .expect("state fixture");
            load_persisted_state(&path).expect_err("invalid state must be rejected")
        };
        let mut state = RateHelperPersistedState {
            artifact_sha256: "a".repeat(64),
            ..RateHelperPersistedState::default()
        };
        let long_id = "i".repeat(RATE_HELPER_MAX_IDENTIFIER_BYTES + 1);
        state.observation_ledger.insert(
            long_id.clone(),
            RateHelperObservationLedgerEntry {
                input_id: long_id,
                digest: "a".repeat(64),
                decision: test_decision(),
                body_status: Some(RateHelperObservationBodyStatus::NotFeedable),
            },
        );
        let path = root.join("identifier.json");
        fs::write(
            &path,
            serde_json::to_vec(&state).expect("identifier state JSON"),
        )
        .expect("identifier state");
        let error = load_persisted_state(&path).expect_err("long identifier must be rejected");
        assert!(error.to_string().contains("byte limit"));

        let mut long_reason_state = RateHelperPersistedState {
            artifact_sha256: "a".repeat(64),
            ..RateHelperPersistedState::default()
        };
        let mut long_reason_decision = test_decision();
        long_reason_decision.hold_reason = Some("r".repeat(RATE_HELPER_MAX_REASON_BYTES + 1));
        long_reason_state.observation_ledger.insert(
            "reason-input".to_owned(),
            RateHelperObservationLedgerEntry {
                input_id: "reason-input".to_owned(),
                digest: "b".repeat(64),
                decision: long_reason_decision,
                body_status: Some(RateHelperObservationBodyStatus::NotFeedable),
            },
        );
        let error = write_and_reject("reason", long_reason_state);
        assert!(error.to_string().contains("hold_reason"));

        let mut long_feedback_state = RateHelperPersistedState {
            artifact_sha256: "a".repeat(64),
            ..RateHelperPersistedState::default()
        };
        long_feedback_state.observations.insert(
            "feedback-input".to_owned(),
            RateHelperObservation {
                input_id: "feedback-input".to_owned(),
                image_sha256: "image".to_owned(),
                features: Vec::new(),
                timestamp_s: 1.0,
                stream_id: "stream".to_owned(),
                position: 0,
                decision: None,
            },
        );
        long_feedback_state.feedback.insert(
            "feedback-event".to_owned(),
            RateHelperFeedback {
                event_id: "feedback-event".to_owned(),
                input_id: "feedback-input".to_owned(),
                image_sha256: "image".to_owned(),
                sign: "s".repeat(RATE_HELPER_MAX_REASON_BYTES + 1),
                strength: 0.5,
                event_time_s: 1.0,
                received_at_s: 1.0,
                applied_at_s: 1.0,
                revision: 1,
                cancelled: false,
                source: "test".to_owned(),
            },
        );
        let error = write_and_reject("feedback-sign", long_feedback_state);
        assert!(error.to_string().contains("feedback sign"));

        let oversized_path = root.join("oversized.json");
        fs::write(&oversized_path, vec![b' '; RATE_HELPER_MAX_STATE_BYTES + 1])
            .expect("oversized state fixture");
        let error =
            load_persisted_state(&oversized_path).expect_err("oversized state must be rejected");
        assert!(error.to_string().contains("persisted state exceeds"));
        fs::remove_dir_all(root).expect("remove state limit test directory");
    }

    #[test]
    fn parent_sync_failure_flushes_one_error_and_stops_before_the_next_request() {
        let root = PathBuf::from(format!(
            "target/connectome-helper-parent-sync-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("parent sync test directory");
        let mut runtime = test_runtime();
        runtime.state_path = root.join("state.json");
        runtime.parent_directory_sync = fail_parent_directory_sync;
        let input = format!(
            "{}\n{}\n",
            serde_json::to_string(&serde_json::json!({
                "v": 1,
                "id": "audio-request",
                "session": "test-session",
                "op": "evaluate",
                "params": {
                    "input_id": "audio-input",
                    "image_path": "",
                    "stream_id": "audio/main",
                    "at_ms": 1000,
                    "timestamp_s": 1.0,
                    "feature_schema": habitua_connectome::RATE_HELPER_OBSERVATION_SCHEMA,
                    "features": [],
                    "observation": {
                        "frame_id": "audio-input",
                        "at_ms": 1000,
                        "app": null,
                        "kind": "audio",
                        "source": "microphone",
                        "text": "test"
                    }
                }
            }))
            .expect("audio request JSON"),
            serde_json::to_string(&serde_json::json!({
                "v": 1,
                "id": "must-not-run",
                "session": "test-session",
                "op": "health",
                "params": {}
            }))
            .expect("health request JSON")
        );
        let mut output = Vec::new();
        let error = run_rate_helper_loop(
            &mut runtime,
            std::io::Cursor::new(input),
            &mut output,
            5_000,
        )
        .expect_err("parent sync uncertainty must stop the resident loop");
        assert!(error.to_string().contains("durability-uncertain"));
        let responses = String::from_utf8(output)
            .expect("helper output UTF-8")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("response JSON"))
            .collect::<Vec<_>>();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["error"]["code"], "persistence_uncertain");
        assert!(
            !responses
                .iter()
                .any(|response| response["id"] == "must-not-run")
        );
        let restored = load_persisted_state(&runtime.state_path).expect("committed state loads");
        validate_state_artifact_binding(&restored, &runtime.artifact_sha256)
            .expect("restarted state remains bound to the artifact");
        assert_eq!(restored.observation_ledger.len(), 1);
        assert!(restored.observation_ledger.contains_key("audio-input"));
        fs::remove_dir_all(root).expect("remove parent sync test directory");
    }

    #[test]
    fn persisted_retention_limits_are_checked_on_load() {
        let root = PathBuf::from(format!(
            "target/connectome-helper-retention-load-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("retention test directory");
        type StatePopulate = fn(&mut RateHelperPersistedState);
        let cases: [(&str, StatePopulate); 6] = [
            ("observations", |state| {
                for index in 0..=RATE_HELPER_MAX_OBSERVATIONS {
                    state.observations.insert(
                        format!("observation-{index}"),
                        RateHelperObservation {
                            input_id: format!("observation-{index}"),
                            image_sha256: "image".to_owned(),
                            features: Vec::new(),
                            timestamp_s: index as f64,
                            stream_id: "stream".to_owned(),
                            position: index as u64,
                            decision: None,
                        },
                    );
                }
            }),
            ("ledger", |state| {
                for index in 0..=RATE_HELPER_MAX_OBSERVATION_LEDGER_ENTRIES {
                    state.observation_ledger.insert(
                        format!("input-{index}"),
                        RateHelperObservationLedgerEntry {
                            input_id: format!("input-{index}"),
                            digest: format!("digest-{index}"),
                            decision: test_decision(),
                            body_status: None,
                        },
                    );
                }
            }),
            ("feedback", |state| {
                for index in 0..=RATE_HELPER_MAX_FEEDBACK_ENTRIES {
                    state.feedback.insert(
                        format!("event-{index}"),
                        RateHelperFeedback {
                            event_id: format!("event-{index}"),
                            input_id: "input".to_owned(),
                            image_sha256: "image".to_owned(),
                            sign: "reward".to_owned(),
                            strength: 1.0,
                            event_time_s: 0.0,
                            received_at_s: 0.0,
                            applied_at_s: 0.0,
                            revision: index as u64,
                            cancelled: false,
                            source: "test".to_owned(),
                        },
                    );
                }
            }),
            ("history", |state| {
                state.histories.insert(
                    "stream".to_owned(),
                    (0..=RATE_HELPER_MAX_HISTORY_ENTRIES_PER_STREAM)
                        .map(|index| FrozenReadoutHistoryEntry {
                            features: vec![index as f32],
                            timestamp_s: index as f64,
                        })
                        .collect(),
                );
            }),
            ("history-streams", |state| {
                for index in 0..=RATE_HELPER_MAX_HISTORY_STREAMS {
                    state
                        .histories
                        .insert(format!("stream-{index}"), Vec::new());
                }
            }),
            ("feedback-audit", |state| {
                state.feedback_history = (0..=RATE_HELPER_MAX_AUDIT_ENTRIES)
                    .map(|index| RateHelperFeedbackAudit {
                        event_id: format!("audit-{index}"),
                        input_id: "input".to_owned(),
                        sign: "reward".to_owned(),
                        strength: 0.5,
                        event_time_s: index as f64,
                        revision: index as u64,
                        cancelled: false,
                        source: "test".to_owned(),
                        received_at_s: index as f64,
                        applied_at_s: index as f64,
                        applied: true,
                        reason: "overflow".to_owned(),
                    })
                    .collect();
            }),
        ];
        for (name, populate) in cases {
            let mut state = RateHelperPersistedState {
                artifact_sha256: "a".repeat(64),
                ..RateHelperPersistedState::default()
            };
            populate(&mut state);
            let path = root.join(format!("{name}.json"));
            fs::write(&path, serde_json::to_vec(&state).expect("state JSON")).expect("state write");
            let error = load_persisted_state(&path).expect_err("retention overflow rejected");
            assert!(error.to_string().contains("capacity"), "{name}: {error}");
        }
        fs::remove_dir_all(root).expect("remove retention test directory");
    }

    #[test]
    fn observation_ledger_rejects_new_inputs_at_capacity_but_allows_delayed_feed() {
        let root = PathBuf::from(format!(
            "target/connectome-helper-ledger-capacity-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("ledger test directory");
        let mut runtime = test_runtime();
        runtime.state_path = root.join("state.json");
        for index in 0..RATE_HELPER_MAX_OBSERVATION_LEDGER_ENTRIES {
            let input_id = format!("input-{index}");
            runtime.state.observation_ledger.insert(
                input_id.clone(),
                RateHelperObservationLedgerEntry {
                    input_id,
                    digest: format!("digest-{index}"),
                    decision: test_decision(),
                    body_status: None,
                },
            );
        }
        runtime.state.observations.insert(
            "input-0".to_owned(),
            RateHelperObservation {
                input_id: "input-0".to_owned(),
                image_sha256: "image".to_owned(),
                features: vec![0.0; 12],
                timestamp_s: 1.0,
                stream_id: "stream".to_owned(),
                position: 0,
                decision: None,
            },
        );
        let error = runtime
            .ensure_observation_ledger_capacity("new-input")
            .expect_err("new input must be rejected at ledger capacity");
        assert!(error.contains("capacity exceeded"));
        runtime
            .ensure_observation_ledger_capacity("input-0")
            .expect("replay of a ledger entry remains accepted");
        let feed = runtime
            .feed_with_deadline(
                serde_json::json!({
                    "event_id": "delayed-event",
                    "input_id": "input-0",
                    "sign": "reward",
                    "strength": 0.5,
                    "event_time_s": 1.0,
                    "received_at_s": 2.0,
                    "revision": 1,
                    "cancelled": false,
                    "source": "explicit"
                }),
                None,
            )
            .expect("delayed feed for a retained observation");
        assert_eq!(feed["applied"], true);
        fs::remove_dir_all(root).expect("remove ledger test directory");
    }

    #[test]
    fn history_stream_capacity_rejects_unbounded_stream_ids() {
        let mut runtime = test_runtime();
        for index in 0..RATE_HELPER_MAX_HISTORY_STREAMS {
            runtime
                .state
                .histories
                .insert(format!("stream-{index}"), Vec::new());
        }
        runtime
            .ensure_history_stream_capacity("stream-0")
            .expect("existing stream remains usable");
        let error = runtime
            .ensure_history_stream_capacity("new-stream")
            .expect_err("new stream must be rejected at capacity");
        assert!(error.contains("history stream capacity exceeded"));
    }

    #[test]
    fn observation_body_rejects_insertion_when_all_retained_observations_have_feedback() {
        let mut runtime = test_runtime();
        for index in 0..RATE_HELPER_MAX_OBSERVATIONS {
            let input_id = format!("input-{index}");
            runtime.state.observations.insert(
                input_id.clone(),
                RateHelperObservation {
                    input_id: input_id.clone(),
                    image_sha256: format!("image-{index}"),
                    features: Vec::new(),
                    timestamp_s: index as f64,
                    stream_id: "stream".to_owned(),
                    position: index as u64,
                    decision: None,
                },
            );
            runtime.state.feedback.insert(
                format!("event-{index}"),
                RateHelperFeedback {
                    event_id: format!("event-{index}"),
                    input_id,
                    image_sha256: format!("image-{index}"),
                    sign: "reward".to_owned(),
                    strength: 1.0,
                    event_time_s: 0.0,
                    received_at_s: 0.0,
                    applied_at_s: 0.0,
                    revision: 1,
                    cancelled: false,
                    source: "test".to_owned(),
                },
            );
        }
        let error = runtime
            .ensure_observation_capacity_for_insert()
            .expect_err("referenced observations must not be silently discarded");
        assert!(error.contains("every retained observation has feedback"));
    }

    fn test_decision() -> RateHelperObservationDecision {
        RateHelperObservationDecision {
            readiness: "ready".to_owned(),
            diagnostic_readiness: "evaluated".to_owned(),
            novelty: Some(0.5),
            relevance: Some(0.5),
            action: "hold".to_owned(),
            diagnostic_action: "hold".to_owned(),
            external_action: "hold".to_owned(),
            hold_reason: Some("test".to_owned()),
            brains: Vec::new(),
        }
    }
}
