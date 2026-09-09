use async_trait::async_trait;
use coosenpai_core::locale::{text as localized_text, Locale, TextKey};
use coosenpai_core::process::{ProcessRequest, ProcessRunner, TokioProcessRunner};
use coosenpai_core::voice_output::VoiceOutputProvider;
use reqwest::{redirect::Policy, Client, RequestBuilder, Url};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const ENDPOINT: &str = "http://127.0.0.1:50021/";
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_WAV_BYTES: usize = 32 * 1024 * 1024;
const ENGINE_ERROR: &str = "VOICEVOX ENGINE と通信できませんでした。ローカルのエンジンが起動していることを確認してください";
const RESPONSE_ERROR: &str = "VOICEVOX ENGINE の応答が不正です";
const FILE_ERROR: &str = "VOICEVOX の一時音声ファイルを準備または削除できませんでした";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VoicevoxVoice {
    pub id: u32,
    pub name: String,
    pub style_name: String,
}

#[derive(Deserialize)]
struct Speaker {
    name: String,
    styles: Vec<Style>,
}

#[derive(Deserialize)]
struct Style {
    id: u32,
    name: String,
    #[serde(rename = "type", default = "talk_style")]
    kind: String,
}

fn talk_style() -> String {
    "talk".to_owned()
}

struct EngineClient {
    client: Client,
    endpoint: Url,
}

impl EngineClient {
    fn new() -> Result<Self, String> {
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .read_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| ENGINE_ERROR.to_owned())?;
        let endpoint = Url::parse(ENDPOINT).map_err(|_| ENGINE_ERROR.to_owned())?;
        Ok(Self { client, endpoint })
    }

    fn url(&self, path: &str) -> Result<Url, String> {
        self.endpoint
            .join(path)
            .map_err(|_| ENGINE_ERROR.to_owned())
    }

    async fn voices(&self) -> Result<Vec<VoicevoxVoice>, String> {
        let request = self
            .client
            .get(self.url("speakers")?)
            .timeout(Duration::from_secs(10));
        parse_voices(&bounded_response(request, MAX_JSON_BYTES).await?)
    }

    async fn synthesize(&self, text: &str, rate: u32, style_id: u32) -> Result<Vec<u8>, String> {
        let query = self
            .client
            .post(self.url("audio_query")?)
            .query(&[("text", text.to_owned()), ("speaker", style_id.to_string())]);
        let bytes = bounded_response(query, MAX_JSON_BYTES).await?;
        let mut query: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| RESPONSE_ERROR.to_owned())?;
        let object = query.as_object_mut().ok_or(RESPONSE_ERROR)?;
        if !object
            .get("accent_phrases")
            .is_some_and(serde_json::Value::is_array)
            || !object
                .get("speedScale")
                .is_some_and(serde_json::Value::is_number)
        {
            return Err(RESPONSE_ERROR.to_owned());
        }
        object.insert(
            "speedScale".to_owned(),
            serde_json::json!(f64::from(rate) / 180.0),
        );
        let synthesis = self
            .client
            .post(self.url("synthesis")?)
            .query(&[("speaker", style_id)])
            .json(&query);
        let bytes = bounded_response(synthesis, MAX_WAV_BYTES).await?;
        validate_wav(&bytes)?;
        Ok(bytes)
    }
}

pub async fn voicevox_voices(
    locale: Locale,
    cancellation: CancellationToken,
) -> Result<Vec<VoicevoxVoice>, String> {
    let engine = EngineClient::new().map_err(|error| localize_error(error, locale))?;
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(localized_text(TextKey::VoicevoxListCancelled, locale).to_owned()),
        result = engine.voices() => result.map_err(|error| localize_error(error, locale)),
    }
}

pub struct VoicevoxProvider {
    engine: EngineClient,
    style_id: u32,
    runner: Arc<dyn ProcessRunner>,
}

impl VoicevoxProvider {
    pub fn new(style_id: u32, locale: Locale) -> Result<Self, String> {
        if style_id > i32::MAX as u32 {
            return Err(localized_text(TextKey::VoicevoxInvalidStyle, locale).to_owned());
        }
        Ok(Self {
            engine: EngineClient::new().map_err(|error| localize_error(error, locale))?,
            style_id,
            runner: Arc::new(TokioProcessRunner),
        })
    }
}

#[async_trait]
impl VoiceOutputProvider for VoicevoxProvider {
    async fn speak(
        &self,
        text: &str,
        rate: u32,
        locale: Locale,
        cancellation: CancellationToken,
    ) -> Result<(), String> {
        self.speak_inner(text, rate, cancellation)
            .await
            .map_err(|error| localize_error(error, locale))
    }
}

