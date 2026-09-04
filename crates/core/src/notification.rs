use crate::mailbox::{ClaimedEnvelope, Mailbox, MailboxError};
use crate::persistence::{atomic_write_json, PersistenceError, SiblingLock};
use crate::ports::{NotificationPort, RuntimeLogger};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

const PROCESSED_ID_LIMIT: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemarkEnvelope {
    #[serde(default)]
    pub conversation_generation: u64,
    pub entry_id: String,
    pub message: String,
    #[serde(default = "default_message_kind")]
    pub message_kind: String,
    pub notification_priority: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caused_by: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub caused_by_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationRecord {
    pub id: String,
    pub created_at: String,
    pub message: String,
    pub message_kind: String,
    pub priority: String,
    pub caused_by: Option<String>,
    pub caused_by_ids: Vec<String>,
    pub conversation_generation: u64,
}

#[derive(Debug, Error)]
pub enum NotificationError {
    #[error("notification mailbox に失敗しました: {0}")]
    Mailbox(#[from] MailboxError),
    #[error("notification の永続化に失敗しました: {0}")]
    Persistence(#[from] PersistenceError),
    #[error("notification の JSON が不正です: {0}")]
    Json(#[from] serde_json::Error),
    #[error("notification の I/O に失敗しました: {0}")]
    Io(#[from] io::Error),
    #[error("notification payload が不正です")]
    InvalidPayload,
    #[error("notification priority が不正です")]
    InvalidPriority,
}

#[derive(Clone)]
pub struct NotificationConsumer {
    mailbox: Mailbox,
    processed: ProcessedIds,
    minimum_priority: String,
    logger: Option<Arc<dyn RuntimeLogger>>,
    conversation_generation_path: PathBuf,
}

impl NotificationConsumer {
    pub fn new(
        mailbox_root: PathBuf,
        recipient: &str,
        processed_path: PathBuf,
        minimum_priority: impl Into<String>,
    ) -> Result<Self, NotificationError> {
        let minimum_priority = minimum_priority.into();
        if priority_rank(&minimum_priority).is_none() {
            return Err(NotificationError::InvalidPriority);
        }
        let conversation_generation_path =
            processed_path.with_file_name("conversation-generation.json");
        Ok(Self {
            mailbox: Mailbox::new(mailbox_root, recipient)?,
            processed: ProcessedIds::new(processed_path),
            minimum_priority,
            logger: None,
            conversation_generation_path,
        })
    }

    pub fn with_logger(mut self, logger: Arc<dyn RuntimeLogger>) -> Self {
        self.logger = Some(logger);
        self
    }

    pub fn update_minimum_priority(
        &mut self,
        minimum_priority: impl Into<String>,
    ) -> Result<(), NotificationError> {
        let minimum_priority = minimum_priority.into();
        if priority_rank(&minimum_priority).is_none() {
            return Err(NotificationError::InvalidPriority);
        }
        self.minimum_priority = minimum_priority;
        Ok(())
    }

    pub fn claim_next(&self) -> Result<Option<PendingNotification>, NotificationError> {
        let result = self.claim_displayable();
        if result.is_err() {
            self.log_failure();
        }
        result
    }

    pub fn accept(&self, pending: PendingNotification) -> Result<(), NotificationError> {
        if !self.is_current(&pending)? {
            return self.skip(pending);
        }
        let result = (|| {
            self.processed
                .claim(&self.processed_key(&pending.record.id))?;
            self.mailbox.complete(pending.envelope)?;
            Ok::<(), NotificationError>(())
        })();
        if result.is_err() {
            self.log_failure();
        }
        result
    }

    pub fn skip(&self, pending: PendingNotification) -> Result<(), NotificationError> {
        self.mailbox
            .complete(pending.envelope)
            .map_err(NotificationError::from)
    }

    pub fn retry(&self, pending: PendingNotification) -> Result<(), NotificationError> {
        let result = self
            .mailbox
            .retry(pending.envelope)
            .map_err(NotificationError::from);
        if result.is_err() {
            self.log_failure();
        }
        result
    }

    pub async fn deliver_one(
        &self,
        notifier: &dyn NotificationPort,
        duration: Duration,
    ) -> Result<bool, NotificationError> {
        let result = async {
            let Some(claimed) = self.claim_displayable()? else {
                return Ok(false);
            };
            if !self.is_current(&claimed)? {
                self.skip(claimed)?;
                return Ok(false);
            }
            if let Err(error) = notifier
                .show(&claimed.record.message, &claimed.record.priority, duration)
                .await
            {
                self.retry(claimed)?;
                return Err(NotificationError::Io(io::Error::other(error.to_string())));
            }
            self.accept(claimed)?;
            Ok(true)
        }
        .await;
        if result.is_err() {
            self.log_failure();
        }
        result
    }

    fn claim_displayable(&self) -> Result<Option<PendingNotification>, NotificationError> {
        loop {
            let Some(envelope) = self.mailbox.claim()? else {
                return Ok(None);
            };
            let payload = match parse_remark(&envelope.envelope.payload) {
                Ok(payload) => payload,
                Err(_) => {
                    self.log_failure();
                    self.mailbox.fail(envelope)?;
                    continue;
                }
            };
            if self
                .processed
                .contains(&self.processed_key(&envelope.envelope.id))?
            {
                self.mailbox.complete(envelope)?;
                continue;
            }
            let record = NotificationRecord {
                id: envelope.envelope.id.clone(),
                created_at: envelope.envelope.created_at.clone(),
                message: payload.message,
                message_kind: payload.message_kind,
                priority: payload.notification_priority,
                caused_by: payload.caused_by,
                caused_by_ids: payload.caused_by_ids,
                conversation_generation: payload.conversation_generation,
            };
            if record.conversation_generation != self.current_generation()? {
                self.mailbox.complete(envelope)?;
                continue;
            }
            let priority =
                priority_rank(&record.priority).ok_or(NotificationError::InvalidPriority)?;
            let minimum =
                priority_rank(&self.minimum_priority).ok_or(NotificationError::InvalidPriority)?;
            if record.message_kind != "chat" && priority < minimum {
                self.mailbox.complete(envelope)?;
                continue;
            }
            return Ok(Some(PendingNotification { envelope, record }));
        }
    }

    fn log_failure(&self) {
        if let Some(logger) = &self.logger {
            let _ = logger.write("WARN", "通知処理に失敗しました: error-type=notification");
        }
    }

    fn processed_key(&self, id: &str) -> String {
        format!("{}:{id}", self.mailbox.recipient_name())
    }

    pub fn is_current(&self, pending: &PendingNotification) -> Result<bool, NotificationError> {
        Ok(pending.record.conversation_generation == self.current_generation()?)
    }

    fn current_generation(&self) -> Result<u64, NotificationError> {
        match fs::read(&self.conversation_generation_path) {
            Ok(bytes) => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Generation {
                    schema_version: u8,
                    generation: u64,
                }
                let value: Generation = serde_json::from_slice(&bytes)?;
                if value.schema_version != 1 {
                    return Err(NotificationError::InvalidPayload);
                }
                Ok(value.generation)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(error.into()),
        }
    }
}

#[derive(Debug)]
pub struct PendingNotification {
    envelope: ClaimedEnvelope,
    pub record: NotificationRecord,
}

fn parse_remark(value: &Value) -> Result<RemarkEnvelope, NotificationError> {
    let payload: RemarkEnvelope = serde_json::from_value(value.clone())?;
    if payload.entry_id.is_empty()
        || payload.message.is_empty()
        || !matches!(
            payload.message_kind.as_str(),
            "advice" | "encouragement" | "nudge" | "celebration" | "summary" | "chat"
        )
        || priority_rank(&payload.notification_priority).is_none()
    {
        return Err(NotificationError::InvalidPayload);
    }
    Ok(payload)
}

fn default_message_kind() -> String {
    "advice".to_owned()
}

fn priority_rank(value: &str) -> Option<u8> {
    match value {
        "none" => Some(0),
        "info" => Some(1),
        "warning" => Some(2),
        "critical" => Some(3),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct ProcessedIds {
    path: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessedSnapshot {
    #[serde(default)]
    ids: Vec<String>,
}

impl ProcessedIds {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn contains(&self, id: &str) -> Result<bool, NotificationError> {
        Ok(self.load()?.ids.iter().any(|value| value == id))
    }

    fn claim(&self, id: &str) -> Result<bool, NotificationError> {
        let lock_path = self.path.with_file_name(format!(
            ".{}.lock",
            self.path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("notification-processed")
        ));
        let _lock = SiblingLock::acquire(&lock_path)?;
        let mut snapshot = self.load_unlocked()?;
        if snapshot.ids.iter().any(|value| value == id) {
            return Ok(false);
        }
        snapshot.ids.push(id.to_owned());
        if snapshot.ids.len() > PROCESSED_ID_LIMIT {
            snapshot
                .ids
                .drain(..snapshot.ids.len() - PROCESSED_ID_LIMIT);
        }
        atomic_write_json(&self.path, &snapshot)?;
        Ok(true)
    }

    fn load(&self) -> Result<ProcessedSnapshot, NotificationError> {
        let lock_path = self.path.with_file_name(format!(
            ".{}.lock",
            self.path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("notification-processed")
        ));
        let _lock = SiblingLock::acquire(&lock_path)?;
        self.load_unlocked()
    }

    fn load_unlocked(&self) -> Result<ProcessedSnapshot, NotificationError> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Ok(ProcessedSnapshot::default())
            }
            Err(error) => Err(error.into()),
        }
    }
}

