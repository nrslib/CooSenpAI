use crate::persistence::{retention_cutoff_date, PersistenceError, SiblingLock};
use chrono::{DateTime, Local, NaiveDate, Utc};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const CODEX_PROVIDER: &str = "codex";
const CLAUDE_PROVIDER: &str = "claude";
const OPENCODE_PROVIDER: &str = "opencode";
const RECORD_HEADER_MAX_BYTES: u64 = 64 * 1024;
const PROVIDER_ACTIVE_WRITE_GRACE_SECS: i64 = 5 * 60;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderRetentionStats {
    pub deleted_files: u64,
    pub deleted_bytes: u64,
}

impl ProviderRetentionStats {
    fn add_file(&mut self, bytes: u64) {
        self.deleted_files = self.deleted_files.saturating_add(1);
        self.deleted_bytes = self.deleted_bytes.saturating_add(bytes);
    }
}

impl std::ops::AddAssign for ProviderRetentionStats {
    fn add_assign(&mut self, other: Self) {
        self.deleted_files = self.deleted_files.saturating_add(other.deleted_files);
        self.deleted_bytes = self.deleted_bytes.saturating_add(other.deleted_bytes);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderRetentionReport {
    pub codex: ProviderRetentionStats,
    pub claude: ProviderRetentionStats,
    pub opencode: ProviderRetentionStats,
}

impl ProviderRetentionReport {
    pub fn by_provider(&self) -> [(&'static str, ProviderRetentionStats); 3] {
        [
            (CODEX_PROVIDER, self.codex),
            (CLAUDE_PROVIDER, self.claude),
            (OPENCODE_PROVIDER, self.opencode),
        ]
    }
}

pub fn prune_provider_storage(
    provider_directory: &Path,
    retention_days: u64,
    now: DateTime<Utc>,
) -> Result<ProviderRetentionReport, PersistenceError> {
    prune_provider_storage_with_hook(provider_directory, retention_days, now, &|_| {})
}

fn prune_provider_storage_with_hook(
    provider_directory: &Path,
    retention_days: u64,
    now: DateTime<Utc>,
    before_codex_unlink: &dyn Fn(&Path),
) -> Result<ProviderRetentionReport, PersistenceError> {
    if !is_real_directory(provider_directory)? {
        return Ok(ProviderRetentionReport::default());
    }
    let _directory_lock = SiblingLock::acquire(&provider_directory.join(".retention.lock"))?;
    let cutoff = retention_cutoff_date(now, retention_days);
    Ok(ProviderRetentionReport {
        codex: prune_codex(
            &provider_directory.join(CODEX_PROVIDER),
            cutoff,
            before_codex_unlink,
        )?,
        claude: prune_claude(&provider_directory.join(CLAUDE_PROVIDER), cutoff, now)?,
        opencode: prune_opencode(&provider_directory.join(OPENCODE_PROVIDER), cutoff, now)?,
    })
}

fn prune_codex(
    provider_directory: &Path,
    cutoff: NaiveDate,
    before_unlink: &dyn Fn(&Path),
) -> Result<ProviderRetentionStats, PersistenceError> {
    let sessions = provider_directory.join("sessions");
    if !is_real_directory(&sessions)? {
        return Ok(ProviderRetentionStats::default());
    }
    let coordination_lock = provider_directory.join("thread-writer-locks/.coordination.lock");
    let Some(_coordination_lock) = try_acquire_lock(&coordination_lock)? else {
        return Ok(ProviderRetentionStats::default());
    };
    let mut stats = ProviderRetentionStats::default();
    for path in collect_files(&sessions)? {
        if !is_codex_rollout(&path) || !is_older_than(&path, &sessions, cutoff)? {
            continue;
        }
        let Some(_thread_lock) = try_acquire_codex_thread_lock(provider_directory, &path)? else {
            continue;
        };
        before_unlink(&path);
        if let Some(bytes) = remove_file(&path)? {
            stats.add_file(bytes);
        }
    }
    Ok(stats)
}

fn prune_claude(
    provider_directory: &Path,
    cutoff: NaiveDate,
    now: DateTime<Utc>,
) -> Result<ProviderRetentionStats, PersistenceError> {
    let mut stats = ProviderRetentionStats::default();
    for root in [
        provider_directory.join("projects"),
        provider_directory.join("sessions"),
    ] {
        stats += prune_files_by_extension(&root, "jsonl", cutoff, now)?;
    }
    Ok(stats)
}

fn prune_opencode(
    provider_directory: &Path,
    cutoff: NaiveDate,
    now: DateTime<Utc>,
) -> Result<ProviderRetentionStats, PersistenceError> {
    let mut stats = ProviderRetentionStats::default();
    for root in [
        provider_directory.join("sessions"),
        provider_directory.join("storage/session"),
    ] {
        stats += prune_files_by_extensions(&root, &["json", "jsonl"], cutoff, now)?;
    }
    Ok(stats)
}

fn prune_files_by_extension(
    root: &Path,
    extension: &str,
    cutoff: NaiveDate,
    now: DateTime<Utc>,
) -> Result<ProviderRetentionStats, PersistenceError> {
    prune_files_by_extensions(root, &[extension], cutoff, now)
}

fn prune_files_by_extensions(
    root: &Path,
    extensions: &[&str],
    cutoff: NaiveDate,
    now: DateTime<Utc>,
) -> Result<ProviderRetentionStats, PersistenceError> {
    if !is_real_directory(root)? {
        return Ok(ProviderRetentionStats::default());
    }
    let mut stats = ProviderRetentionStats::default();
    for path in collect_files(root)? {
        if !extensions.contains(
            &path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default(),
        ) || !is_older_and_inactive(&path, root, cutoff, now)?
        {
            continue;
        }
        if let Some(bytes) = remove_file(&path)? {
            stats.add_file(bytes);
        }
    }
    Ok(stats)
}

fn is_real_directory(path: &Path) -> Result<bool, PersistenceError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn collect_files(root: &Path) -> Result<Vec<PathBuf>, PersistenceError> {
    let mut files = Vec::new();
    collect_files_into(root, &mut files)?;
    Ok(files)
}

fn collect_files_into(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), PersistenceError> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_files_into(&path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn is_codex_rollout(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("jsonl")
        && path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.starts_with("rollout-"))
}

fn is_older_than(path: &Path, root: &Path, cutoff: NaiveDate) -> Result<bool, PersistenceError> {
    if let Some(date) = date_from_path(path, root).or_else(|| date_from_file_name(path)) {
        return Ok(date < cutoff);
    }
    let Some(modified) = modified_time(path)? else {
        return Ok(false);
    };
    let modified_date = DateTime::<Utc>::from(modified)
        .with_timezone(&Local)
        .date_naive();
    if let Some(date) = date_from_record(path)? {
        return Ok(date < cutoff && modified_date < cutoff);
    }
    Ok(modified_date < cutoff)
}

fn is_older_and_inactive(
    path: &Path,
    root: &Path,
    cutoff: NaiveDate,
    now: DateTime<Utc>,
) -> Result<bool, PersistenceError> {
    if !is_older_than(path, root, cutoff)? {
        return Ok(false);
    }
    let Some(modified) = modified_time(path)? else {
        return Ok(false);
    };
    let modified = DateTime::<Utc>::from(modified);
    Ok(now.signed_duration_since(modified)
        >= chrono::Duration::seconds(PROVIDER_ACTIVE_WRITE_GRACE_SECS))
}

fn modified_time(path: &Path) -> Result<Option<std::time::SystemTime>, PersistenceError> {
    match fs::metadata(path) {
        Ok(metadata) => match metadata.modified() {
            Ok(modified) => Ok(Some(modified)),
            Err(_) => Ok(None),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn date_from_record(path: &Path) -> Result<Option<NaiveDate>, PersistenceError> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(RECORD_HEADER_MAX_BYTES)
        .read_to_end(&mut bytes)?;
    let Some(line) = bytes
        .split(|byte| *byte == b'\n')
        .find(|line| !line.iter().all(u8::is_ascii_whitespace))
    else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
        return Ok(None);
    };
    Ok(record_timestamp(&value).map(|timestamp| timestamp.with_timezone(&Local).date_naive()))
}

fn record_timestamp(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    for key in ["timestamp", "createdAt", "created_at"] {
        if let Some(timestamp) = value.get(key).and_then(serde_json::Value::as_str) {
            if let Ok(timestamp) = DateTime::parse_from_rfc3339(timestamp) {
                return Some(timestamp.with_timezone(&Utc));
            }
        }
    }
    for key in ["updated", "created"] {
        let timestamp = value
            .get("time")
            .and_then(serde_json::Value::as_object)
            .and_then(|time| time.get(key))
            .and_then(serde_json::Value::as_i64)
            .and_then(DateTime::<Utc>::from_timestamp_millis);
        if timestamp.is_some() {
            return timestamp;
        }
    }
    None
}

fn date_from_path(path: &Path, root: &Path) -> Option<NaiveDate> {
    let relative = path.strip_prefix(root).ok()?;
    let components = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    components.windows(3).find_map(|parts| {
        let year = parts[0].parse::<i32>().ok()?;
        let month = parts[1].parse::<u32>().ok()?;
        let day = parts[2].parse::<u32>().ok()?;
        NaiveDate::from_ymd_opt(year, month, day)
    })
}

fn date_from_file_name(path: &Path) -> Option<NaiveDate> {
    let name = path.file_name()?.to_str()?;
    let date = name.strip_prefix("rollout-").unwrap_or(name).get(..10)?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

fn try_acquire_codex_thread_lock(
    provider_directory: &Path,
    path: &Path,
) -> Result<Option<SiblingLock>, PersistenceError> {
    let Some(thread_id) = rollout_thread_id(path) else {
        return Ok(None);
    };
    let lock_path = provider_directory
        .join("thread-writer-locks")
        .join(format!("{thread_id}.lock"));
    try_acquire_lock(&lock_path)
}

fn rollout_thread_id(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    let max_start = stem.len().saturating_sub(36);
    for (start, _) in stem.char_indices().rev() {
        if start > max_start {
            continue;
        }
        let candidate = stem.get(start..start + 36)?;
        if Uuid::parse_str(candidate).is_ok() {
            return Some(candidate.to_owned());
        }
    }
    None
}

fn try_acquire_lock(path: &Path) -> Result<Option<SiblingLock>, PersistenceError> {
    match SiblingLock::acquire_nowait(path) {
        Ok(lock) => Ok(Some(lock)),
        Err(PersistenceError::AlreadyLocked) => Ok(None),
        Err(error) => Err(error),
    }
}

fn remove_file(path: &Path) -> Result<Option<u64>, PersistenceError> {
    let size = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    match fs::remove_file(path) {
        Ok(()) => Ok(Some(size)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

