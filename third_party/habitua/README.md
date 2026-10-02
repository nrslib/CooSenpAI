# habitua

[日本語版](README.ja.md)

Repository: <https://github.com/nrslib/habitua>

`habitua` shortens *habituation*: the simplest form of learning, in which a
response weakens when the same stimulus is repeated.

`habitua` is a Rust crate for habituation over arbitrary feature vectors. It
separates evaluation from learning: an application can inspect an evaluation,
hold it, reduce its weight, or commit it as one experience.

The core does not perform image recognition, OCR, audio recognition, or text
embedding. An adapter defines the meaning, normalization, and version of each
feature schema and supplies finalized vectors.

## Quick start

The default `Habitua` alias is the core `FlyModel`. The connectome brain is a
separately distributed `habitua-connectome` plugin that uses a pack supplied by
the user in a separate process; core model construction and evaluation never
download data.

```rust
use habitua::{Config, Habitua, Model, Timestamp};

let config = Config::default();
let features = vec![0.0_f32; config.dimensions];
let mut model = Habitua::with_config(config)?;
let evaluation = model.evaluate("screen", &features, Timestamp::ZERO)?;
model.commit(evaluation.update, 1.0)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Timestamp` is supplied by the caller. The crate never reads `SystemTime` for
model time, so tests and replay can be deterministic. Each stream has
independent state. Invalid input returns `Err` and never becomes a panic.

## Model boundary

The typed replaceable boundary is:

```text
trait Model {
    evaluate(stream_id, features, at) -> Result<Evaluation, ModelError>
    evaluate_input(stream_id, input, at) -> Result<Evaluation, ModelError>
    commit(update, learning_weight) -> Result<(), ModelError>
}
```

`ModelInput::Features` is the ordinary feature-vector input. A model that
consumes structured operation results can implement `Model::evaluate_input`;
the built-in `FailureModel` accepts `ModelInput::Failure` containing
`FailureObservation { success, result_code, error_type, state }`. It reports a
`Failure` reaction for an unsuccessful result and has no learning state, so
repeated failures are not made familiar by this model.

`Evaluation` contains optional common values: short- and long-term novelty and
familiarity are `Option<f32>`, as are exact-pattern `recency` and `pattern_id`.
A model supplies only concepts it actually defines, plus model-specific
diagnostics and an uncommitted update. A weight is finite and in `0..=1`.
Weight zero is a defined observation-bookkeeping operation: it records the
arrival metadata carried by the update, such as the latest timestamp and
exact-pattern recency, without reinforcing learned state. A fractional weight
reduces learned state proportionally. A model with no arrival metadata has no
learned-state change at weight zero.

`Habituation<E, M>` is the generic Fly-style implementation. Its per-stream
factory is held internally as
`Box<dyn Fn() -> Result<M, MemoryError> + Send + Sync>`, so the public type has
only the `Encoder` and `Memory` type parameters. The first factory result must
be empty and establishes the memory fingerprint. Every later result is checked
for the same fingerprint and emptiness. `FlyModel` and `Habitua` are aliases
for `Habituation<FlyHashEncoder, DecayingMemory>` in older integrations;
the default alias `Habitua` is `FlyModel`; the connectome plugin is not
registered in the core registry and crosses the boundary through a separate
process.

`Encoder` and `Memory` remain separate inside `Habituation`; either can be
replaced independently. `NumericDeviationModel` and
`NearestNeighborModel` implement the same `Model` and persistence contracts
without using those two components.

## Multiple brains

`BrainRunner` lets several different models evaluate the same finalized input.
Each configured brain has its own ID and role, active or shadow mode, input
schema and target, model kind/version/settings/seed, learning policy, reaction
thresholds, resource limits, and state identity. A `ModelRegistry` creates the
typed models and places them behind a thin type-erased wrapper only at the
runner boundary.

The processing order is fixed:

1. The adapter finalizes an input and its availability time.
2. Every matching enabled brain evaluates the unchanged pre-update state.
3. The runner records each report and readiness state.
4. The caller uses only active reactions for its activation policy; shadow
   reactions are available for comparison.
5. The caller commits each brain with its own policy or explicit weight.

