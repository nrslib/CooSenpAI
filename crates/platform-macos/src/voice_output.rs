use async_trait::async_trait;
use coosenpai_core::locale::{text as localized_text, Locale, TextKey};
use coosenpai_core::process::{ProcessRequest, ProcessRunner, TokioProcessRunner};
use coosenpai_core::voice_output::VoiceOutputProvider;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub struct MacSystemVoiceProvider {
    runner: Arc<dyn ProcessRunner>,
}

impl Default for MacSystemVoiceProvider {
    fn default() -> Self {
        Self {
            runner: Arc::new(TokioProcessRunner),
        }
    }
}

#[async_trait]
impl VoiceOutputProvider for MacSystemVoiceProvider {
    async fn speak(
        &self,
        text: &str,
        rate: u32,
        locale: Locale,
        cancellation: CancellationToken,
    ) -> Result<(), String> {
        if cancellation.is_cancelled() {
            return Ok(());
        }
        if text.trim().is_empty() || text.len() > 16 * 1024 || !(100..=400).contains(&rate) {
            return Err(localized_text(TextKey::VoiceOutputInvalidInput, locale).to_owned());
        }
        // say の埋め込み制御コマンド [[...]] を本文として扱う。シェルや argv に本文を渡さない。
        let plain_text = text.replace('[', "［").replace(']', "］");
        let result = self
            .runner
            .run(
                ProcessRequest {
                    executable: "/usr/bin/say".into(),
                    args: vec!["--input-file=-".to_owned(), format!("--rate={rate}")],
                    env: Vec::new(),
                    cwd: None,
                    stdin: plain_text.into_bytes(),
                    timeout: Duration::from_secs(180),
                },
                cancellation.clone(),
            )
            .await;
        if cancellation.is_cancelled() {
            return Ok(());
        }
        match result {
            Ok(output) if output.status == Some(0) => Ok(()),
            // stderr に本文が含まれる可能性があるため、UI・ログへ転送しない。
            _ => Err(localized_text(TextKey::VoiceOutputFailed, locale).to_owned()),
        }
    }
}

