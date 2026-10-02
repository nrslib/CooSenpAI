use crate::connectome::{
    existing_private_children, pack_directory, pack_path_reason, private_children, verify_pack,
    ARTIFACT_NAME,
};
use coosenpai_core::config::ConfigPaths;
use habitua_connectome::RateHelperArtifact;
use reqwest::{redirect::Policy, Client};
use serde::Deserialize;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::ffi::CString;
use std::fs::{self, File};
use std::io;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub(crate) struct DownloadTask {
    pub cancel: CancellationToken,
    pub join: tokio::task::JoinHandle<()>,
}

pub(crate) async fn cancel_and_wait(task: &tokio::sync::Mutex<Option<DownloadTask>>) {
    let running = task.lock().await.take();
    if let Some(running) = running {
        running.cancel.cancel();
        let _ = running.join.await;
    }
}

pub(crate) const ARCHIVE_BYTES: u64 = 605_276_160;
const ARCHIVE_NAME: &str = "malecns-v1.0-rate-full.tar";
const STAGING_PREFIX: &str = ".rate-full-download-";
const PACK_FILES: [&str; 5] = [
    "rate-neurons.json",
    "rate-out.bin",
    "rate-in.bin",
    "rate-populations.json",
    "rate_manifest.json",
];

struct DownloadSpec<'a> {
    distribution_url: Cow<'a, str>,
    archive_url: Cow<'a, str>,
    archive_sha256: Cow<'a, str>,
    archive_bytes: u64,
}

const DISTRIBUTION: DownloadSpec<'static> = DownloadSpec {
    distribution_url: Cow::Borrowed("https://github.com/nrslib/habitua/releases/download/v0.1.0/malecns-v1.0-rate-full.distribution.json"),
    archive_url: Cow::Borrowed("https://github.com/nrslib/habitua/releases/download/v0.1.0/malecns-v1.0-rate-full.tar"),
    archive_sha256: Cow::Borrowed("203400f78a1366e83eb0906d373e7c4cd317b97869cf636462caf9b20da4c390"),
    archive_bytes: ARCHIVE_BYTES,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DownloadView {
    pub phase: String,
    pub received_bytes: u64,
    pub total_bytes: u64,
    pub reason: Option<String>,
}

impl Default for DownloadView {
    fn default() -> Self {
        Self {
            phase: "idle".to_owned(),
            received_bytes: 0,
            total_bytes: ARCHIVE_BYTES,
            reason: None,
        }
    }
}

impl DownloadView {
    pub(crate) fn complete() -> Self {
        Self {
            phase: "complete".to_owned(),
            ..Self::default()
        }
    }

    pub(crate) fn downloading(received_bytes: u64, total_bytes: u64) -> Self {
        Self {
            phase: "downloading".to_owned(),
            received_bytes,
            total_bytes,
            reason: None,
        }
    }

    pub(crate) fn failed(reason: &str) -> Self {
        Self {
            phase: "failed".to_owned(),
            received_bytes: 0,
            total_bytes: ARCHIVE_BYTES,
            reason: Some(reason.to_owned()),
        }
    }

    pub(crate) fn verifying() -> Self {
        Self {
            phase: "verifying".to_owned(),
            reason: Some("pack-verifying".to_owned()),
            ..Self::default()
        }
    }
}

#[derive(Deserialize)]
struct Distribution {
    schema: String,
    archive: String,
    archive_sha256: String,
    pack_manifest_sha256: String,
}

async fn fetch_distribution(
    client: &Client,
    url: &str,
    cancel: &CancellationToken,
) -> Result<Distribution, &'static str> {
    let mut response = tokio::select! {
        _ = cancel.cancelled() => return Err("cancelled"),
        response = client.get(url).send() => response.map_err(|_| "network-error")?,
    }
    .error_for_status()
    .map_err(|_| "network-error")?;
    const MAX_DISTRIBUTION_BYTES: usize = 64 * 1024;
    let mut bytes = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err("cancelled"),
            chunk = response.chunk() => chunk.map_err(|_| "network-error")?,
        };
        let Some(chunk) = chunk else { break };
        if bytes.len() + chunk.len() > MAX_DISTRIBUTION_BYTES {
            return Err("distribution-invalid");
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "distribution-invalid")
}

pub(crate) fn client() -> Result<Client, &'static str> {
    Client::builder()
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 || attempt.url().scheme() != "https" {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| "network-error")
}

pub(crate) fn cleanup_stale(paths: &ConfigPaths) -> Result<(), &'static str> {
    let parent = match existing_private_children(&paths.root, &["brain", "packs", "malecns-v1.0"]) {
        Ok(Some(parent)) => parent,
        Ok(None) => return Ok(()),
        Err(error) => return Err(pack_path_reason(&error)),
    };
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(_) => return Err("pack-unreadable"),
    };
    for entry in entries {
        let entry = entry.map_err(|_| "pack-unreadable")?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
        {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|_| "pack-unreadable")?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(entry.path()).map_err(|_| "pack-unwritable")?;
        }
    }
    Ok(())
}

pub(crate) async fn download<F>(
    paths: &ConfigPaths,
    resources: &Path,
    cancel: &CancellationToken,
    progress: F,
) -> Result<(), &'static str>
where
    F: Fn(u64, u64) + Send + Sync,
{
    if !DISTRIBUTION.distribution_url.starts_with("https://")
        || !DISTRIBUTION.archive_url.starts_with("https://")
    {
        return Err("insecure-url");
    }
    let artifact_bytes = fs::read(resources.join("connectome").join(ARTIFACT_NAME))
        .map_err(|_| "artifact-unreadable")?;
    let artifact: RateHelperArtifact =
        serde_json::from_slice(&artifact_bytes).map_err(|_| "artifact-invalid")?;
    artifact
        .validate_shape(None)
        .map_err(|_| "artifact-invalid")?;
    download_from(
        &client()?,
        &DISTRIBUTION,
        &artifact,
        paths,
        cancel,
        progress,
    )
    .await
}

