use super::{
    ProviderCall, ProviderCapabilities, ProviderClient, ProviderError, ProviderErrorKind,
    ProviderName, ProviderResult, ProviderSession, SessionRequest,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

pub const MOCK_COMPANION_RESPONSE: &str = "E2E MOCK RESPONSE: accepted.";
pub const MOCK_OBSERVER_ACTIVITY: &str = "E2E MOCK OBSERVATION: screen accepted.";
pub const MOCK_MEMORY_RESPONSE: &str = "E2E MOCK MEMORY: consolidated.";

/// GUI E2E とオフラインの通常運用検査だけで使う固定応答 provider。
///
/// bridge を起動せず、認証情報も読み込まない。応答の形は既存の observer / companion
/// structured output 契約に合わせるため、呼び出し側の schema 以外の挙動は本番 provider
/// と同じ runtime 経路を通る。
#[derive(Debug, Clone, Copy, Default)]
pub struct MockProvider;

impl MockProvider {
    pub fn new() -> Self {
        Self
    }

    fn session(input: &ProviderCall) -> Result<ProviderSession, ProviderError> {
        if let SessionRequest::Resume(session) = &input.session {
            if session.provider != ProviderName::Mock {
                return Err(ProviderError {
                    kind: ProviderErrorKind::InvalidOutput,
                    message: "mock provider の session provider が一致しません。".to_owned(),
                });
            }
        }
        Ok(ProviderSession {
            provider: ProviderName::Mock,
            model: input.model.clone().filter(|model| model != "default"),
            id: "mock-session".to_owned(),
        })
    }

    fn response(input: &ProviderCall) -> Result<(String, Option<Value>), ProviderError> {
        let Some(schema) = input.output_schema.as_ref() else {
            return Ok(("E2E MOCK CONNECTION: OK".to_owned(), None));
        };
        let properties = schema.get("properties");
        if properties.and_then(|value| value.get("activity")).is_some() {
            let value = json!({
                "activity": MOCK_OBSERVER_ACTIVITY,
                "outline": "E2E mock observation outline.",
                "changes": ["E2E mock observation accepted."],
                "events": [],
                "guess": null,
                "confidence": null,
                "wakeCompanion": false
            });
            return Ok((value.to_string(), Some(value)));
        }
        if properties.and_then(|value| value.get("emit")).is_some() {
            let value = json!({
                "emit": true,
                "message": MOCK_COMPANION_RESPONSE,
                "messageKind": "chat",
                "notificationPriority": "none",
                "factCandidates": [],
                "factUpdates": []
            });
            return Ok((MOCK_COMPANION_RESPONSE.to_owned(), Some(value)));
        }
        if properties.and_then(|value| value.get("text")).is_some() {
            return Ok((
                MOCK_MEMORY_RESPONSE.to_owned(),
                Some(json!({"text": MOCK_MEMORY_RESPONSE})),
            ));
        }
        Err(ProviderError {
            kind: ProviderErrorKind::Unsupported,
            message: "mock provider が対応していない出力 schema です。".to_owned(),
        })
    }
}

#[async_trait]
impl ProviderClient for MockProvider {
    fn provider_name(&self) -> Option<ProviderName> {
        Some(ProviderName::Mock)
    }

    fn capabilities(&self) -> Option<ProviderCapabilities> {
        Some(ProviderCapabilities {
            default_model: "mock".to_owned(),
            model_candidates: vec!["mock".to_owned()],
            image_input: true,
            native_structured_output: true,
            effective_structured_output: true,
            streaming: true,
            cancellation: true,
            session_resume: true,
            session_compact: false,
            effort: true,
            mid_turn_input: false,
        })
    }

    async fn call(
        &self,
        input: ProviderCall,
        cancellation: CancellationToken,
    ) -> Result<ProviderResult, ProviderError> {
        if cancellation.is_cancelled() {
            return Err(ProviderError {
                kind: ProviderErrorKind::Retryable,
                message: "mock provider をキャンセルしました".to_owned(),
            });
        }
        let (text, value) = Self::response(&input)?;
        Ok(ProviderResult {
            text,
            value,
            session: Some(Self::session(&input)?),
        })
    }

    async fn compact_session(
        &self,
        _options: super::ProviderCompactSessionOptions,
        _cancellation: CancellationToken,
    ) -> Result<(), ProviderError> {
        Err(ProviderError {
            kind: ProviderErrorKind::Unsupported,
            message: "mock provider は session compact に対応していません。".to_owned(),
        })
    }
}
