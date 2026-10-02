use std::io::{self, Read, Write};

use crate::model::{
    Evaluation, FailureObservation, Model, ModelError, ModelIdentity, ModelInput, PersistentModel,
    validate_learning_weight,
};
use crate::pattern::stable_hash;
use crate::persistence::{read_u16, read_u32, read_u64, write_u16, write_u32, write_u64};
use crate::reaction::Readiness;
use crate::timestamp::Timestamp;

const FAILURE_ALGORITHM_VERSION: u64 = 1;
const FAILURE_STATE_VERSION: u16 = 1;
const FAILURE_MAGIC: &[u8; 8] = b"FAILURE\0";

/// Configuration for the structured failure model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FailureModelConfig {
    /// Version of the failure rule implementation.
    pub model_version: u32,
    /// Version of the adapter's structured-result schema.
    pub feature_schema_version: u32,
}

impl Default for FailureModelConfig {
    fn default() -> Self {
        Self {
            model_version: 1,
            feature_schema_version: 1,
        }
    }
}

impl FailureModelConfig {
    fn validate(self) -> Result<(), ModelError> {
        if self.model_version == 0 || self.feature_schema_version == 0 {
            return Err(ModelError::Incompatible(
                "failure model and schema versions must be non-zero".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Diagnostics emitted by [`FailureModel`].
#[derive(Clone, Debug, PartialEq)]
pub struct FailureDiagnostics {
    /// The structured result supplied by the adapter.
    pub result: FailureObservation,
}

/// An update from [`FailureModel`]. It carries identity checks even though the
/// model itself has no learning state.
pub struct FailureUpdate {
    model_token: u64,
    configuration_fingerprint: u64,
}

/// A non-learning model that turns structured operation failures into
/// evaluations. It does not consume or encode feature vectors.
pub struct FailureModel {
    config: FailureModelConfig,
    model_token: u64,
}

impl FailureModel {
    /// Creates a failure model with the supplied model and schema versions.
    pub fn new(config: FailureModelConfig) -> Result<Self, ModelError> {
        config.validate()?;
        Ok(Self {
            config,
            model_token: next_failure_token(),
        })
    }

    /// Returns the model configuration.
    pub fn config(&self) -> FailureModelConfig {
        self.config
    }

    fn evaluate_failure(
        &self,
        _stream_id: &str,
        result: &FailureObservation,
        _at: Timestamp,
    ) -> Result<Evaluation<FailureDiagnostics, FailureUpdate>, ModelError> {
        Ok(Evaluation {
            novelty_short: None,
            novelty_long: None,
            familiarity_short: None,
            familiarity_long: None,
            recency: None,
            pattern_id: None,
            readiness: Readiness::Evaluated,
            diagnostics: FailureDiagnostics {
                result: result.clone(),
            },
            update: FailureUpdate {
                model_token: self.model_token,
                configuration_fingerprint: self.fingerprint(),
            },
        })
    }
}

impl Model for FailureModel {
    type Diagnostics = FailureDiagnostics;
    type Update = FailureUpdate;

    fn evaluate(
        &self,
        _stream_id: &str,
        _features: &[f32],
        _at: Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError> {
        Err(ModelError::Incompatible(
            "failure model requires structured failure input".to_owned(),
        ))
    }

    fn evaluate_input(
        &self,
        stream_id: &str,
        input: ModelInput<'_>,
        at: Timestamp,
    ) -> Result<Evaluation<Self::Diagnostics, Self::Update>, ModelError> {
        match input {
            ModelInput::Failure(result) => self.evaluate_failure(stream_id, result, at),
            ModelInput::Features(_) => Err(ModelError::Incompatible(
                "failure model requires structured failure input".to_owned(),
            )),
        }
    }

    fn commit(&mut self, update: Self::Update, weight: f32) -> Result<(), ModelError> {
        validate_learning_weight(weight)?;
        if update.model_token != self.model_token
            || update.configuration_fingerprint != self.fingerprint()
        {
            return Err(ModelError::ForeignUpdate);
        }
        Ok(())
    }

    fn identity(&self) -> ModelIdentity {
        ModelIdentity {
            model_version: self.config.model_version,
            feature_schema_version: self.config.feature_schema_version,
        }
    }

    fn fingerprint(&self) -> u64 {
        stable_hash([
            FAILURE_ALGORITHM_VERSION,
            u64::from(self.config.model_version),
            u64::from(self.config.feature_schema_version),
        ])
    }
}

impl PersistentModel for FailureModel {
    fn state_format_version(&self) -> u16 {
        FAILURE_STATE_VERSION
    }

    fn save_state(&self, writer: &mut dyn Write) -> io::Result<()> {
        writer.write_all(FAILURE_MAGIC)?;
        write_u16(writer, FAILURE_STATE_VERSION)?;
        write_u64(writer, self.fingerprint())?;
        write_u32(writer, self.config.model_version)?;
        write_u32(writer, self.config.feature_schema_version)
    }

    fn load_state(&mut self, reader: &mut dyn Read) -> io::Result<()> {
        let mut magic = [0_u8; FAILURE_MAGIC.len()];
        reader.read_exact(&mut magic)?;
        if &magic != FAILURE_MAGIC {
            return Err(invalid_data("invalid failure model state marker"));
        }
        let version = read_u16(reader)?;
        if version != FAILURE_STATE_VERSION {
            return Err(invalid_data("unsupported failure model state version"));
        }
        let fingerprint = read_u64(reader)?;
        let model_version = read_u32(reader)?;
        let schema_version = read_u32(reader)?;
        if model_version != self.config.model_version
            || schema_version != self.config.feature_schema_version
            || fingerprint != self.fingerprint()
        {
            return Err(invalid_data("failure model state identity mismatch"));
        }
        let mut trailing = [0_u8; 1];
        if reader.read(&mut trailing)? != 0 {
            return Err(invalid_data("trailing bytes in failure model state"));
        }
        Ok(())
    }

    fn reserve_load_budget(&self, _load_budget: &mut crate::LoadBudget) -> io::Result<()> {
        Ok(())
    }

    fn load_state_with_budget(
        &mut self,
        reader: &mut dyn Read,
        _load_budget: &mut crate::LoadBudget,
    ) -> io::Result<()> {
        self.load_state(reader)
    }
}

fn next_failure_token() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TOKEN: AtomicU64 = AtomicU64::new(1);
    TOKEN.fetch_add(1, Ordering::Relaxed)
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
