use coosenpai_core::config::{
    validate_config, BundledConnectome, Config, ConfigPaths, JudgeComposition, JudgeConfig,
    JudgeModuleConfig,
};
use habitua_connectome::{RateHelperArtifact, RatePackManifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub(crate) const ARTIFACT_NAME: &str = "malecns-frozen-response-learning-artifact-r14.json";
const HELPER_NAME: &str = "coosenpai-connectome";
const VERIFICATION_STAMP: &str = ".rate-full-verified.json";

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct FileStamp {
    size: u64,
    modified_ns: u128,
}

#[derive(Serialize, Deserialize)]
struct VerificationStamp {
    manifest_sha256: String,
    files: BTreeMap<String, FileStamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectomeStatus {
    pub state: String,
    pub reason: Option<String>,
    pub pack_path: String,
    pub manifest_sha256: Option<String>,
}

pub(crate) fn pack_directory(paths: &ConfigPaths) -> PathBuf {
    paths.root.join("brain/packs/malecns-v1.0/rate-full")
}

pub(crate) fn private_children(base: &Path, components: &[&str]) -> std::io::Result<PathBuf> {
    fs::create_dir_all(base)?;
    ensure_real_directory(base)?;
    let mut directory = base.to_path_buf();
    for component in components {
        directory.push(component);
        match fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        ensure_real_directory(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(directory)
}

fn ensure_real_directory(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "pack-symlink",
        ));
    }
    if !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "pack path is not a directory",
        ));
    }
    Ok(())
}

pub(crate) fn existing_private_children(
    base: &Path,
    components: &[&str],
) -> std::io::Result<Option<PathBuf>> {
    let mut directory = base.to_path_buf();
    for component in std::iter::once(&"").chain(components.iter()) {
        if !component.is_empty() {
            directory.push(component);
        }
        match ensure_real_directory(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    Ok(Some(directory))
}

pub(crate) fn pack_path_reason(error: &std::io::Error) -> &'static str {
    if error.kind() == std::io::ErrorKind::InvalidInput {
        "pack-symlink"
    } else {
        "pack-unreadable"
    }
}

pub(crate) fn prepare_pack_directory(paths: &ConfigPaths) -> std::io::Result<PathBuf> {
    private_children(&paths.root, &["brain", "packs", "malecns-v1.0"])
}

fn status(
    state: &'static str,
    reason: Option<&'static str>,
    pack: &Path,
    manifest_sha256: Option<String>,
) -> ConnectomeStatus {
    ConnectomeStatus {
        state: state.to_owned(),
        reason: reason.map(str::to_owned),
        pack_path: pack.to_string_lossy().into_owned(),
        manifest_sha256,
    }
}

fn file_metadata(path: &Path) -> Result<fs::Metadata, &'static str> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "pack-missing"
        } else {
            "pack-unreadable"
        }
    })?;
    if metadata.file_type().is_symlink() {
        return Err("pack-symlink");
    }
    if !metadata.is_file() {
        return Err("pack-invalid");
    }
    Ok(metadata)
}

fn file_stamp(path: &Path) -> Result<FileStamp, &'static str> {
    let metadata = file_metadata(path)?;
    let modified_ns = metadata
        .modified()
        .map_err(|_| "pack-unreadable")?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "pack-unreadable")?
        .as_nanos();
    Ok(FileStamp {
        size: metadata.len(),
        modified_ns,
    })
}

pub(crate) fn stamp_path(pack: &Path) -> PathBuf {
    pack.join(VERIFICATION_STAMP)
}

pub(crate) fn verify_pack(pack: &Path, expected_manifest: &str) -> Result<(), &'static str> {
    verify_pack_with_stamp(pack, expected_manifest, true)
}

