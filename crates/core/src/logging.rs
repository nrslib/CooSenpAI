use crate::persistence::{set_private_directory_mode, set_private_file_mode, SiblingLock};
use crate::ports::RuntimeLogger;
use chrono::Utc;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileLogger {
    path: PathBuf,
    lock_path: PathBuf,
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
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        set_private_file_mode(&file)?;
        file.write_all(line.as_bytes())?;
        file.sync_all()
    }
}

