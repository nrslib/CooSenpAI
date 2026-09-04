use super::*;
use std::path::Component;

impl CompanionStorage {
    pub fn persist_attachment(
        &self,
        source: &Path,
        id: &str,
        created_at: &str,
    ) -> Result<String, PersistenceError> {
        let relative = self.attachment_store().persist(source, id, created_at)?;
        self.prune_attachments()?;
        Ok(relative)
    }

    pub fn resolve_attachment(&self, relative: &str) -> Result<PathBuf, PersistenceError> {
        self.attachment_store().resolve(relative)
    }

    pub fn prune_attachments(&self) -> Result<(), PersistenceError> {
        let preserved = self.protected_attachment_paths()?;
        self.attachment_store()
            .prune_at_preserving(chrono::Utc::now(), &preserved)
    }

    fn protected_attachment_paths(&self) -> Result<HashSet<PathBuf>, PersistenceError> {
        let generation = self.conversation_generation()?;
        let mut preserved = HashSet::new();
        if self.conversation_directory.exists() {
            for path in super::daily_conversation_paths(&self.conversation_directory)? {
                let file = File::open(path)?;
                for line in BufReader::new(file).lines() {
                    let line = line?;
                    let Ok(value) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    let stored_generation = value
                        .get("conversationGeneration")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    if stored_generation == generation {
                        continue;
                    }
                    if let Some(path) = value
                        .get("attachmentPath")
                        .and_then(Value::as_str)
                        .filter(|path| valid_attachment_relative_path(path))
                    {
                        preserved.insert(PathBuf::from(path));
                    }
                }
            }
        }
        if self.cursor_path.exists() {
            let cursor = self.load_cursor()?;
            for pending in cursor.pending_inputs {
                let PendingInput::UserMessage(input) = pending;
                if let Some(path) = input
                    .attachment_path
                    .filter(|path| valid_attachment_relative_path(path))
                {
                    preserved.insert(PathBuf::from(path));
                }
            }
        }
        Ok(preserved)
    }

    fn attachment_store(&self) -> crate::attachments::AttachmentStore {
        crate::attachments::AttachmentStore::new(
            self.state_directory.clone(),
            self.attachments_directory.clone(),
            self.retention_days,
        )
    }
}

fn valid_attachment_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    path.is_relative()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && path
            .components()
            .next()
            .and_then(|component| match component {
                Component::Normal(value) => value.to_str(),
                _ => None,
            })
            == Some("attachments")
        && path.extension().and_then(|extension| extension.to_str()) == Some("png")
}
