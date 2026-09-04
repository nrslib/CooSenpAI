use super::{
    bridge::ProviderBridge, ProviderCall, ProviderCapabilities, ProviderClient,
    ProviderCompactSessionOptions, ProviderError, ProviderErrorKind, ProviderEventSink,
    ProviderMidTurnInput, ProviderName, ProviderResult,
};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct BridgeProvider {
    provider: ProviderName,
    executable: Option<PathBuf>,
    bridge: ProviderBridge,
}

impl BridgeProvider {
    pub(super) fn new(
        provider: ProviderName,
        executable: Option<PathBuf>,
        bridge: ProviderBridge,
    ) -> Self {
        Self {
            provider,
            executable,
            bridge,
        }
    }
}

#[async_trait]
impl ProviderClient for BridgeProvider {
    fn cancellation_must_complete(&self) -> bool {
        true
    }

    fn provider_name(&self) -> Option<ProviderName> {
        Some(self.provider)
    }

    fn capabilities(&self) -> Option<ProviderCapabilities> {
        self.bridge.cached_capabilities(self.provider)
    }

    async fn resolve_capabilities(
        &self,
        cancellation: CancellationToken,
        timeout: std::time::Duration,
    ) -> Result<Option<ProviderCapabilities>, ProviderError> {
        self.bridge
            .open(self.provider, cancellation, timeout)
            .await
            .map(Some)
    }

    async fn resolve_model_capabilities(
        &self,
        model: Option<&str>,
        cancellation: CancellationToken,
        timeout: std::time::Duration,
    ) -> Result<Option<ProviderCapabilities>, ProviderError> {
        self.bridge
            .resolve_model_capabilities(
                self.provider,
                self.executable.as_ref(),
                model,
                cancellation,
                timeout,
            )
            .await
            .map(Some)
    }

    async fn call(
        &self,
        input: ProviderCall,
        cancellation: CancellationToken,
    ) -> Result<ProviderResult, ProviderError> {
        self.call_streaming(input, cancellation, Arc::new(super::IgnoreProviderEvents))
            .await
    }

    async fn call_streaming(
        &self,
        input: ProviderCall,
        cancellation: CancellationToken,
        events: Arc<dyn ProviderEventSink>,
    ) -> Result<ProviderResult, ProviderError> {
        self.bridge
            .send(
                self.provider,
                self.executable.as_ref(),
                input,
                cancellation,
                events,
                None,
            )
            .await
    }

    async fn call_streaming_with_mid_turn(
        &self,
        input: ProviderCall,
        cancellation: CancellationToken,
        events: Arc<dyn ProviderEventSink>,
        additional_inputs: mpsc::UnboundedReceiver<ProviderMidTurnInput>,
    ) -> Result<ProviderResult, ProviderError> {
        self.bridge
            .send(
                self.provider,
                self.executable.as_ref(),
                input,
                cancellation,
                events,
                Some(additional_inputs),
            )
            .await
    }

    async fn compact_session(
        &self,
        _options: ProviderCompactSessionOptions,
        _cancellation: CancellationToken,
    ) -> Result<(), ProviderError> {
        Err(ProviderError {
            kind: ProviderErrorKind::Unsupported,
            message: "bridge の compact 操作はまだ利用できません。".to_owned(),
        })
    }
}
