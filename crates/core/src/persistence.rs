use chrono::{Duration, Local, NaiveDate};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;
use thiserror::Error;
use uuid::Uuid;

const LOCK_RETRY_TIMEOUT: StdDuration = StdDuration::from_secs(2);
const LOCK_RETRY_INTERVAL: StdDuration = StdDuration::from_millis(20);

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("永続化 I/O に失敗しました: {0}")]
    Io(#[from] io::Error),
    #[error("データを JSON 化できません: {0}")]
    Json(#[from] serde_json::Error),
    #[error("永続化データの形式が不正です: {0}")]
    Invalid(String),
    #[error("別の watch プロセスが実行中です")]
    AlreadyLocked,
}

/// 同一ディレクトリの一時ファイルを sync してから置き換える。
pub fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "親ディレクトリがありません"))?;
    fs::create_dir_all(parent)?;
    set_private_directory_mode(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("snapshot"),
        Uuid::new_v4()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        set_private_file_mode(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        File::open(parent)?.sync_all()?;
        set_private_path_mode(path)?;
        Ok::<(), io::Error>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// 前回の異常終了で残った、本実装が作成した sibling temp を起動時に片付ける。
pub fn cleanup_stale_temps(directory: &Path) -> io::Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if name.starts_with('.') && name.ends_with(".tmp") && entry.file_type()?.is_file() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), PersistenceError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    atomic_write_bytes(path, &bytes)?;
    Ok(())
}

pub struct SiblingLock {
    file: File,
}

impl SiblingLock {
    pub fn acquire(path: &Path) -> Result<Self, PersistenceError> {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            if matches!(
                handle.runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            ) {
                return tokio::task::block_in_place(|| handle.block_on(Self::acquire_async(path)));
            }
            return Self::acquire_nowait(path);
        }
        Self::open_and_lock_blocking(path)
    }

    pub fn acquire_nowait(path: &Path) -> Result<Self, PersistenceError> {
        let file = open_lock_file(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file }),
            Err(TryLockError::WouldBlock) => Err(PersistenceError::AlreadyLocked),
            Err(TryLockError::Error(error)) => Err(PersistenceError::Io(error)),
        }
    }

    pub async fn acquire_async(path: &Path) -> Result<Self, PersistenceError> {
        let path = path.to_owned();
        let file = tokio::task::spawn_blocking(move || open_lock_file(&path))
            .await
            .map_err(|error| PersistenceError::Io(io::Error::other(error.to_string())))??;
        let deadline = tokio::time::Instant::now() + LOCK_RETRY_TIMEOUT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(LOCK_RETRY_INTERVAL).await;
                }
                Err(TryLockError::WouldBlock) => return Err(PersistenceError::AlreadyLocked),
                Err(TryLockError::Error(error)) => return Err(PersistenceError::Io(error)),
            }
        }
    }

    fn open_and_lock_blocking(path: &Path) -> Result<Self, PersistenceError> {
        let file = open_lock_file(path)?;
        file.lock()?;
        Ok(Self { file })
    }
}

fn open_lock_file(path: &Path) -> Result<File, PersistenceError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        set_private_directory_mode(parent)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    set_private_file_mode(&file)?;
    Ok(file)
}

impl Drop for SiblingLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub struct JsonlStore {
    path: PathBuf,
    lock_path: PathBuf,
}

impl JsonlStore {
    pub fn new(path: PathBuf) -> Self {
        let lock_path = jsonl_lock_path(&path);
        Self { path, lock_path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append<T: Serialize>(&self, value: &T) -> Result<(), PersistenceError> {
        let bytes = serde_json::to_vec(value)?;
        let parent = self.path.parent().ok_or_else(|| {
            PersistenceError::Invalid("JSONL の親ディレクトリがありません".to_owned())
        })?;
        let _directory_lock = SiblingLock::acquire(&parent.join(".retention.lock"))?;
        let _lock = SiblingLock::acquire(&self.lock_path)?;
        self.append_bytes(&bytes)?;
        Ok(())
    }

    pub fn append_unique<T, F>(&self, value: &T, same_identity: F) -> Result<bool, PersistenceError>
    where
        T: Serialize + DeserializeOwned + PartialEq,
        F: Fn(&T) -> bool,
    {
        let bytes = serde_json::to_vec(value)?;
        let parent = self.path.parent().ok_or_else(|| {
            PersistenceError::Invalid("JSONL の親ディレクトリがありません".to_owned())
        })?;
        let _directory_lock = SiblingLock::acquire(&parent.join(".retention.lock"))?;
        let _lock = SiblingLock::acquire(&self.lock_path)?;
        if self.path.exists() {
            let file = File::open(&self.path)?;
            set_private_file_mode(&file)?;
            for line in BufReader::new(file).lines() {
                let line = line?;
                let Ok(existing) = serde_json::from_str::<T>(&line) else {
                    continue;
                };
                if same_identity(&existing) {
                    if existing == *value {
                        return Ok(false);
                    }
                    return Err(PersistenceError::Invalid(
                        "同じ ID の JSONL record が異なる内容を持ちます".to_owned(),
                    ));
                }
            }
        }
        self.append_bytes(&bytes)?;
        Ok(true)
    }

    pub(crate) fn append_idempotent<T, F>(
        &self,
        value: &T,
        same_identity: F,
    ) -> Result<bool, PersistenceError>
    where
        T: Serialize + DeserializeOwned,
        F: Fn(&T) -> bool,
    {
        let bytes = serde_json::to_vec(value)?;
        let parent = self.path.parent().ok_or_else(|| {
            PersistenceError::Invalid("JSONL の親ディレクトリがありません".to_owned())
        })?;
        let _directory_lock = SiblingLock::acquire(&parent.join(".retention.lock"))?;
        let _lock = SiblingLock::acquire(&self.lock_path)?;
        if self.path.exists() {
            let file = File::open(&self.path)?;
            set_private_file_mode(&file)?;
            for line in BufReader::new(file).lines() {
                let line = line?;
                if serde_json::from_str::<T>(&line).is_ok_and(|existing| same_identity(&existing)) {
                    return Ok(false);
                }
            }
        }
        self.append_bytes(&bytes)?;
        Ok(true)
    }

    pub fn read<T: DeserializeOwned>(&self) -> Result<Vec<T>, PersistenceError> {
        let _lock = SiblingLock::acquire(&self.lock_path)?;
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path)?;
        set_private_file_mode(&file)?;
        let reader = BufReader::new(file);
        let mut records = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(record) = serde_json::from_str(&line) {
                records.push(record);
            }
        }
        Ok(records)
    }