Schema validation is performed only after target and context selection. One
experiment can therefore contain brains for different schemas and dimensions;
a brain for another target is recorded as `InputMissing` rather than making
the whole evaluation fail. Each stream has its own update generation, so
pending updates from independent streams can be committed independently.
`promote_shadow` copies state only when the input contract, model settings,
state format, learning history, and state identity are compatible.

Shadow means “does not participate in activation”; it still learns under the
same policy. Learning can be stopped with weight zero while evaluation count,
arrival time, processing position, and exact-event metadata continue to be
recorded.

`BrainLearningPolicy::default()` permits a model to learn while its baseline is
still insufficient, because Numeric and nearest-neighbor models need initial
samples or representatives before they can evaluate a reaction. Set
`require_evaluated` to `true` when a caller must withhold learning until a
model reports `Evaluated`. `RepeatedNormal` keeps its streak and bootstrap
state per stream, including for Numeric and nearest-neighbor baseline
construction; it does not mix observations from different streams. For Fly
and nearest-neighbor models, novelty remains a diagnostic reaction while an
evaluated input can still be counted as a normal learning experience; Numeric
and Failure models mark an evaluated deviation or failure as abnormal.

The common report is `Reaction`:

```text
Reaction {
    brain, target,
    kind: Novel | Deviation | Absence | Repeat | Failure,
    strength: Option<f32>,
    metrics: name, value, unit, direction,
    evidence,
    readiness: Evaluated | BaselineInsufficient | InputMissing |
               Invalid | Expired | Error,
}
```

`strength` is `None` for the uncalibrated built-in raw signals. A reaction is
not an anomaly probability. Evidence and readiness must be considered before
an application combines reactions.

To add another biological-circuit model to the registry:

1. Define its target, feature conditions, time semantics, learning rule, and
   output meaning.
2. Implement `Model`, `PersistentModel`, configuration validation, and a
   report converter, then register the typed factory.
3. Add a shadow brain using the same input schema and a distinct state ID.
4. Run `examples/replay.rs` and compare newly detected events, false
   notifications, delay, and resource cost.

Unknown model names, schema mismatches, dimension mismatches, and incompatible
saved state are errors. The registry never silently substitutes a default.

## Connectome plugin (separate distribution and process)

`habitua-connectome` runs a CPU firing-rate model on the MaleCNS v1.0 connectome
from Janelia. The dataset is CC BY 4.0. Source: <https://male-cns.janelia.org/>;
Berg et al., *Cell* (2026), doi:10.1016/j.cell.2026.08.015.

Build the MaleCNS rate pack from separately obtained data using
`scripts/malecns_feather_to_parquet.py` and `connectome-data convert-malecns-rate`.
The pack and raw connectome data are not bundled with habitua. The code licenses
(MIT or Apache-2.0) and the dataset license apply separately. A 1,024-position
soma sample for activity display is included with CC BY 4.0 attribution.
See [`docs/connectome-activity-view.md`](docs/connectome-activity-view.md)
for the activity display and data attribution. The original data file checksums
are in [`docs/malecns-v1.0-SHA256SUMS.txt`](docs/malecns-v1.0-SHA256SUMS.txt).

`connectome-helper` is a separate judge plugin for CooSenpAI. Provide the
trained artifact and a writable state path with `--artifact` and `--state`, or
with `HABITUA_RATE_ARTIFACT` and `HABITUA_RATE_STATE`. The distributable r14 artifact is
`dist/connectome/malecns-frozen-response-learning-artifact-r14.json`. The
rate-full pack is distributed separately; provide
its absolute directory path with `--pack` or `HABITUA_RATE_PACK`. The command
line option takes precedence, followed by the environment variable and then
the artifact's `pack_path` (relative to the artifact directory when needed).
Run `python3 tools/prepare-connectome-distribution.py artifact <source-json> dist/connectome`
to recreate the artifact and its SHA-256 record, or
`python3 tools/prepare-connectome-distribution.py pack <rate-full-dir> <distribution-artifact> <output-dir>`
to create a tar and attribution/checksum manifest. Start the helper with
`connectome-helper --artifact <distribution-artifact> --state <writable-state-path> --pack <absolute-rate-full-dir>`.
The protocol
is documented in [`docs/connectome-plugin-protocol-v1.md`](docs/connectome-plugin-protocol-v1.md).

