use crate::logging::FileLogger;
use crate::mailbox::{Mailbox, MailboxError};
use crate::persistence::{atomic_write_json, PersistenceError, SiblingLock};
use crate::ports::RuntimeLogger;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

const PENDING_DIRECTORY: &str = "pending";
const DONE_DIRECTORY: &str = "done";
const FAILED_DIRECTORY: &str = "failed";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OutboxEntry {
    pub schema_version: u8,
    pub id: String,
    pub created_at: String,
    pub kind: String,
    pub payload: Value,
    pub recipients: Vec<String>,
    pub delivered_recipients: Vec<String>,
}

struct PendingOutboxEntry {
    created_at: DateTime<Utc>,
    id: String,
    path: PathBuf,
    entry: OutboxEntry,
}

#[derive(Debug, Error)]
pub enum OutboxError {
    #[error("outbox I/O に失敗しました: {0}")]
    Io(#[from] io::Error),
    #[error("outbox の JSON が不正です: {0}")]
    Json(#[from] serde_json::Error),
    #[error("outbox の永続化に失敗しました: {0}")]
    Persistence(#[from] PersistenceError),
    #[error("outbox mailbox に失敗しました: {0}")]
    Mailbox(#[from] MailboxError),
    #[error("outbox entry が不正です")]
    Invalid,
}

#[derive(Debug, Clone)]
pub struct DurableOutbox {
    directory: PathBuf,
    log_path: Option<PathBuf>,
}

impl DurableOutbox {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            log_path: None,
        }
    }

    pub fn with_log_path(mut self, log_path: PathBuf) -> Self {
        self.log_path = Some(log_path);
        self
    }

    pub fn enqueue(
        &self,
        id: &str,
        created_at: &str,
        kind: &str,
        payload: Value,
        recipients: &[String],
    ) -> Result<OutboxEntry, OutboxError> {
        validate_entry_parts(id, created_at, kind, recipients)?;
        let _lock = self.directory_lock()?;
        self.validate_root_layout()?;
        let pending_path = self.entry_path(PENDING_DIRECTORY, kind, id)?;
        let done_path = self.entry_path(DONE_DIRECTORY, kind, id)?;
        for path in [pending_path, done_path] {
            if path.exists() {
                let entry: OutboxEntry = serde_json::from_slice(&fs::read(path)?)?;
                validate_entry(&entry)?;
                if entry.id != id
                    || entry.kind != kind
                    || entry.created_at != created_at
                    || entry.payload != payload
                    || entry.recipients != recipients
                {
                    return Err(OutboxError::Invalid);
                }
                return Ok(entry);
            }
        }
        let entry = OutboxEntry {
            schema_version: 1,
            id: id.to_owned(),
            created_at: created_at.to_owned(),
            kind: kind.to_owned(),
            payload,
            recipients: recipients.to_vec(),
            delivered_recipients: Vec::new(),
        };
        atomic_write_json(&self.entry_path(PENDING_DIRECTORY, kind, id)?, &entry)?;
        Ok(entry)
    }

    pub fn contains(&self, kind: &str, id: &str) -> Result<bool, OutboxError> {
        let _lock = self.directory_lock()?;
        self.validate_root_layout()?;
        for path in [
            self.entry_path(PENDING_DIRECTORY, kind, id)?,
            self.entry_path(DONE_DIRECTORY, kind, id)?,
        ] {
            if !path.exists() {
                continue;
            }
            match load_pending_entry(&path) {
                Ok(entry) if entry.entry.id == id && entry.entry.kind == kind => return Ok(true),
                Ok(_) | Err(OutboxError::Json(_) | OutboxError::Invalid) => {
                    self.quarantine_invalid_entry(&path)?;
                    self.log_invalid_entry();
                }
                Err(error) => return Err(error),
            }
        }
        Ok(false)
    }

    pub fn deliver_pending(&self, mailbox_root: &Path) -> Result<(), OutboxError> {
        let _directory_lock = self.directory_lock()?;
        self.validate_root_layout()?;
        let mut entries = self.load_pending_entries()?;
        entries.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        for pending in entries {
            let path = pending.path;
            let mut entry = pending.entry;
            let delivery_result = if entry.recipients.iter().all(|recipient| {
                entry
                    .delivered_recipients
                    .iter()
                    .any(|value| value == recipient)
            }) {
                Ok(())
            } else {
                deliver_entry(&mut entry, mailbox_root)
            };
            atomic_write_json(&path, &entry)?;
            delivery_result?;
            if entry.recipients.iter().all(|recipient| {
                entry
                    .delivered_recipients
                    .iter()
                    .any(|value| value == recipient)
            }) {
                let done_path = self
                    .done_directory()
                    .join(path.file_name().ok_or(OutboxError::Invalid)?);
                fs::rename(&path, done_path)?;
                fs::File::open(self.pending_directory())?.sync_all()?;
                fs::File::open(self.done_directory())?.sync_all()?;
            }
        }
        Ok(())
    }

    pub fn archive_kind(&self, kind: &str, destination: &Path) -> Result<(), OutboxError> {
        if !matches!(kind, "observation" | "remark") {
            return Err(OutboxError::Invalid);
        }
        let _lock = self.directory_lock()?;
        self.validate_root_layout()?;
        for phase in [PENDING_DIRECTORY, DONE_DIRECTORY] {
            let source = self.directory.join(phase);
            let target = destination.join(phase);
            fs::create_dir_all(&target)?;
            crate::persistence::set_private_directory_mode(&target)?;
            for item in fs::read_dir(&source)? {
                let path = item?.path();
                let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if name.starts_with(&format!("{kind}-")) && name.ends_with(".json") {
                    let archived = target.join(name);
                    if !archived.exists() {
                        fs::rename(&path, archived)?;
                    }
                }
            }
            fs::File::open(&source)?.sync_all()?;
            fs::File::open(&target)?.sync_all()?;
        }
        Ok(())
    }

    fn directory_lock(&self) -> Result<SiblingLock, OutboxError> {
        fs::create_dir_all(&self.directory)?;
        fs::create_dir_all(self.pending_directory())?;
        fs::create_dir_all(self.done_directory())?;
        fs::create_dir_all(self.failed_directory())?;
        crate::persistence::set_private_directory_mode(&self.directory)?;
        crate::persistence::set_private_directory_mode(&self.pending_directory())?;
        crate::persistence::set_private_directory_mode(&self.done_directory())?;
        crate::persistence::set_private_directory_mode(&self.failed_directory())?;
        Ok(SiblingLock::acquire(&self.directory.join(".outbox.lock"))?)
    }

    fn pending_directory(&self) -> PathBuf {
        self.directory.join(PENDING_DIRECTORY)
    }

    fn done_directory(&self) -> PathBuf {
        self.directory.join(DONE_DIRECTORY)
    }

    fn failed_directory(&self) -> PathBuf {
        self.directory.join(FAILED_DIRECTORY)
    }

    fn load_pending_entries(&self) -> Result<Vec<PendingOutboxEntry>, OutboxError> {
        let mut entries = Vec::new();
        for directory_entry in fs::read_dir(self.pending_directory())? {
            let directory_entry = directory_entry?;
            let path = directory_entry.path();
            if !directory_entry.file_type()?.is_file()
                || path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            match load_pending_entry(&path) {
                Ok(entry) => entries.push(entry),
                Err(OutboxError::Json(_) | OutboxError::Invalid) => {
                    self.quarantine_invalid_entry(&path)?;
                    self.log_invalid_entry();
                }
                Err(error) => return Err(error),
            }
        }
        Ok(entries)
    }

    fn quarantine_invalid_entry(&self, path: &Path) -> Result<(), OutboxError> {
        let file_name = path.file_name().ok_or(OutboxError::Invalid)?;
        fs::rename(path, self.failed_directory().join(file_name))?;
        fs::File::open(path.parent().ok_or(OutboxError::Invalid)?)?.sync_all()?;
        fs::File::open(self.failed_directory())?.sync_all()?;
        Ok(())
    }

    fn log_invalid_entry(&self) {
        let Some(log_path) = &self.log_path else {
            return;
        };
        if let Ok(logger) = FileLogger::new(log_path.clone()) {
            let _ = logger.write(
                "WARN",
                "outbox entryをquarantineしました: error-type=invalid-outbox",
            );
        }
    }

    fn entry_path(&self, directory: &str, kind: &str, id: &str) -> Result<PathBuf, OutboxError> {
        if !is_safe_part(id) || !matches!(kind, "observation" | "remark") {
            return Err(OutboxError::Invalid);
        }
        Ok(self
            .directory
            .join(directory)
            .join(format!("{kind}-{id}.json")))
    }

    fn validate_root_layout(&self) -> Result<(), OutboxError> {
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            if entry.file_name().to_str() != Some(".outbox.lock") {
                return Err(OutboxError::Invalid);
            }
        }
        Ok(())
    }
}

fn load_pending_entry(path: &Path) -> Result<PendingOutboxEntry, OutboxError> {
    let entry = serde_json::from_slice::<OutboxEntry>(&fs::read(path)?)?;
    validate_entry(&entry)?;
    let created_at = DateTime::parse_from_rfc3339(&entry.created_at)
        .map_err(|_| OutboxError::Invalid)?
        .with_timezone(&Utc);
    Ok(PendingOutboxEntry {
        created_at,
        id: entry.id.clone(),
        path: path.to_owned(),
        entry,
    })
}

fn deliver_entry(entry: &mut OutboxEntry, mailbox_root: &Path) -> Result<(), OutboxError> {
    let mut recipients = entry.recipients.clone();
    recipients.sort();
    recipients.dedup();
    let mut targets = Vec::with_capacity(recipients.len());
    for recipient in recipients {
        let mailbox = Mailbox::new(mailbox_root.to_owned(), recipient.clone())?;
        let lock = mailbox.acquire_delivery_lock()?;
        targets.push((recipient, mailbox, lock));
    }
    for recipient in &entry.recipients {
        if entry
            .delivered_recipients
            .iter()
            .any(|value| value == recipient)
        {
            continue;
        }
        let (_, mailbox, lock) = targets
            .iter()
            .find(|(value, _, _)| value == recipient)
            .ok_or(OutboxError::Invalid)?;
        mailbox.publish_with_lock(
            lock,
            &crate::mailbox::MailboxEnvelope {
                schema_version: 1,
                id: entry.id.clone(),
                created_at: entry.created_at.clone(),
                kind: entry.kind.clone(),
                payload: entry.payload.clone(),
            },
        )?;
        entry.delivered_recipients.push(recipient.clone());
    }
    Ok(())
}

fn validate_entry(entry: &OutboxEntry) -> Result<(), OutboxError> {
    if entry.schema_version != 1
        || entry.recipients.is_empty()
        || entry
            .recipients
            .iter()
            .any(|recipient| !valid_recipient(recipient))
        || entry
            .delivered_recipients
            .iter()
            .any(|recipient| !entry.recipients.iter().any(|value| value == recipient))
    {
        return Err(OutboxError::Invalid);
    }
    validate_entry_parts(&entry.id, &entry.created_at, &entry.kind, &entry.recipients)
}

fn validate_entry_parts(
    id: &str,
    created_at: &str,
    kind: &str,
    recipients: &[String],
) -> Result<(), OutboxError> {
    if !is_safe_part(id)
        || !is_safe_part(created_at)
        || !matches!(kind, "observation" | "remark")
        || recipients.is_empty()
        || recipients
            .iter()
            .any(|recipient| !valid_recipient(recipient))
    {
        return Err(OutboxError::Invalid);
    }
    Ok(())
}

fn valid_recipient(value: &str) -> bool {
    !value.is_empty()
        && value.chars().enumerate().all(|(index, character)| {
            character.is_ascii_alphanumeric() || index > 0 && (character == '_' || character == '-')
        })
}

fn is_safe_part(value: &str) -> bool {
    !value.is_empty()
        && !value
            .chars()
            .any(|character| character == '/' || character == '\\' || character.is_control())
}