impl VoicevoxProvider {
    async fn speak_inner(
        &self,
        text: &str,
        rate: u32,
        cancellation: CancellationToken,
    ) -> Result<(), String> {
        if cancellation.is_cancelled() {
            return Ok(());
        }
        if text.trim().is_empty() || text.len() > 16 * 1024 || !(100..=400).contains(&rate) {
            return Err("読み上げる文章または速度が不正です".to_owned());
        }
        let bytes = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Ok(()),
            result = tokio::time::timeout(Duration::from_secs(120), self.engine.synthesize(text, rate, self.style_id)) => {
                result.map_err(|_| ENGINE_ERROR.to_owned())??
            },
        };
        let file =
            tokio::task::spawn_blocking(move || -> Result<tempfile::NamedTempFile, String> {
                let mut file = tempfile::Builder::new()
                    .prefix("coosenpai-voicevox-")
                    .suffix(".wav")
                    .permissions(std::fs::Permissions::from_mode(0o600))
                    .tempfile()
                    .map_err(|_| FILE_ERROR.to_owned())?;
                file.write_all(&bytes)
                    .and_then(|_| file.flush())
                    .map_err(|_| FILE_ERROR.to_owned())?;
                Ok(file)
            })
            .await
            .map_err(|_| FILE_ERROR.to_owned())??;
        let result = if cancellation.is_cancelled() {
            Ok(())
        } else {
            let path = file.path().to_str().ok_or(FILE_ERROR)?;
            // 本文やエンジンの応答をプロセス引数・エラーへ渡さない。
            let result = self
                .runner
                .run(
                    ProcessRequest {
                        executable: "/usr/bin/afplay".into(),
                        args: vec![path.to_owned()],
                        env: Vec::new(),
                        cwd: None,
                        stdin: Vec::new(),
                        timeout: Duration::from_secs(180),
                    },
                    cancellation.clone(),
                )
                .await;
            if cancellation.is_cancelled() {
                Ok(())
            } else {
                match result {
                    Ok(output) if output.status == Some(0) => Ok(()),
                    _ => Err("VOICEVOX の音声を再生できませんでした".to_owned()),
                }
            }
        };
        file.close().map_err(|_| FILE_ERROR.to_owned())?;
        result
    }
}

async fn bounded_response(request: RequestBuilder, limit: usize) -> Result<Vec<u8>, String> {
    let mut response = request.send().await.map_err(|_| ENGINE_ERROR.to_owned())?;
    if !response.status().is_success() {
        return Err(ENGINE_ERROR.to_owned());
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(RESPONSE_ERROR.to_owned());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ENGINE_ERROR.to_owned())?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(RESPONSE_ERROR.to_owned());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_voices(bytes: &[u8]) -> Result<Vec<VoicevoxVoice>, String> {
    let speakers: Vec<Speaker> =
        serde_json::from_slice(bytes).map_err(|_| RESPONSE_ERROR.to_owned())?;
    let mut voices = Vec::new();
    let mut ids = HashSet::new();
    for speaker in speakers {
        if speaker.name.trim().is_empty() || speaker.name.len() > 256 {
            return Err(RESPONSE_ERROR.to_owned());
        }
        for style in speaker.styles {
            if style.kind != "talk" {
                continue;
            }
            if style.name.trim().is_empty()
                || style.name.len() > 256
                || style.id > i32::MAX as u32
                || voices.len() >= 4096
                || !ids.insert(style.id)
            {
                return Err(RESPONSE_ERROR.to_owned());
            }
            voices.push(VoicevoxVoice {
                id: style.id,
                name: speaker.name.clone(),
                style_name: style.name,
            });
        }
    }
    Ok(voices)
}

fn validate_wav(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 12
        || &bytes[..4] != b"RIFF"
        || &bytes[8..12] != b"WAVE"
        || u32::from_le_bytes(bytes[4..8].try_into().map_err(|_| RESPONSE_ERROR)?) as usize
            != bytes.len() - 8
    {
        return Err(RESPONSE_ERROR.to_owned());
    }
    let mut offset = 12usize;
    let mut block_align = None;
    let mut data_length = None;
    while offset < bytes.len() {
        let header = bytes.get(offset..offset + 8).ok_or(RESPONSE_ERROR)?;
        let size =
            u32::from_le_bytes(header[4..8].try_into().map_err(|_| RESPONSE_ERROR)?) as usize;
        let start = offset + 8;
        let end = start.checked_add(size).ok_or(RESPONSE_ERROR)?;
        let chunk = bytes.get(start..end).ok_or(RESPONSE_ERROR)?;
        if &header[..4] == b"fmt " {
            if block_align.is_some() || size < 16 {
                return Err(RESPONSE_ERROR.to_owned());
            }
            let format = u16::from_le_bytes([chunk[0], chunk[1]]);
            let channels = u16::from_le_bytes([chunk[2], chunk[3]]);
            let sample_rate =
                u32::from_le_bytes(chunk[4..8].try_into().map_err(|_| RESPONSE_ERROR)?);
            let byte_rate =
                u32::from_le_bytes(chunk[8..12].try_into().map_err(|_| RESPONSE_ERROR)?);
            let alignment = u16::from_le_bytes([chunk[12], chunk[13]]);
            let bits = u16::from_le_bytes([chunk[14], chunk[15]]);
            if format != 1
                || !(1..=2).contains(&channels)
                || !(8000..=192000).contains(&sample_rate)
                || ![8, 16, 24, 32].contains(&bits)
                || alignment != channels * (bits / 8)
                || byte_rate != sample_rate * u32::from(alignment)
            {
                return Err(RESPONSE_ERROR.to_owned());
            }
            block_align = Some(usize::from(alignment));
        } else if &header[..4] == b"data" && (data_length.replace(size).is_some() || size == 0) {
            return Err(RESPONSE_ERROR.to_owned());
        }
        offset = end.checked_add(size % 2).ok_or(RESPONSE_ERROR)?;
        if offset > bytes.len() {
            return Err(RESPONSE_ERROR.to_owned());
        }
    }
    match (block_align, data_length) {
        (Some(align), Some(length)) if length % align == 0 => Ok(()),
        _ => Err(RESPONSE_ERROR.to_owned()),
    }
}

fn localize_error(error: String, locale: Locale) -> String {
    let key = match error.as_str() {
        ENGINE_ERROR => TextKey::VoicevoxEngineFailed,
        RESPONSE_ERROR => TextKey::VoicevoxInvalidResponse,
        FILE_ERROR => TextKey::VoicevoxFileFailed,
        "読み上げる文章または速度が不正です" => TextKey::VoiceOutputInvalidInput,
        "VOICEVOX の音声を再生できませんでした" => TextKey::VoicevoxPlaybackFailed,
        _ => return error,
    };
    localized_text(key, locale).to_owned()
}