## Fly model

`FlyHashEncoder` creates a deterministic random sparse projection from its
seed and keeps only the top `k` units. It is an engineering model inspired by
sparse coding in the fly olfactory system, not a biological simulation.
`PatternId` is a stable fingerprint, not a collision-free identifier.

`DecayingMemory` stores short- and long-term weights for active units. For a
unit with weight `w(0)` last updated at `t0`, its value at `t` is:

```text
w(t) = w(0) exp(-(t - t0) / tau)
```

Before the current pattern is reinforced, its familiarity is calculated as:

```text
f_short = mean(short_unit_weight(t))
f_long  = mean(long_unit_weight(t))
familiarity = (alpha * f_short + beta * f_long) / (alpha + beta)
novelty = 1 - familiarity
```

For learning weight `q`, the short-term update is:

```text
short_weight = min(1, short_weight(t) + short_reinforcement * q)
```

When the configured long-term policy permits reinforcement, the long-term
update is:

```text
long_weight = min(1, long_weight(t) + long_reinforcement * q)
```

Otherwise the decayed long-term weight is left unchanged. Both familiarity
values are calculated from the pre-update unit weights; only then are the
unit weights and the exact-pattern recency record updated.

The default long-term policy is `LongLearningPolicy::Revisit`: the long-term
update is made only for an exact-pattern revisit after the configured interval.
`LongLearningPolicy::RepeatedNormal` is a separate policy for a caller that
has established consecutive normal experiences. New configurations cannot use
the compatibility-only legacy every-commit mode. Thus a one-second failure
burst does not become long-term normality merely because it repeats.

Familiarity uses sparse-unit overlap; exact-pattern `recency` uses the full
sorted unit vector. Exact records have an LRU bound from `Config::max_patterns`
and use `(last_seen, observation_sequence, pattern)` to break timestamp ties.
Unit indices are checked against `Config::encoder_units`, so a custom encoder
cannot create an unbounded unit map inside this memory.

The Fly diagnostics expose normalized input strength and deviation from the
running feature profile. Profile deviation means distance from the feature
baseline; it is not a check of the interval between observations.

## Numeric deviation model

`NumericDeviationModel` accepts named values such as `latency_ms` and
`retry_count`. It keeps a bounded, time-windowed sample set per stream. The
current value is evaluated before it is learned. For each name:

```text
z+ = max(0, (x - median) / max(1.4826 * MAD, mad_floor))       # Up
z- = max(0, (median - x) / max(1.4826 * MAD, mad_floor))       # Down
z  = max(z+, z-)                                               # Both
```

`mad_floor`, direction, sample window, and minimum sample count are explicit
configuration. Until the minimum sample count is available, readiness is
`BaselineInsufficient`; the result is not treated as familiar normality.
Diagnostics include the current value, median, MAD, retained sample count,
window, and direction. The raw `z+`/`z` value is not calibrated, so the common
reaction strength remains `None` unless an application supplies calibration.
The sample window, timestamps, exact-value recency map, and model identity are
persisted and restored.

## Nearest-neighbor reference model

`NearestNeighborModel` keeps at most `max_representatives` weighted vectors per
stream. Vectors within `merge_distance` cosine distance are merged; otherwise
the lowest-weight, oldest representative is evicted. It reports the nearest
cosine distance, the mean, median, and maximum distance to the current
representatives, and exact-value recency. The default merge distance is `0.03`;
the distance diagnostics should be used to calibrate this setting for a real
schema. This deliberately simple reference helps compare Fly's sparse-unit
generalization with a finite-example model.

## Structured failure model

`FailureModel` is a non-learning model for adapters that already have a
structured operation result. It consumes `ModelInput::Failure` instead of a
feature vector and exposes the result code, error category, and state as
evidence. An unsuccessful result produces a `Failure` reaction; success does
not become a learned normal pattern. This keeps an explicit failure signal
independent from a Fly or numeric model whose novelty may decrease when an
incident repeats.

## Persistence and compatibility

