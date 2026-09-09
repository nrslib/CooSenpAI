use crate::update_format::{self, MAX_ARCHIVE_BYTES};
use coosenpai_core::locale::{text, Locale, TextKey};
use minisign_verify::{PublicKey, Signature};
use reqwest::{redirect::Policy, Client, Url};
use semver::Version;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

const METADATA_URL: &str =
    "https://github.com/nrslib/CooSenpAI/releases/latest/download/latest.json";
const MAX_METADATA_BYTES: usize = 1024 * 1024;

pub(crate) struct UpdateClient {
    client: Client,
    key: PublicKey,
    current: Version,
    architecture: String,
    locale: Locale,
}

pub(crate) struct PendingUpdate {
    pub(crate) version: Version,
    pub(crate) notes: Option<String>,
    url: Url,
    signature: Signature,
}

#[derive(Deserialize)]
struct UpdateConfig {
    pubkey: String,
    endpoints: Vec<String>,
}

#[derive(Deserialize)]
struct Manifest {
    version: Version,
    notes: Option<String>,
    platforms: HashMap<String, Artifact>,
}

#[derive(Deserialize)]
struct Artifact {
    url: Url,
    signature: String,
}

impl UpdateClient {
    #[cfg(test)]
    pub(crate) fn new(
        config: &serde_json::Value,
        current: Version,
        architecture: &str,
    ) -> Result<Self, String> {
        Self::new_for_locale(config, current, architecture, Locale::Ja)
    }

    pub(crate) fn new_for_locale(
        config: &serde_json::Value,
        current: Version,
        architecture: &str,
        locale: Locale,
    ) -> Result<Self, String> {
        let config: UpdateConfig = serde_json::from_value(config.clone())
            .map_err(|_| text(TextKey::UpdateConfigInvalid, locale).to_owned())?;
        if config.endpoints != [METADATA_URL] || !matches!(architecture, "aarch64" | "x86_64") {
            return Err(text(TextKey::UpdateDestinationInvalid, locale).to_owned());
        }
        let client = Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .user_agent(concat!("CooSenpAI/", env!("CARGO_PKG_VERSION")))
            .redirect(Policy::custom(move |attempt| {
                if attempt.previous().len() >= 5 || !allowed_distribution_url(attempt.url()) {
                    attempt.error(text(TextKey::UpdateRedirectNotAllowed, locale))
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(|_| text(TextKey::UpdateTransportInitFailed, locale).to_owned())?;
        Ok(Self {
            client,
            key: update_format::public_key_for_locale(&config.pubkey, locale)?,
            current,
            architecture: architecture.to_owned(),
            locale,
        })
    }

    pub(crate) async fn check(&self) -> Result<Option<PendingUpdate>, String> {
        let bytes = self
            .get(
                Url::parse(METADATA_URL).expect("static update URL"),
                MAX_METADATA_BYTES,
                Duration::from_secs(15),
                |_, _| {},
            )
            .await?;
        self.parse_manifest(&bytes)
    }

    fn parse_manifest(&self, bytes: &[u8]) -> Result<Option<PendingUpdate>, String> {
        if bytes.len() > MAX_METADATA_BYTES {
            return Err(text(TextKey::UpdateMetadataTooLarge, self.locale).to_owned());
        }
        let manifest: Manifest = serde_json::from_slice(bytes)
            .map_err(|_| text(TextKey::UpdateMetadataInvalid, self.locale).to_owned())?;
        if !manifest.version.pre.is_empty() || manifest.version <= self.current {
            return Ok(None);
        }
        let artifact = manifest
            .platforms
            .get(&format!("darwin-{}", self.architecture))
            .ok_or_else(|| text(TextKey::UpdateArtifactMissing, self.locale).to_owned())?;
        let expected = format!("https://github.com/nrslib/CooSenpAI/releases/download/v{0}/CooSenpAI_{0}_{1}.app.tar.gz", manifest.version, self.architecture);
        if artifact.url.as_str() != expected || !allowed_distribution_url(&artifact.url) {
            return Err(text(TextKey::UpdateArtifactUrlInvalid, self.locale).to_owned());
        }
        Ok(Some(PendingUpdate {
            version: manifest.version,
            notes: manifest.notes,
            url: artifact.url.clone(),
            signature: update_format::signature_for_locale(&artifact.signature, self.locale)?,
        }))
    }

    pub(crate) async fn download(
        &self,
        update: &PendingUpdate,
        progress: impl FnMut(u64, Option<u64>),
    ) -> Result<update_format::VerifiedArchive, String> {
        let bytes = self
            .get(
                update.url.clone(),
                MAX_ARCHIVE_BYTES,
                Duration::from_secs(300),
                progress,
            )
            .await?;
        update_format::verify_for_locale(bytes, &update.signature, &self.key, self.locale)
    }

    async fn get(
        &self,
        url: Url,
        limit: usize,
        timeout: Duration,
        mut progress: impl FnMut(u64, Option<u64>),
    ) -> Result<Vec<u8>, String> {
        if !allowed_distribution_url(&url) {
            return Err(text(TextKey::UpdateDestinationNotAllowed, self.locale).to_owned());
        }
        // CDN の署名付き query をログや renderer のエラーへ含めない。
        let mut response = self
            .client
            .get(url)
            .timeout(timeout)
            .send()
            .await
            .map_err(|_| text(TextKey::UpdateServerCommunication, self.locale).to_owned())?;
        if !response.status().is_success() {
            return Err(text(TextKey::UpdateHttpStatus, self.locale)
                .replace("{status}", &response.status().as_u16().to_string()));
        }
        let total = response.content_length();
        if total.is_some_and(|size| size > limit as u64) {
            return Err(text(TextKey::UpdateFileTooLarge, self.locale).to_owned());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| text(TextKey::UpdateDownloadInterrupted, self.locale).to_owned())?
        {
            append_chunk_for_locale(&mut bytes, &chunk, limit, self.locale)?;
            progress(bytes.len() as u64, total);
        }
        Ok(bytes)
    }
}

fn append_chunk_for_locale(
    bytes: &mut Vec<u8>,
    chunk: &[u8],
    limit: usize,
    locale: Locale,
) -> Result<(), String> {
    if chunk.len() > limit.saturating_sub(bytes.len()) {
        return Err(text(TextKey::UpdateFileTooLarge, locale).to_owned());
    }
    bytes.extend_from_slice(chunk);
    Ok(())
}

fn allowed_distribution_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}

