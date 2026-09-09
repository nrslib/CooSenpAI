use crate::state::AudioObservationSource;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

pub const MAX_HEARING_TEXT_BYTES: usize = 4096;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HearingContext {
    pub session_id: String,
    pub source: AudioObservationSource,
    pub generation: u64,
    pub sequence: u64,
    pub text: String,
    pub confirmed: bool,
}

impl HearingContext {
    pub(crate) fn same_segment(&self, other: &Self) -> bool {
        self.session_id == other.session_id
            && self.source == other.source
            && self.generation == other.generation
    }
}

pub(crate) fn deserialize_contexts<'de, D>(deserializer: D) -> Result<Vec<HearingContext>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let contexts = Vec::<HearingContext>::deserialize(deserializer)?;
    validate_contexts(&contexts).map_err(serde::de::Error::custom)?;
    Ok(contexts)
}

pub(crate) fn validate_contexts(contexts: &[HearingContext]) -> Result<(), &'static str> {
    if contexts.len() > 2
        || contexts.iter().enumerate().any(|(index, context)| {
            uuid::Uuid::parse_str(&context.session_id).is_err()
                || context.generation == 0
                || context.sequence == 0
                || context.text.len() > MAX_HEARING_TEXT_BYTES
                || contexts[..index]
                    .iter()
                    .any(|previous| previous.source == context.source)
        })
    {
        return Err("音声文脈の世代、更新順序、件数またはサイズが不正です");
    }
    Ok(())
}

#[derive(Default)]
pub(crate) struct HearingContextBuffer {
    session: Option<(u64, String, CancellationToken)>,
    latest: Vec<HearingContext>,
}

impl HearingContextBuffer {
    pub(crate) fn begin(
        &mut self,
        generation: u64,
        cancellation: CancellationToken,
    ) -> Option<String> {
        if generation == 0
            || cancellation.is_cancelled()
            || self
                .session
                .as_ref()
                .is_some_and(|(previous, _, _)| *previous >= generation)
        {
            return None;
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.session = Some((generation, id.clone(), cancellation));
        self.latest.clear();
        Some(id)
    }

    pub(crate) fn prepare_update(&self, mut context: HearingContext) -> Option<HearingContext> {
        let (_, session_id, cancellation) = self.session.as_ref()?;
        if cancellation.is_cancelled()
            || context.session_id != *session_id
            || context.generation == 0
            || context.sequence == 0
            || self.latest.iter().any(|previous| {
                previous.source == context.source
                    && (context.generation < previous.generation
                        || (context.generation == previous.generation
                            && (context.sequence <= previous.sequence || previous.confirmed)))
            })
        {
            return None;
        }
        let mut end = context.text.len().min(MAX_HEARING_TEXT_BYTES);
        while !context.text.is_char_boundary(end) {
            end -= 1;
        }
        context.text.truncate(end);
        Some(context)
    }

    pub(crate) fn record(&mut self, context: HearingContext) {
        self.latest
            .retain(|previous| previous.source != context.source);
        self.latest.push(context);
    }

    pub(crate) fn snapshot(&self) -> Vec<HearingContext> {
        if self
            .session
            .as_ref()
            .is_none_or(|(_, _, cancellation)| cancellation.is_cancelled())
        {
            return Vec::new();
        }
        self.latest
            .iter()
            .filter(|context| !context.text.is_empty())
            .cloned()
            .collect()
    }
}
