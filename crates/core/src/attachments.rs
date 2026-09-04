use crate::image_processing::png_dimensions;
use crate::persistence::{
    atomic_write_bytes, retention_cutoff_date, set_private_directory_mode, PersistenceError,
};
use chrono::{DateTime, Local, NaiveDate, Utc};
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const MAX_ATTACHMENT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_TEXT_ATTACHMENT_BYTES: usize = 256 * 1024;
const MAX_ATTACHMENT_DIMENSION: u32 = 16_384;

#[derive(Debug, Clone)]
pub struct AttachmentStore {
    state_directory: PathBuf,
    root: PathBuf,
    retention_days: u64,
}

impl AttachmentStore {
    pub fn new(state_directory: PathBuf, root: PathBuf, retention_days: u64) -> Self {
        Self {
            state_directory,
            root,
            retention_days,
        }
    }

    pub fn persist(
        &self,
        source: &Path,
        id: &str,
        created_at: &str,
    ) -> Result<String, PersistenceError> {
        if !valid_id(id) {
            return Err(PersistenceError::Invalid(
                "添付画像の id が不正です".to_owned(),
            ));
        }
        let metadata = fs::metadata(source)?;
        if !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > MAX_ATTACHMENT_BYTES as u64
        {
            return Err(PersistenceError::Invalid(
                "添付画像の容量が不正です".to_owned(),
            ));
        }
        let bytes = fs::read(source)?;
        let (width, height) = png_dimensions(&bytes)
            .ok_or_else(|| PersistenceError::Invalid("添付画像が PNG ではありません".to_owned()))?;
        if width > MAX_ATTACHMENT_DIMENSION || height > MAX_ATTACHMENT_DIMENSION {
            return Err(PersistenceError::Invalid(
                "添付画像の寸法が上限を超えています".to_owned(),
            ));
        }
        let date = DateTime::parse_from_rfc3339(created_at)
            .map_err(|_| PersistenceError::Invalid("添付画像の日時が不正です".to_owned()))?
            .with_timezone(&Local)
            .format("%Y-%m-%d")
            .to_string();
        let relative = PathBuf::from("attachments")
            .join(date)
            .join(format!("{id}.png"));
        let destination = self.state_directory.join(&relative);
        if destination.exists() {
            let existing = fs::read(&destination)?;
            if existing != bytes {
                return Err(PersistenceError::Invalid(
                    "同じ id の添付画像が一致しません".to_owned(),
                ));
            }
        } else {
            atomic_write_bytes(&destination, &bytes)?;
        }
        relative
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| PersistenceError::Invalid("添付画像のパスが不正です".to_owned()))
    }

    pub fn resolve(&self, relative: &str) -> Result<PathBuf, PersistenceError> {
        let path = Path::new(relative);
        if path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || path.components().next().and_then(|value| match value {
                Component::Normal(value) => value.to_str(),
                _ => None,
            }) != Some("attachments")
            || path.extension().and_then(|value| value.to_str()) != Some("png")
        {
            return Err(PersistenceError::Invalid(
                "添付画像の相対パスが不正です".to_owned(),
            ));
        }
        let resolved = self.state_directory.join(path);
        if !resolved.is_file() || !resolved.starts_with(&self.root) {
            return Err(PersistenceError::Invalid(
                "添付画像が見つかりません".to_owned(),
            ));
        }
        Ok(resolved)
    }

    pub fn prune_at(&self, now: DateTime<Utc>) -> Result<(), PersistenceError> {
        self.prune_at_preserving(now, &HashSet::new())
    }

    pub fn prune_at_preserving(
        &self,
        now: DateTime<Utc>,
        preserved: &HashSet<PathBuf>,
    ) -> Result<(), PersistenceError> {
        if !self.root.exists() {
            return Ok(());
        }
        let cutoff = retention_cutoff_date(now, self.retention_days);
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(date) = NaiveDate::parse_from_str(&name, "%Y-%m-%d") else {
                continue;
            };
            if date < cutoff {
                let entry_path = entry.path();
                let relative_directory =
                    entry_path
                        .strip_prefix(&self.state_directory)
                        .map_err(|_| {
                            PersistenceError::Invalid(
                                "添付画像の日付ディレクトリが不正です".to_owned(),
                            )
                        })?;
                if preserved
                    .iter()
                    .any(|path| path.starts_with(relative_directory))
                {
                    continue;
                }
                fs::remove_dir_all(entry_path)?;
            }
        }
        set_private_directory_mode(&self.root)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedTextAttachment {
    pub text: String,
    pub truncated: bool,
    pub truncated_characters: usize,
}

pub fn bound_text_attachment(value: &str) -> Option<BoundedTextAttachment> {
    if value.trim().is_empty() {
        return None;
    }
    let normalized = normalize_text_attachment(value);
    if normalized.len() <= MAX_TEXT_ATTACHMENT_BYTES {
        return Some(BoundedTextAttachment {
            text: normalized,
            truncated: false,
            truncated_characters: 0,
        });
    }
    let mut end = utf8_boundary_at_or_before(&normalized, MAX_TEXT_ATTACHMENT_BYTES);
    loop {
        let omitted = normalized[end..].chars().count();
        let notice = truncation_notice(omitted);
        let next = utf8_boundary_at_or_before(
            &normalized,
            MAX_TEXT_ATTACHMENT_BYTES.saturating_sub(notice.len()),
        );
        if next == end {
            return Some(BoundedTextAttachment {
                text: format!("{}{}", &normalized[..end], notice),
                truncated: true,
                truncated_characters: omitted,
            });
        }
        end = next;
    }
}

fn normalize_text_attachment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect()
}

fn utf8_boundary_at_or_before(value: &str, limit: usize) -> usize {
    let mut end = limit.min(value.len());
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    end
}

fn truncation_notice(omitted: usize) -> String {
    format!("\n\n末尾を切りました（{omitted} 文字）")
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

