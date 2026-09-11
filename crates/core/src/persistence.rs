use chrono::{Duration, Local, NaiveDate};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{MutexGuard, TryLockError as MutexTryLockError};
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

/// 公開の開始とキャンセルの順序だけを直列化する。ファイル準備はこのゲートの外で行う。
#[derive(Debug, Clone)]
pub struct PublicationGate {
    cancellation: tokio_util::sync::CancellationToken,
    publication: std::sync::Arc<std::sync::Mutex<()>>,
}

impl PublicationGate {
    pub fn new(cancellation: tokio_util::sync::CancellationToken) -> Self {
        Self {
            cancellation,
            publication: Default::default(),
        }
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn publish_batch<T, E>(&self, operation: impl FnOnce() -> Result<T, E>) -> Result<T, E>
    where
        E: From<io::Error>,
    {
        self.publish_batch_with_directories(Vec::new(), |_| operation())
    }

    pub fn publish_batch_with_directories<T, E>(
        &self,
        directories: impl IntoIterator<Item = PathBuf>,
        operation: impl FnOnce(&DirectoryLocks) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<io::Error>,
    {
        let directories =
            DirectoryLocks::acquire(directories, Some(&self.cancellation)).map_err(E::from)?;
        let _guard = self.lock_publication().map_err(E::from)?;
        if self.cancellation.is_cancelled() {
            return Err(
                io::Error::new(io::ErrorKind::Interrupted, "画面記録が取り消されました").into(),
            );
        }
        operation(&directories)
    }

    fn lock_publication(&self) -> io::Result<MutexGuard<'_, ()>> {
        if self.cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "画面記録が取り消されました",
            ));
        }
        match self.publication.try_lock() {
            Ok(guard) => Ok(guard),
            Err(MutexTryLockError::Poisoned(_)) => {
                Err(io::Error::other("publication gate が壊れています"))
            }
            Err(MutexTryLockError::WouldBlock) => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "publication gate が使用中です",
            )),
        }
    }
}

#[derive(Debug)]
pub struct DirectoryLock {
    path: PathBuf,
    file: File,
}

impl DirectoryLock {
    fn acquire(
        path: &Path,
        cancellation: Option<&tokio_util::sync::CancellationToken>,
    ) -> io::Result<Self> {
        let file = File::open(path)?;
        lock_file(&file, cancellation)?;
        Ok(Self {
            path: path.to_owned(),
            file,
        })
    }

    fn matches(&self, path: &Path) -> bool {
        self.path == path
    }
}

#[derive(Debug)]
pub struct DirectoryLocks {
    locks: Vec<DirectoryLock>,
}

impl DirectoryLocks {
    pub fn acquire(
        paths: impl IntoIterator<Item = PathBuf>,
        cancellation: Option<&tokio_util::sync::CancellationToken>,
    ) -> io::Result<Self> {
        let mut paths = paths.into_iter().collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        let locks = paths
            .iter()
            .map(|path| DirectoryLock::acquire(path, cancellation))
            .collect::<io::Result<Vec<_>>>()?;
        Ok(Self { locks })
    }

    fn for_path(&self, path: &Path) -> io::Result<&DirectoryLock> {
        self.locks
            .iter()
            .find(|lock| lock.matches(path))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "ディレクトリロックがありません",
                )
            })
    }
}

fn lock_file(
    file: &File,
    cancellation: Option<&tokio_util::sync::CancellationToken>,
) -> io::Result<()> {
    let Some(cancellation) = cancellation else {
        file.lock()?;
        return Ok(());
    };
    if cancellation.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "画面記録が取り消されました",
        ));
    }
    match file.try_lock() {
        Ok(()) => Ok(()),
        Err(TryLockError::Error(error)) => Err(error),
        Err(TryLockError::WouldBlock) => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "ディレクトリロックが使用中です",
        )),
    }
}

