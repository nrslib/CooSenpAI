use crate::config::VoiceOutputConfig;
use crate::locale::Locale;
use async_trait::async_trait;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// 音声出力は音声入力の SpeechPort とは独立した Provider 境界とする。
#[async_trait]
pub trait VoiceOutputProvider: Send + Sync {
    /// 正常時は再生完了まで、キャンセル時は実音声の停止・資源回収まで待つ。
    async fn speak(
        &self,
        text: &str,
        rate: u32,
        locale: Locale,
        cancellation: CancellationToken,
    ) -> Result<(), String>;
}

/// 再生ごとの設定から Provider を構築し、実行中の Provider の設定は変えない。
pub trait VoiceOutputProviderFactory: Send + Sync {
    fn create(
        &self,
        config: &VoiceOutputConfig,
        locale: Locale,
    ) -> Result<Arc<dyn VoiceOutputProvider>, String>;
}