    pub(crate) fn rewrite<T, F>(&self, update: F) -> Result<bool, PersistenceError>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce(&mut Vec<T>) -> bool,
    {
        let parent = self.path.parent().ok_or_else(|| {
            PersistenceError::Invalid("JSONL の親ディレクトリがありません".to_owned())
        })?;
        let _directory_lock = SiblingLock::acquire(&parent.join(".retention.lock"))?;
        let _lock = SiblingLock::acquire(&self.lock_path)?;
        if !self.path.exists() {
            return Ok(false);
        }
        let file = File::open(&self.path)?;
        set_private_file_mode(&file)?;
        let mut records = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            records.push(serde_json::from_str(&line)?);
        }
        if !update(&mut records) {
            return Ok(false);
        }
        let mut bytes = Vec::new();
        for record in records {
            serde_json::to_writer(&mut bytes, &record)?;
            bytes.push(b'\n');
        }
        atomic_write_bytes(&self.path, &bytes)?;
        Ok(true)
    }

    fn append_bytes(&self, bytes: &[u8]) -> Result<(), PersistenceError> {
        let parent = self.path.parent().ok_or_else(|| {
            PersistenceError::Invalid("JSONL の親ディレクトリがありません".to_owned())
        })?;
        fs::create_dir_all(parent)?;
        let is_new_file = !self.path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        set_private_file_mode(&file)?;
        file.write_all(bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        if is_new_file {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}

fn jsonl_lock_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        ".{}.lock",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("records")
    ))
}

pub struct WatchLock {
    lock: SiblingLock,
}

impl WatchLock {
    pub fn acquire(path: &Path) -> Result<Self, PersistenceError> {
        Ok(Self {
            lock: SiblingLock::acquire_nowait(path)?,
        })
    }

    pub fn is_held(&self) -> bool {
        let _ = &self.lock;
        true
    }
}

pub fn prune_daily_jsonl(
    directory: &Path,
    retention_days: u64,
    max_bytes: u64,
) -> Result<(), PersistenceError> {
    prune_daily_jsonl_at(directory, retention_days, max_bytes, chrono::Utc::now())
}

pub fn prune_daily_jsonl_at(
    directory: &Path,
    retention_days: u64,
    max_bytes: u64,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), PersistenceError> {
    if !directory.exists() {
        return Ok(());
    }
    let _directory_lock = SiblingLock::acquire(&directory.join(".retention.lock"))?;
    let oldest = retention_cutoff_date(now, retention_days);
    for (date, path) in enumerate_daily_files(directory)? {
        if date < oldest {
            let _lock = SiblingLock::acquire(&jsonl_lock_path(&path))?;
            if path.exists() {
                fs::remove_file(path)?;
            }
        }
    }
    let mut files = enumerate_daily_files(directory)?
        .into_iter()
        .map(|(date, path)| {
            let lock = SiblingLock::acquire(&jsonl_lock_path(&path))?;
            let size = fs::metadata(&path)?.len();
            Ok((date, path, size, lock))
        })
        .collect::<Result<Vec<_>, PersistenceError>>()?;
    files.sort_by_key(|(date, _, _, _)| *date);
    let mut total: u64 = files.iter().map(|(_, _, size, _)| *size).sum();
    for (_, path, size, _lock) in files {
        if total <= max_bytes {
            break;
        }
        fs::remove_file(&path)?;
        total = total.saturating_sub(size);
    }
    Ok(())
}

pub fn retention_cutoff_date(
    now: chrono::DateTime<chrono::Utc>,
    retention_days: u64,
) -> chrono::NaiveDate {
    now.with_timezone(&Local).date_naive() - Duration::days(retention_days as i64)
}

fn enumerate_daily_files(directory: &Path) -> Result<Vec<(NaiveDate, PathBuf)>, PersistenceError> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl")
            || !entry.file_type()?.is_file()
        {
            continue;
        }
        let Some(date) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| NaiveDate::parse_from_str(stem, "%Y-%m-%d").ok())
        else {
            continue;
        };
        files.push((date, path));
    }
    Ok(files)
}

pub(crate) fn set_private_file_mode(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub(crate) fn set_private_directory_mode(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_private_path_mode(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