/// 公開前の sibling 一時ファイル。復元はこの型自身では行わず、batch の所有者が行う。
#[derive(Debug)]
pub struct StagedFile {
    path: PathBuf,
    temporary: Option<PathBuf>,
    previous: Option<Vec<u8>>,
    published: bool,
    committed: bool,
    restored: bool,
    rollback_attempted: bool,
}

impl StagedFile {
    pub fn prepare(
        path: &Path,
        bytes: &[u8],
        cancellation: Option<&tokio_util::sync::CancellationToken>,
    ) -> io::Result<Self> {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "親ディレクトリがありません")
        })?;
        fs::create_dir_all(parent)?;
        set_private_directory_mode(parent)?;
        let directory_lock = DirectoryLock::acquire(parent, cancellation)?;
        let temporary = parent.join(format!(
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
                .open(&temporary)?;
            set_private_file_mode(&file)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            let previous = if path.exists() {
                Some(fs::read(path)?)
            } else {
                None
            };
            if cancellation.is_some_and(|value| value.is_cancelled()) {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "画面記録が取り消されました",
                ));
            }
            drop(directory_lock);
            Ok(Self {
                path: path.to_owned(),
                temporary: Some(temporary.clone()),
                previous,
                published: false,
                committed: false,
                restored: false,
                rollback_attempted: false,
            })
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn parent(&self) -> &Path {
        self.path.parent().expect("staged file parent")
    }

    pub fn publish(&mut self, directories: &DirectoryLocks) -> io::Result<()> {
        if self.committed || self.restored || self.rollback_attempted {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "確定済み、復元済み、または復元試行済みの staged file は公開できません",
            ));
        }
        let temporary = self.temporary.as_ref().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "一時ファイルがありません")
        })?;
        let directory = directories.for_path(self.parent())?;
        fs::rename(temporary, &self.path)?;
        self.temporary = None;
        self.published = true;
        #[cfg(test)]
        failpoints::after_rename(&self.path)?;
        directory.file.sync_all()?;
        set_private_path_mode(&self.path)?;
        Ok(())
    }

    pub fn restore(&mut self) -> io::Result<()> {
        if self.committed {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "commit 後の rollback はできません",
            ));
        }
        if self.restored {
            return Ok(());
        }
        if self.rollback_attempted {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "復元試行済みの staged file は再度 rollback できません",
            ));
        }
        self.rollback_attempted = true;
        if !self.published {
            self.remove_temporary()?;
            self.restored = true;
            return Ok(());
        }
        let parent = self.path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "親ディレクトリがありません")
        })?;
        let directories = DirectoryLocks::acquire(vec![parent.to_owned()], None)?;
        self.restore_locked_inner(&directories)
    }

    pub(crate) fn restore_locked(&mut self, directories: &DirectoryLocks) -> io::Result<()> {
        if self.committed {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "commit 後の rollback はできません",
            ));
        }
        if self.restored {
            return Ok(());
        }
        if self.rollback_attempted {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "復元試行済みの staged file は再度 rollback できません",
            ));
        }
        self.rollback_attempted = true;
        self.restore_locked_inner(directories)
    }

    fn restore_locked_inner(&mut self, directories: &DirectoryLocks) -> io::Result<()> {
        let directory = if self.published {
            let parent = self.path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "親ディレクトリがありません")
            })?;
            Some(directories.for_path(parent)?)
        } else {
            None
        };
        self.remove_temporary()?;
        if let Some(directory) = directory {
            match &self.previous {
                Some(bytes) => write_replacement_locked(&self.path, bytes, directory)?,
                None => match fs::remove_file(&self.path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                },
            }
            if self.previous.is_none() {
                directory.file.sync_all()?;
            }
        }
        self.published = false;
        self.restored = true;
        Ok(())
    }

    fn remove_temporary(&mut self) -> io::Result<()> {
        if let Some(temporary) = self.temporary.take() {
            match fs::remove_file(temporary) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn commit(&mut self) -> io::Result<()> {
        if self.committed {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "commit 済みの staged file は再度 commit できません",
            ));
        }
        if self.restored || self.rollback_attempted {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "rollback 後または復元試行後の commit はできません",
            ));
        }
        if !self.published {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "公開前の staged file は commit できません",
            ));
        }
        self.committed = true;
        Ok(())
    }

    pub fn validate_commit(&self) -> io::Result<()> {
        if self.committed {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "commit 済みの staged file は再度 commit できません",
            ));
        }
        if self.restored || self.rollback_attempted {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "rollback 後または復元試行後の commit はできません",
            ));
        }
        if !self.published {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "公開前の staged file は commit できません",
            ));
        }
        Ok(())
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        if let Some(temporary) = self.temporary.take() {
            let _ = fs::remove_file(temporary);
        }
    }
}

