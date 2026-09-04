use crate::companion_storage::PendingInput;
use crate::config::ConfigPaths;
use crate::persistence::PersistenceError;
use chrono::{DateTime, Utc};
use std::path::{Component, Path};

pub(super) fn rebase_pending_input_attachments(
    paths: &ConfigPaths,
    archive: &Path,
    pending_inputs: Vec<PendingInput>,
    now: DateTime<Utc>,
    retention_days: u64,
) -> Result<Vec<PendingInput>, PersistenceError> {
    let attachment_store = crate::attachments::AttachmentStore::new(
        paths.state.clone(),
        paths.attachments.clone(),
        retention_days,
    );
    let created_at = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    pending_inputs
        .into_iter()
        .map(|pending| {
            let PendingInput::UserMessage(mut input) = pending;
            if let Some(relative) = input.attachment_path.as_deref() {
                let relative_path = Path::new(relative);
                if !valid_attachment_relative_path(relative_path) {
                    return Err(PersistenceError::Invalid(
                        "pending input の添付画像パスが不正です".to_owned(),
                    ));
                }
                let source = archive.join(relative_path);
                if source.is_file() {
                    input.attachment_path =
                        Some(attachment_store.persist(&source, &input.id, &created_at)?);
                }
            }
            Ok(PendingInput::UserMessage(input))
        })
        .collect()
}

fn valid_attachment_relative_path(path: &Path) -> bool {
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
