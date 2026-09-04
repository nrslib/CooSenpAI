use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use uuid::Uuid;

const RETENTION: Duration = Duration::minutes(10);
const FILE_PREFIX: &str = "frame-";
const FILE_SUFFIX: &str = ".png";

#[derive(Debug, Clone)]
pub struct FrameBuffer {
    directory: PathBuf,
}

impl FrameBuffer {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory: absolute_path(directory),
        }
    }

    pub fn save_frame(
        &self,
        frame_id: &str,
        source: &Path,
        captured_at: DateTime<Utc>,
    ) -> io::Result<Option<PathBuf>> {
        self.ensure_directory()?;
        let file_name = file_name(frame_id, captured_at);
        let destination = self.directory.join(&file_name);
        let temporary = self
            .directory
            .join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
        let result = (|| {
            match fs::copy(source, &temporary) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            }
            set_private_file_mode(&temporary)?;
            File::open(&temporary)?.sync_all()?;
            fs::rename(&temporary, &destination)?;
            File::open(&self.directory)?.sync_all()?;
            Ok(Some(destination.clone()))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn paths_for_ids<'a, I>(
        &self,
        frame_ids: I,
        now: DateTime<Utc>,
    ) -> io::Result<HashMap<String, PathBuf>>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut digest_to_id = HashMap::new();
        for frame_id in frame_ids {
            digest_to_id
                .entry(frame_digest(frame_id))
                .or_insert_with(|| frame_id.to_owned());
        }
        if digest_to_id.is_empty() {
            return Ok(HashMap::new());
        }
        let cutoff = (now - RETENTION).timestamp_millis();
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(error) => return Err(error),
        };
        let mut paths = HashMap::<String, (i64, PathBuf)>::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            let Some((digest, timestamp)) = parse_file_name(&path) else {
                continue;
            };
            if timestamp < cutoff {
                continue;
            }
            let Some(frame_id) = digest_to_id.get(digest) else {
                continue;
            };
            let replace = paths
                .get(frame_id)
                .is_none_or(|(previous, _)| timestamp > *previous);
            if replace {
                paths.insert(frame_id.clone(), (timestamp, path));
            }
        }
        Ok(paths
            .into_iter()
            .map(|(frame_id, (_, path))| (frame_id, path))
            .collect())
    }

    pub fn cleanup_expired(&self, now: DateTime<Utc>) -> io::Result<()> {
        self.ensure_directory()?;
        let cutoff = (now - RETENTION).timestamp_millis();
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let Some((_, timestamp)) = parse_file_name(&entry.path()) else {
                continue;
            };
            if timestamp < cutoff {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    fn ensure_directory(&self) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        crate::persistence::set_private_directory_mode(&self.directory)?;
        crate::persistence::cleanup_stale_temps(&self.directory)
    }
}

fn absolute_path(path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(&path))
            .unwrap_or(path)
    }
}

fn file_name(frame_id: &str, captured_at: DateTime<Utc>) -> String {
    format!(
        "{FILE_PREFIX}{}-{}{FILE_SUFFIX}",
        frame_digest(frame_id),
        captured_at.timestamp_millis()
    )
}

fn frame_digest(frame_id: &str) -> String {
    let digest = Sha256::digest(frame_id.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_file_name(path: &Path) -> Option<(&str, i64)> {
    let stem = path
        .file_name()?
        .to_str()?
        .strip_prefix(FILE_PREFIX)?
        .strip_suffix(FILE_SUFFIX)?;
    let (digest, timestamp) = stem.rsplit_once('-')?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some((digest, timestamp.parse().ok()?))
}

fn set_private_file_mode(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

