use super::{issue, validate_config, Config, ConfigError, ConfigPaths};
use crate::persistence::{
    atomic_write_bytes, cleanup_stale_temps, set_private_directory_mode, set_private_file_mode,
    SiblingLock,
};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::Path;

pub fn load_config(paths: &ConfigPaths) -> Result<Config, ConfigError> {
    let lock_path = paths.config.with_file_name(".config.lock");
    let _lock = SiblingLock::acquire(&lock_path)?;
    load_config_locked(paths)
}

fn load_config_locked(paths: &ConfigPaths) -> Result<Config, ConfigError> {
    let mut file = fs::File::open(&paths.config)?;
    set_private_file_mode(&file)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let raw: Value = serde_json::from_slice(&bytes)?;
    let config = super::parse_config(raw)?;
    validate_config(&config)?;
    Ok(config)
}

pub fn patch_config<F>(
    paths: &ConfigPaths,
    malformed_recovery_base: Option<&Config>,
    patch: F,
) -> Result<Config, ConfigError>
where
    F: FnOnce(Config) -> Result<Config, ConfigError>,
{
    patch_config_before_save(paths, malformed_recovery_base, patch, |_, _| Ok(()))
        .map(|(config, ())| config)
}

pub fn patch_config_before_save<F, B, T>(
    paths: &ConfigPaths,
    malformed_recovery_base: Option<&Config>,
    patch: F,
    before_save: B,
) -> Result<(Config, T), ConfigError>
where
    F: FnOnce(Config) -> Result<Config, ConfigError>,
    B: FnOnce(&Config, &Config) -> Result<T, ConfigError>,
{
    fs::create_dir_all(&paths.root)?;
    let lock_path = paths.config.with_file_name(".config.lock");
    let _lock = SiblingLock::acquire(&lock_path)?;
    let current = match load_config_locked(paths) {
        Ok(config) => config,
        Err(error @ ConfigError::Json(_)) => match malformed_recovery_base {
            Some(base) => base.clone(),
            None => return Err(error),
        },
        Err(error) => return Err(error),
    };
    let previous = current.clone();
    let mut updated = super::normalize_config(patch(current)?);
    super::normalize_audio_sources_on_enable(previous.audio.enabled, &mut updated);
    validate_config(&updated)?;
    validate_executable_overrides(&updated)?;
    let before_save_result = before_save(&previous, &updated)?;
    let bytes = serde_json::to_vec_pretty(&updated)?;
    atomic_write_bytes(&paths.config, &bytes).map_err(ConfigError::Io)?;
    Ok((updated, before_save_result))
}

pub fn save_config(paths: &ConfigPaths, config: &Config) -> Result<(), ConfigError> {
    let config = super::normalize_config(config.clone());
    validate_config(&config)?;
    validate_executable_overrides(&config)?;
    fs::create_dir_all(&paths.root)?;
    let bytes = serde_json::to_vec_pretty(&config)?;
    let lock_path = paths.config.with_file_name(".config.lock");
    let _lock = SiblingLock::acquire(&lock_path)?;
    atomic_write_bytes(&paths.config, &bytes).map_err(ConfigError::Io)
}

pub fn ensure_layout(paths: &ConfigPaths) -> Result<(), ConfigError> {
    let outbox_pending = paths.outbox.join("pending");
    let outbox_done = paths.outbox.join("done");
    let outbox_failed = paths.outbox.join("failed");
    for directory in [
        &paths.root,
        &paths.personas,
        &paths.mailbox,
        &paths.state,
        &paths.archive,
        &paths.outbox,
        &outbox_pending,
        &outbox_done,
        &outbox_failed,
        &paths.observations,
        &paths.transcripts,
        &paths.frame_buffer,
        &paths.conversation,
        &paths.attachments,
        &paths.memory,
        &paths.memory_daily,
        &paths.memory_weekly,
        &paths.memory_jobs,
        &paths.memory_failed,
        &paths.logs,
    ] {
        fs::create_dir_all(directory)?;
        set_private_directory_mode(directory)?;
        cleanup_stale_temps(directory)?;
    }
    crate::frame_buffer::FrameBuffer::new(paths.frame_buffer.clone())
        .cleanup_expired(chrono::Utc::now())?;
    crate::conversation_archive::reconcile_conversation_reset(paths)?;
    if !paths.config.exists() {
        save_config(paths, &Config::default())?;
    }
    Ok(())
}

fn validate_executable_overrides(config: &Config) -> Result<(), ConfigError> {
    let mut issues = Vec::new();
    for (path, value) in [
        (
            "watch.ocrGate.executable",
            &config.watch.ocr_gate.executable,
        ),
        ("observer.executable", &config.observer.executable),
        ("companion.executable", &config.companion.executable),
    ] {
        let Some(value) = value else { continue };
        if !is_executable(Path::new(value)) {
            issues.push(issue(path, "実行可能なファイルではありません。"));
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Validation(issues))
    }
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