fn verify_pack_with_stamp(
    pack: &Path,
    expected_manifest: &str,
    reuse_stamp: bool,
) -> Result<(), &'static str> {
    let manifest_path = pack.join("rate_manifest.json");
    let manifest_stamp = file_stamp(&manifest_path)?;
    let manifest_bytes = fs::read(&manifest_path).map_err(|_| "pack-unreadable")?;
    let actual_manifest = format!("{:x}", Sha256::digest(&manifest_bytes));
    if actual_manifest != expected_manifest {
        return Err("pack-manifest-mismatch");
    }
    let manifest: RatePackManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|_| "pack-invalid")?;
    let files = [
        ("rate-neurons.json", manifest.neurons_sha256.as_str()),
        ("rate-out.bin", manifest.outgoing_sha256.as_str()),
        ("rate-in.bin", manifest.incoming_sha256.as_str()),
        (
            "rate-populations.json",
            manifest.populations_sha256.as_str(),
        ),
    ];
    let mut current = BTreeMap::from([("rate_manifest.json".to_owned(), manifest_stamp)]);
    for (name, _) in files {
        current.insert(name.to_owned(), file_stamp(&pack.join(name))?);
    }
    let stamp_file = stamp_path(pack);
    match fs::symlink_metadata(&stamp_file) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Err("pack-symlink"),
        Ok(metadata) if !metadata.is_file() => return Err("pack-invalid"),
        Ok(_) => {
            if let Ok(bytes) = fs::read(&stamp_file) {
                if let Ok(stamp) = serde_json::from_slice::<VerificationStamp>(&bytes) {
                    if reuse_stamp
                        && stamp.manifest_sha256 == actual_manifest
                        && stamp.files == current
                    {
                        return Ok(());
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("pack-unreadable"),
    }
    for (name, expected_hash) in files {
        let file = fs::File::open(pack.join(name)).map_err(|_| "pack-unreadable")?;
        let mut reader = BufReader::new(file);
        let mut digest = Sha256::new();
        let mut size = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = reader.read(&mut buffer).map_err(|_| "pack-unreadable")?;
            if count == 0 {
                break;
            }
            size += count as u64;
            digest.update(&buffer[..count]);
        }
        if size != current[name].size || format!("{:x}", digest.finalize()) != expected_hash {
            return Err("pack-file-mismatch");
        }
        if file_stamp(&pack.join(name))? != current[name] {
            return Err("pack-file-changed");
        }
    }
    if file_stamp(&manifest_path)? != current["rate_manifest.json"] {
        return Err("pack-file-changed");
    }
    let stamp = VerificationStamp {
        manifest_sha256: actual_manifest,
        files: current,
    };
    let mut temporary = tempfile::NamedTempFile::new_in(pack).map_err(|_| "pack-unwritable")?;
    serde_json::to_writer(&mut temporary, &stamp).map_err(|_| "pack-unwritable")?;
    temporary
        .persist(&stamp_file)
        .map_err(|_| "pack-unwritable")?;
    Ok(())
}

fn executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn module_for_pack(
    helper: &Path,
    artifact: &Path,
    pack: &Path,
    state_dir: &Path,
) -> JudgeModuleConfig {
    let mut environment = BTreeMap::new();
    environment.insert(
        "HABITUA_RATE_STATE".to_owned(),
        state_dir
            .join("learning-state.json")
            .to_string_lossy()
            .into_owned(),
    );
    JudgeModuleConfig {
        executable: helper.to_string_lossy().into_owned(),
        arguments: vec![
            "rate".to_owned(),
            "--artifact".to_owned(),
            artifact.to_string_lossy().into_owned(),
            "--pack".to_owned(),
            pack.to_string_lossy().into_owned(),
        ],
        environment,
        weight: 1.0,
    }
}

pub(crate) fn resolve(
    config: &Config,
    paths: &ConfigPaths,
    resources: Option<&Path>,
    executable_dir: &Path,
) -> (JudgeConfig, ConnectomeStatus) {
    resolve_with_stamp(config, paths, resources, executable_dir, true)
}

pub(crate) fn resolve_force(
    config: &Config,
    paths: &ConfigPaths,
    resources: Option<&Path>,
    executable_dir: &Path,
) -> (JudgeConfig, ConnectomeStatus) {
    resolve_with_stamp(config, paths, resources, executable_dir, false)
}

fn resolve_with_stamp(
    config: &Config,
    paths: &ConfigPaths,
    resources: Option<&Path>,
    executable_dir: &Path,
    reuse_stamp: bool,
) -> (JudgeConfig, ConnectomeStatus) {
    let mut judge = config.judge.clone();
    let pack = pack_directory(paths);
    if !judge.modules.is_empty() {
        return (judge, status("manual", None, &pack, None));
    }
    if judge.bundled_connectome == BundledConnectome::Off {
        return (judge, status("off", None, &pack, None));
    }

    let Some(artifact_path) = resources.map(|root| root.join("connectome").join(ARTIFACT_NAME))
    else {
        return (
            judge,
            status("unavailable", Some("artifact-missing"), &pack, None),
        );
    };
    let artifact_bytes = match fs::read(&artifact_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let reason = if error.kind() == std::io::ErrorKind::NotFound {
                "artifact-missing"
            } else {
                "artifact-unreadable"
            };
            return (judge, status("unavailable", Some(reason), &pack, None));
        }
    };
    let artifact: RateHelperArtifact = match serde_json::from_slice(&artifact_bytes) {
        Ok(artifact) => artifact,
        Err(_) => {
            return (
                judge,
                status("unavailable", Some("artifact-invalid"), &pack, None),
            )
        }
    };
    let expected = Some(artifact.pack_manifest_sha256.clone());
    if artifact.validate_shape(None).is_err() {
        return (
            judge,
            status("unavailable", Some("artifact-invalid"), &pack, expected),
        );
    }
    let helper = executable_dir.join(HELPER_NAME);
    if !executable(&helper) {
        return (
            judge,
            status("unavailable", Some("helper-missing"), &pack, expected),
        );
    }
    match existing_private_children(
        &paths.root,
        &["brain", "packs", "malecns-v1.0", "rate-full"],
    ) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return (
                judge,
                status("unavailable", Some("pack-missing"), &pack, expected),
            )
        }
        Err(error) => {
            return (
                judge,
                status(
                    "unavailable",
                    Some(pack_path_reason(&error)),
                    &pack,
                    expected,
                ),
            );
        }
    }
    if let Err(reason) = verify_pack_with_stamp(&pack, &artifact.pack_manifest_sha256, reuse_stamp)
    {
        let reason = if reason == "pack-missing" {
            "pack-invalid"
        } else {
            reason
        };
        return (judge, status("unavailable", Some(reason), &pack, expected));
    }
    let state_dir = match private_children(&paths.state, &["brain", "connectome"]) {
        Ok(path) => path,
        Err(_) => {
            return (
                judge,
                status("unavailable", Some("state-unavailable"), &pack, expected),
            )
        }
    };
    judge
        .modules
        .push(module_for_pack(&helper, &artifact_path, &pack, &state_dir));
    // 同梱は単独の判断役なので、保存された複数モジュール用の合成方式を適用しない。
    judge.composition = JudgeComposition::Single;
    let mut resolved_config = config.clone();
    resolved_config.judge = judge.clone();
    if let Err(error) = validate_config(&resolved_config) {
        let mut unavailable = status("unavailable", None, &pack, expected);
        unavailable.reason = Some(format!("judge-config-invalid: {}", error.format_for_user()));
        return (config.judge.clone(), unavailable);
    }
    (judge, status("ready", None, &pack, expected))
}
