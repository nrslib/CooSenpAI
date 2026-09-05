use crate::persistence::{set_private_directory_mode, set_private_file_mode, SiblingLock};
use crate::ports::RuntimeLogger;
use chrono::Utc;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;
const MAX_GENERATIONS: usize = 5;

#[derive(Debug, Clone)]
pub struct FileLogger {
    path: PathBuf,
    lock_path: PathBuf,
    max_bytes: u64,
}

impl FileLogger {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
            set_private_directory_mode(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        set_private_file_mode(&file)?;
        Ok(Self {
            lock_path: path.with_file_name(format!(
                ".{}.lock",
                path.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("log")
            )),
            path,
            max_bytes: DEFAULT_MAX_BYTES,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl RuntimeLogger for FileLogger {
    fn write(&self, level: &str, message: &str) -> io::Result<()> {
        if !matches!(level, "DEBUG" | "INFO" | "WARN" | "ERROR") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ログレベルが不正です",
            ));
        }
        let _lock = SiblingLock::acquire(&self.lock_path)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let message = message.split_whitespace().collect::<Vec<_>>().join(" ");
        let line = format!(
            "{} {} {}\n",
            Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            level,
            message
        );
        if should_rotate(&self.path, self.max_bytes, line.len() as u64)? {
            rotate(&self.path)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        set_private_file_mode(&file)?;
        file.write_all(line.as_bytes())?;
        file.sync_all()
    }
}

fn should_rotate(path: &Path, max_bytes: u64, next_line_bytes: u64) -> io::Result<bool> {
    let current_bytes = match fs::metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };
    Ok(current_bytes > 0 && current_bytes.saturating_add(next_line_bytes) > max_bytes)
}

fn rotate(path: &Path) -> io::Result<()> {
    remove_file_if_exists(&generation_path(path, MAX_GENERATIONS))?;
    for generation in (1..MAX_GENERATIONS).rev() {
        let source = generation_path(path, generation);
        let target = generation_path(path, generation + 1);
        if source.exists() {
            remove_file_if_exists(&target)?;
            fs::rename(source, target)?;
        }
    }
    if path.exists() {
        let first_generation = generation_path(path, 1);
        remove_file_if_exists(&first_generation)?;
        fs::rename(path, first_generation)?;
    }
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn generation_path(path: &Path, generation: usize) -> PathBuf {
    path.with_file_name(format!(
        "{}.{}",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("coosenpai.log"),
        generation
    ))
}

fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