async fn download_from<F>(
    client: &Client,
    spec: &DownloadSpec<'_>,
    artifact: &RateHelperArtifact,
    paths: &ConfigPaths,
    cancel: &CancellationToken,
    progress: F,
) -> Result<(), &'static str>
where
    F: Fn(u64, u64) + Send + Sync,
{
    let distribution = fetch_distribution(client, spec.distribution_url.as_ref(), cancel).await?;
    if distribution.schema != "habitua-rate-pack-distribution-v1"
        || distribution.archive != ARCHIVE_NAME
        || distribution.archive_sha256 != spec.archive_sha256.as_ref()
        || distribution.pack_manifest_sha256 != artifact.pack_manifest_sha256
    {
        return Err("distribution-mismatch");
    }

    let parent =
        private_children(&paths.root, &["brain", "packs", "malecns-v1.0"]).map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidInput {
                "pack-symlink"
            } else {
                "pack-unwritable"
            }
        })?;
    let temporary = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(&parent)
        .map_err(|_| "pack-unwritable")?;
    let archive_path = temporary.path().join("pack.tar");
    let mut output = tokio::fs::File::create(&archive_path)
        .await
        .map_err(|_| "pack-unwritable")?;
    let mut response = tokio::select! {
        _ = cancel.cancelled() => return Err("cancelled"),
        response = client.get(spec.archive_url.as_ref()).send() => response.map_err(|_| "network-error")?,
    }
    .error_for_status()
    .map_err(|_| "network-error")?;
    if response
        .content_length()
        .is_some_and(|size| size != spec.archive_bytes)
    {
        return Err("archive-size-mismatch");
    }
    let mut digest = Sha256::new();
    let mut received = 0_u64;
    progress(received, spec.archive_bytes);
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err("cancelled"),
            chunk = response.chunk() => chunk.map_err(|_| "network-error")?,
        };
        let Some(chunk) = chunk else { break };
        received = received
            .checked_add(chunk.len() as u64)
            .ok_or("archive-size-mismatch")?;
        if received > spec.archive_bytes {
            return Err("archive-size-mismatch");
        }
        output
            .write_all(&chunk)
            .await
            .map_err(|_| "pack-unwritable")?;
        digest.update(&chunk);
        progress(received, spec.archive_bytes);
        if cancel.is_cancelled() {
            return Err("cancelled");
        }
    }
    output.flush().await.map_err(|_| "pack-unwritable")?;
    drop(output);
    if received != spec.archive_bytes
        || format!("{:x}", digest.finalize()) != spec.archive_sha256.as_ref()
    {
        return Err("archive-sha256-mismatch");
    }
    let staging = temporary.path().join("rate-full");
    let expected_manifest = artifact.pack_manifest_sha256.clone();
    let cancel_for_unpack = cancel.clone();
    tokio::task::spawn_blocking(move || {
        unpack_and_verify(
            &archive_path,
            &staging,
            &expected_manifest,
            &cancel_for_unpack,
        )
    })
    .await
    .map_err(|_| "pack-invalid")??;
    if cancel.is_cancelled() {
        return Err("cancelled");
    }
    let destination = pack_directory(paths);
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Err("pack-symlink"),
        Ok(_) => return Err("pack-already-exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("pack-unwritable"),
    }
    rename_exclusive(&temporary.path().join("rate-full"), &destination).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            "pack-already-exists"
        } else {
            "pack-unwritable"
        }
    })?;
    Ok(())
}

fn rename_exclusive(source: &Path, destination: &Path) -> io::Result<()> {
    let source = CString::new(source.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let destination = CString::new(destination.as_os_str().as_bytes()).map_err(io::Error::other)?;
    let result =
        unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn unpack_and_verify(
    archive_path: &Path,
    staging: &Path,
    expected_manifest: &str,
    cancel: &CancellationToken,
) -> Result<(), &'static str> {
    fs::create_dir(staging).map_err(|_| "pack-unwritable")?;
    let mut archive = tar::Archive::new(File::open(archive_path).map_err(|_| "pack-unreadable")?);
    let mut seen = std::collections::BTreeSet::new();
    for entry in archive.entries().map_err(|_| "pack-invalid")? {
        if cancel.is_cancelled() {
            return Err("cancelled");
        }
        let mut entry = entry.map_err(|_| "pack-invalid")?;
        let path = entry.path().map_err(|_| "pack-invalid")?;
        if entry.header().entry_type().is_dir() && path == Path::new("rate-full") {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err("pack-invalid");
        }
        let mut components = path.components();
        let (Some(Component::Normal(root)), Some(Component::Normal(name)), None) =
            (components.next(), components.next(), components.next())
        else {
            return Err("pack-invalid");
        };
        if root != "rate-full" || !PACK_FILES.iter().any(|file| name == *file) {
            return Err("pack-invalid");
        }
        let name = name.to_string_lossy().into_owned();
        if !seen.insert(name.clone()) {
            return Err("pack-invalid");
        }
        let mut output = File::create(staging.join(name)).map_err(|_| "pack-unwritable")?;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            if cancel.is_cancelled() {
                return Err("cancelled");
            }
            let count = entry.read(&mut buffer).map_err(|_| "pack-invalid")?;
            if count == 0 {
                break;
            }
            output
                .write_all(&buffer[..count])
                .map_err(|_| "pack-unwritable")?;
        }
    }
    if seen.len() != PACK_FILES.len() {
        return Err("pack-invalid");
    }
    verify_pack(staging, expected_manifest)
}