`Habituation::save`/`load`, `NumericDeviationModel::save`/`load`,
`NearestNeighborModel::save`/`load`, and `BrainRunner::save`/`load` use
versioned binary formats. Model payloads include model and feature-schema
versions and configuration fingerprints. Runner state is partitioned by brain
ID and configuration identity. Disabling a brain keeps its model state;
removing a brain removes it from the in-memory runner. Model changes require a
new state identity. `promote_shadow` explicitly copies state only between
compatible shadow and active models.

The caller supplies both the model version and the adapter feature-schema
version. A schema version covers the meaning, normalization, ordering, and
units of the input; changing those meanings requires a new compatible state
identity. A structured failure schema uses the same version fields even though
it has no vector dimension.

The cumulative `LoadBudget` is checked before saving and while loading. It
covers IDs, stream entries, profile/sample slots, representatives, unit state,
and pattern keys. A successful save cannot become unloadable merely because
the loader's allocation budget is smaller. Malformed or truncated state is
rejected transactionally.

## Replay evaluation

`examples/replay.rs` is a multi-brain comparison example:

```text
cargo run -p habitua --example replay -- records.jsonl experiment.json labels.jsonl
```

Records contain an ID, optional `source_id` for a derived record, source
timestamp, optional input-availability timestamp, stream, target, feature
schema ID/version, and features or a structured failure result. The experiment file
contains the brain list, seeds, warmup and calibration boundaries, reaction
thresholds, learning policies, and resource limits. Labels are separate
intervals or record annotations for `failure`, `important_change`,
`confirmed_normal`, and `undetermined`.

Labels are scoring-only. They never decide whether a record is learned; any
label-driven feedback must be represented by a separate time-stamped input
stream. Replay evaluates at the input-availability time, splits warmup,
calibration, and evaluation chronologically, and emits for every brain the
pre-learning report, evidence, readiness, learning weight, and processing
time. A label can be scoped by stream and target, and can match either the
record ID or its `source_id`; a parent label therefore also matches a derived
record. Omitted scope is an explicit whole-experiment label. Event detection is
counted only on the same brain input contract and within the configured finite
window from the label start (also bounded by `end_ms`), and the first reaction
is used for delay. Every in-period event remains in the denominator; an event
without a usable candidate is reported as `undetermined` or `missed`.

Each brain has an explicit replay score selector: metric name, unit, direction,
reaction kind, and label kinds. The selector is applied to the same metric that
the live reporter uses; the summary prints both the replay threshold and the
live threshold. The summary is a table for every threshold, with per-kind
recall, detection delay, misses, false notifications per confirmed-normal
operating hour, false notifications per 1,000 normal records, and the
additional events detected when each brain is added. Confirmed-normal intervals
are unioned per brain input scope; the complete confirmed-normal duration is
used for the comparable false-notification denominator. If no such interval
exists, the normal duration and rate are reported as unknown rather than
assuming one hour.

## Why there is no core salience

The former salience product multiplied novelty, input magnitude, and profile
deviation. Those values have different meanings and calibration, and the
product can hide a numeric anomaly after a pattern becomes familiar. It also
changes with feature representation and dimensionality. `habitua` therefore
returns independent model metrics and reactions; it does not manufacture a
universal single importance score. CooSenpAI or another application may
combine calibrated signals according to its own activation policy.

## Examples and benchmark

`structured_log` reads structured feature records from standard input and
returns the optional Fly metrics without panicking on invalid input.
`random_vectors` feeds deterministic random vectors. The Criterion benchmark
measures ordinary and drifting 128-dimensional Fly input, a saturated
many-pattern case, multiple streams, Numeric input, and a
4,096-dimensional/2,048-unit projection. The 50 microsecond target is a
development guide for the ordinary 128-dimensional case, not a guarantee for
every model or state size.

## CooSenpAI integration plan

CooSenpAI adapters will keep image recognition, OCR, audio recognition, and
text embeddings outside this crate. They can branch input before existing
change gates, assign stable context streams, attach feature-schema versions,
and send layout, text, sound, event, internal-log, and named numeric features
to separate brains. CooSenpAI decides whether a reaction starts observation,
conversation, or an operational notification. `habitua` supplies independent
reports, evidence, state compatibility, and explicit evaluate/commit ordering.

## License

Licensed under either of:

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
* MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
