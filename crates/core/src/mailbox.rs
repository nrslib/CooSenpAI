use crate::persistence::{set_private_file_mode, PersistenceError, SiblingLock};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MailboxEnvelope {
    pub schema_version: u8,
    pub id: String,
    pub created_at: String,
    pub kind: String,
    pub payload: Value,
}

#[derive(Debug, Error)]
pub enum MailboxError {
    #[error("mailbox I/O に失敗しました: {0}")]
    Io(#[from] io::Error),
    #[error("mailbox の JSON が不正です")]
    Json(#[from] serde_json::Error),
    #[error("mailbox recipient が不正です")]
    InvalidRecipient,
    #[error("mailbox envelope が不正です")]
    InvalidEnvelope,
    #[error("mailbox の lock を取得できません: {0}")]
    Lock(#[from] PersistenceError),
}

#[derive(Debug, Clone)]
pub struct Mailbox {
    root: PathBuf,
    recipient: String,
}

impl Mailbox {
    pub fn new(root: PathBuf, recipient: impl Into<String>) -> Result<Self, MailboxError> {
        let mailbox = Self::open(root, recipient)?;
        mailbox.recover()?;
        Ok(mailbox)
    }

    pub fn open(root: PathBuf, recipient: impl Into<String>) -> Result<Self, MailboxError> {
        let recipient = recipient.into();
        if !valid_recipient(&recipient) {
            return Err(MailboxError::InvalidRecipient);
        }
        let mailbox = Self { root, recipient };
        mailbox.ensure_layout()?;
        Ok(mailbox)
    }

    pub fn recover(&self) -> Result<(), MailboxError> {
        let _lock = self.lock()?;
        self.recover_processing()
    }

    pub fn publish(
        &self,
        kind: impl Into<String>,
        payload: Value,
    ) -> Result<MailboxEnvelope, MailboxError> {
        let envelope = MailboxEnvelope {
            schema_version: 1,
            id: Uuid::new_v4().to_string(),
            created_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            kind: kind.into(),
            payload,
        };
        let _lock = self.lock()?;
        self.publish_with_lock(&_lock, &envelope)
    }

    pub(crate) fn acquire_delivery_lock(&self) -> Result<SiblingLock, MailboxError> {
        self.lock()
    }

    pub(crate) fn publish_with_lock(
        &self,
        _lock: &SiblingLock,
        envelope: &MailboxEnvelope,
    ) -> Result<MailboxEnvelope, MailboxError> {
        if !valid_envelope(envelope) {
            return Err(MailboxError::InvalidEnvelope);
        }
        if self.contains_id_with_lock(&envelope.id)? {
            return Ok(envelope.clone());
        }
        let bytes = serde_json::to_vec(envelope)?;
        let filename = format!(
            "{}-{}.json",
            safe_timestamp(&envelope.created_at),
            envelope.id
        );
        let inbox = self.directory("inbox");
        let temp = inbox.join(format!("{filename}.tmp"));
        let destination = inbox.join(&filename);
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            set_private_file_mode(&file)?;
            use std::io::Write;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temp, &destination)?;
            std::fs::File::open(&inbox)?.sync_all()?;
            Ok::<(), io::Error>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result?;
        Ok(envelope.clone())
    }

    pub(crate) fn publish_with_identity(
        &self,
        kind: String,
        id: String,
        created_at: String,
        payload: Value,
    ) -> Result<MailboxEnvelope, MailboxError> {
        let envelope = MailboxEnvelope {
            schema_version: 1,
            id,
            created_at,
            kind,
            payload,
        };
        let lock = self.lock()?;
        self.publish_with_lock(&lock, &envelope)
    }

    pub(crate) fn root_path(&self) -> &Path {
        &self.root
    }

    pub(crate) fn recipient_name(&self) -> &str {
        &self.recipient
    }

    pub fn claim(&self) -> Result<Option<ClaimedEnvelope>, MailboxError> {
        let inbox = self.directory("inbox");
        let processing = self.directory("processing");
        let _lock = self.lock()?;
        let Some(path) = first_json(&inbox)? else {
            return Ok(None);
        };
        let destination = processing.join(path.file_name().ok_or(MailboxError::InvalidEnvelope)?);
        fs::rename(&path, &destination)?;
        let bytes = fs::read(&destination)?;
        let envelope = match serde_json::from_slice::<MailboxEnvelope>(&bytes) {
            Ok(envelope) if valid_envelope(&envelope) => envelope,
            _ => {
                fs::rename(
                    &destination,
                    self.directory("failed").join(
                        destination
                            .file_name()
                            .ok_or(MailboxError::InvalidEnvelope)?,
                    ),
                )?;
                return Ok(None);
            }
        };
        if !valid_envelope(&envelope) {
            return Err(MailboxError::InvalidEnvelope);
        }
        Ok(Some(ClaimedEnvelope {
            envelope,
            path: destination,
        }))
    }

    pub fn complete(&self, claimed: ClaimedEnvelope) -> Result<(), MailboxError> {
        let file_name = claimed.file_name().to_owned();
        let _lock = self.lock()?;
        fs::rename(claimed.path, self.directory("done").join(file_name))?;
        Ok(())
    }

    pub fn retry(&self, claimed: ClaimedEnvelope) -> Result<(), MailboxError> {
        let file_name = claimed.file_name().to_owned();
        let _lock = self.lock()?;
        fs::rename(claimed.path, self.directory("inbox").join(file_name))?;
        Ok(())
    }

    pub fn fail(&self, claimed: ClaimedEnvelope) -> Result<(), MailboxError> {
        let file_name = claimed.file_name().to_owned();
        let _lock = self.lock()?;
        fs::rename(claimed.path, self.directory("failed").join(file_name))?;
        Ok(())
    }

    pub fn prune_done(&self, retention_days: i64) -> Result<(), MailboxError> {
        let _lock = self.lock()?;
        let cutoff = Utc::now() - Duration::days(retention_days);
        for entry in fs::read_dir(self.directory("done"))? {
            let entry = entry?;
            let modified = entry.metadata()?.modified().ok().map(DateTime::<Utc>::from);
            if modified.is_some_and(|time| time < cutoff) {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    fn ensure_layout(&self) -> Result<(), MailboxError> {
        fs::create_dir_all(&self.root)?;
        set_private_mode(&self.root)?;
        let recipient = self.root.join(&self.recipient);
        fs::create_dir_all(&recipient)?;
        set_private_mode(&recipient)?;
        for directory in ["inbox", "processing", "done", "failed"] {
            let path = self.directory(directory);
            fs::create_dir_all(&path)?;
            set_private_mode(&path)?;
        }
        Ok(())
    }

    fn lock(&self) -> Result<SiblingLock, MailboxError> {
        Ok(SiblingLock::acquire(
            &self.root.join(&self.recipient).join(".mailbox.lock"),
        )?)
    }

    fn recover_processing(&self) -> Result<(), MailboxError> {
        let processing = self.directory("processing");
        for entry in fs::read_dir(&processing)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some("json") {
                if let Some(name) = path.file_name().map(|value| value.to_owned()) {
                    fs::rename(path, self.directory("inbox").join(name))?;
                }
            }
        }
        Ok(())
    }

    fn contains_id_with_lock(&self, id: &str) -> Result<bool, MailboxError> {
        for phase in ["inbox", "processing", "done"] {
            for entry in fs::read_dir(self.directory(phase))? {
                let path = entry?.path();
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let Ok(bytes) = fs::read(path) else {
                    continue;
                };
                let Ok(envelope) = serde_json::from_slice::<MailboxEnvelope>(&bytes) else {
                    continue;
                };
                if envelope.id == id {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn directory(&self, phase: &str) -> PathBuf {
        self.root.join(&self.recipient).join(phase)
    }
}

pub fn archive_mailbox_kind(
    root: &Path,
    kind: &str,
    destination: &Path,
) -> Result<(), MailboxError> {
    let recipients = match fs::read_dir(root) {
        Ok(entries) => entries.collect::<Result<Vec<_>, _>>()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for recipient in recipients {
        if !recipient.file_type()?.is_dir() {
            continue;
        }
        let Some(name) = recipient.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let mailbox = Mailbox::open(root.to_path_buf(), name.clone())?;
        let _lock = mailbox.lock()?;
        for phase in ["inbox", "processing", "done"] {
            let source = mailbox.directory(phase);
            let target = destination.join(&name).join(phase);
            fs::create_dir_all(&target)?;
            set_private_mode(&target)?;
            for item in fs::read_dir(&source)? {
                let path = item?.path();
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let Ok(bytes) = fs::read(&path) else { continue };
                let Ok(envelope) = serde_json::from_slice::<MailboxEnvelope>(&bytes) else {
                    continue;
                };
                if envelope.kind == kind {
                    let target_path =
                        target.join(path.file_name().ok_or(MailboxError::InvalidEnvelope)?);
                    if !target_path.exists() {
                        fs::rename(&path, target_path)?;
                    }
                }
            }
            fs::File::open(&source)?.sync_all()?;
            fs::File::open(&target)?.sync_all()?;
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct ClaimedEnvelope {
    pub envelope: MailboxEnvelope,
    path: PathBuf,
}

impl ClaimedEnvelope {
    fn file_name(&self) -> &Path {
        self.path
            .file_name()
            .map(Path::new)
            .unwrap_or_else(|| Path::new("envelope.json"))
    }
}

fn first_json(directory: &Path) -> Result<Option<PathBuf>, io::Error> {
    let mut paths = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_file())
                .map(|_| entry.path())
        })
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths.into_iter().next())
}

fn valid_recipient(value: &str) -> bool {
    !value.is_empty()
        && value.chars().enumerate().all(|(index, character)| {
            character.is_ascii_alphanumeric() || index > 0 && (character == '_' || character == '-')
        })
}

fn valid_envelope(value: &MailboxEnvelope) -> bool {
    value.schema_version == 1
        && is_safe_file_part(&value.id)
        && is_safe_file_part(&value.created_at)
        && matches!(value.kind.as_str(), "observation" | "remark")
        && value.payload.is_object()
}

fn is_safe_file_part(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|character| character == '/' || character == '\\' || character.is_control())
}

fn safe_timestamp(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric()
                || *character == '-'
                || *character == 'T'
                || *character == 'Z'
        })
        .collect()
}

fn set_private_mode(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