fn write_replacement_locked(
    path: &Path,
    bytes: &[u8],
    directory: &DirectoryLock,
) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "親ディレクトリがありません"))?;
    let temporary = parent.join(format!(
        ".{}.{}.rollback.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("snapshot"),
        Uuid::new_v4()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        set_private_file_mode(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        directory.file.sync_all()?;
        set_private_path_mode(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

/// 同一ディレクトリの一時ファイルを sync してから置き換える。
pub fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    atomic_write_bytes_cancellable(path, bytes, None)
}

pub fn atomic_write_bytes_cancellable(
    path: &Path,
    bytes: &[u8],
    publication: Option<&PublicationGate>,
) -> io::Result<()> {
    let mut staged = StagedFile::prepare(path, bytes, publication.map(|gate| &gate.cancellation))?;
    let mut publication_started = false;
    let publish = match publication {
        Some(gate) => {
            gate.publish_batch_with_directories(vec![staged.parent().to_owned()], |directories| {
                publication_started = true;
                publish_and_commit_staged(&mut staged, directories).or_else(|error| {
                    match staged.restore_locked(directories) {
                        Ok(()) => Err(error),
                        Err(rollback) => Err(io::Error::other(format!(
                            "{error}; ロールバックにも失敗しました: {rollback}"
                        ))),
                    }
                })
            })
        }
        None => {
            let directories = DirectoryLocks::acquire(vec![staged.parent().to_owned()], None)?;
            publication_started = true;
            publish_and_commit_staged(&mut staged, &directories).or_else(|error| {
                match staged.restore_locked(&directories) {
                    Ok(()) => Err(error),
                    Err(rollback) => Err(io::Error::other(format!(
                        "{error}; ロールバックにも失敗しました: {rollback}"
                    ))),
                }
            })
        }
    };
    if let Err(error) = publish {
        if !publication_started {
            if let Err(rollback) = staged.restore() {
                return Err(io::Error::other(format!(
                    "{error}; staging の後始末にも失敗しました: {rollback}"
                )));
            }
        }
        return Err(error);
    }
    Ok(())
}

fn publish_and_commit_staged(
    staged: &mut StagedFile,
    directories: &DirectoryLocks,
) -> io::Result<()> {
    staged.publish(directories)?;
    staged.validate_commit()?;
    staged.commit()
}

/// 前回の異常終了で残った、本実装が作成した sibling temp を起動時に片付ける。
pub fn cleanup_stale_temps(directory: &Path) -> io::Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    let directory_lock = File::open(directory)?;
    match directory_lock.try_lock() {
        Ok(()) => {}
        // 保存途中のtempを異常終了の残骸と区別できないため、保存中の掃除は見送る。
        Err(TryLockError::WouldBlock) => return Ok(()),
        Err(TryLockError::Error(error)) => return Err(error),
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

pub fn atomic_write_json_cancellable<T: Serialize>(
    path: &Path,
    value: &T,
    publication: Option<&PublicationGate>,
) -> Result<(), PersistenceError> {
    let bytes = serde_json::to_vec_pretty(value)?;
    atomic_write_bytes_cancellable(path, &bytes, publication)?;
    Ok(())
}

#[derive(Debug)]
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

