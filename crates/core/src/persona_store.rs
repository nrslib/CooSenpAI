use crate::config::ConfigPaths;
use crate::persistence::{
    atomic_write_bytes, atomic_write_json, set_private_directory_mode, PersistenceError,
    SiblingLock,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

const VERSION_LIMIT: usize = 5;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersonaCatalogEntry {
    pub id: String,
    pub display_name: String,
    pub builtin: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PersonaVersion {
    pub id: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct PersonaStore {
    user_directory: PathBuf,
    builtin_directory: PathBuf,
}

impl PersonaStore {
    pub fn from_paths(paths: &ConfigPaths) -> Result<Self, PersonaStoreError> {
        let builtin_directory = paths
            .builtin_personas
            .clone()
            .ok_or(PersonaStoreError::MissingBuiltinDirectory)?;
        Ok(Self {
            user_directory: paths.personas.clone(),
            builtin_directory,
        })
    }

    pub fn list(&self) -> Result<Vec<PersonaCatalogEntry>, PersonaStoreError> {
        let names = self.load_names()?;
        let builtin_ids = markdown_ids(&self.builtin_directory)?;
        let mut result = Vec::new();
        for id in &builtin_ids {
            result.push(PersonaCatalogEntry {
                display_name: id.clone(),
                id: id.clone(),
                builtin: true,
            });
        }
        for id in markdown_ids(&self.user_directory)?
            .difference(&builtin_ids)
            .cloned()
        {
            result.push(PersonaCatalogEntry {
                display_name: names.get(&id).cloned().unwrap_or_else(|| id.clone()),
                id,
                builtin: false,
            });
        }
        result.sort_by(|left, right| {
            right
                .builtin
                .cmp(&left.builtin)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(result)
    }

    pub fn load_body(&self, id: &str) -> Result<String, PersonaStoreError> {
        validate_id(id)?;
        let builtin = self.builtin_path(id);
        let path = if builtin.is_file() {
            builtin
        } else {
            self.user_path(id)
        };
        Ok(fs::read_to_string(path)?)
    }

    pub fn save_custom(
        &self,
        id: &str,
        display_name: &str,
        body: &str,
        timestamp: &str,
    ) -> Result<(), PersonaStoreError> {
        validate_id(id)?;
        validate_display_name(display_name)?;
        validate_body(body)?;
        if self.builtin_path(id).is_file() {
            return Err(PersonaStoreError::BuiltinReadOnly(id.to_owned()));
        }
        fs::create_dir_all(&self.user_directory)?;
        let _guard = SiblingLock::acquire(&self.lock_path())?;
        let path = self.user_path(id);
        if path.is_file() {
            self.archive_current(id, timestamp)?;
        }
        atomic_write_bytes(&path, body.as_bytes())?;
        let mut names = self.load_names_unlocked()?;
        names.insert(id.to_owned(), display_name.trim().to_owned());
        atomic_write_json(&self.names_path(), &NamesDocument { names })?;
        self.prune_versions(id)?;
        Ok(())
    }

    pub fn delete_custom(&self, id: &str) -> Result<(), PersonaStoreError> {
        validate_id(id)?;
        if self.builtin_path(id).is_file() {
            return Err(PersonaStoreError::BuiltinReadOnly(id.to_owned()));
        }
        let _guard = SiblingLock::acquire(&self.lock_path())?;
        let path = self.user_path(id);
        if !path.is_file() {
            return Err(PersonaStoreError::NotCustom(id.to_owned()));
        }
        fs::remove_file(path)?;
        let versions = self.versions_directory(id);
        if versions.is_dir() {
            fs::remove_dir_all(versions)?;
        }
        let mut names = self.load_names_unlocked()?;
        names.remove(id);
        atomic_write_json(&self.names_path(), &NamesDocument { names })?;
        Ok(())
    }

    pub fn versions(&self, id: &str) -> Result<Vec<PersonaVersion>, PersonaStoreError> {
        validate_id(id)?;
        let mut values = version_paths(&self.versions_directory(id))?
            .into_iter()
            .filter_map(|path| {
                path.file_stem()
                    .and_then(|value| value.to_str())
                    .map(|created_at| PersonaVersion {
                        id: created_at.to_owned(),
                        created_at: created_at.to_owned(),
                    })
            })
            .collect::<Vec<_>>();
        values.sort_by(|left, right| version_order(&right.id).cmp(&version_order(&left.id)));
        Ok(values)
    }

    pub fn restore_version(
        &self,
        id: &str,
        version: &str,
        timestamp: &str,
    ) -> Result<(), PersonaStoreError> {
        validate_id(id)?;
        validate_version_id(version)?;
        let _guard = SiblingLock::acquire(&self.lock_path())?;
        let path = self.user_path(id);
        if !path.is_file() {
            return Err(PersonaStoreError::NotCustom(id.to_owned()));
        }
        let version_path = self.versions_directory(id).join(format!("{version}.md"));
        let body = fs::read(&version_path)?;
        self.archive_current(id, timestamp)?;
        atomic_write_bytes(&path, &body)?;
        self.prune_versions(id)?;
        Ok(())
    }

    fn load_names(&self) -> Result<BTreeMap<String, String>, PersonaStoreError> {
        let _guard = SiblingLock::acquire(&self.lock_path())?;
        self.load_names_unlocked()
    }

    fn load_names_unlocked(&self) -> Result<BTreeMap<String, String>, PersonaStoreError> {
        match fs::read(self.names_path()) {
            Ok(bytes) => Ok(serde_json::from_slice::<NamesDocument>(&bytes)?.names),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(error.into()),
        }
    }

    fn archive_current(&self, id: &str, timestamp: &str) -> Result<(), PersonaStoreError> {
        validate_version_id(timestamp)?;
        let directory = self.versions_directory(id);
        fs::create_dir_all(&directory)?;
        set_private_directory_mode(&directory)?;
        let body = fs::read(self.user_path(id))?;
        let mut suffix = 0_u64;
        let path = loop {
            let version = if suffix == 0 {
                timestamp.to_owned()
            } else {
                format!("{timestamp}-{suffix:04}")
            };
            let candidate = directory.join(format!("{version}.md"));
            if !candidate.exists() {
                break candidate;
            }
            suffix = suffix.saturating_add(1);
        };
        atomic_write_bytes(&path, &body)?;
        Ok(())
    }

    fn prune_versions(&self, id: &str) -> Result<(), PersonaStoreError> {
        let paths = version_paths(&self.versions_directory(id))?;
        let remove_count = paths.len().saturating_sub(VERSION_LIMIT);
        for path in paths.into_iter().take(remove_count) {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    fn user_path(&self, id: &str) -> PathBuf {
        self.user_directory.join(format!("{id}.md"))
    }

    fn builtin_path(&self, id: &str) -> PathBuf {
        self.builtin_directory.join(format!("{id}.md"))
    }

    fn versions_directory(&self, id: &str) -> PathBuf {
        self.user_directory.join(format!("{id}.versions"))
    }

    fn names_path(&self) -> PathBuf {
        self.user_directory.join("persona-names.json")
    }

    fn lock_path(&self) -> PathBuf {
        self.user_directory.join(".personas.lock")
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NamesDocument {
    names: BTreeMap<String, String>,
}

fn markdown_ids(directory: &Path) -> Result<BTreeSet<String>, PersonaStoreError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error.into()),
    };
    Ok(entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
        .filter_map(|path| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(str::to_owned)
        })
        .collect())
}

fn version_paths(directory: &Path) -> Result<Vec<PathBuf>, PersonaStoreError> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        let left = left
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let right = right
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        version_order(left).cmp(&version_order(right))
    });
    Ok(paths)
}

fn version_order(value: &str) -> (&str, u64) {
    let Some((timestamp, suffix)) = value.rsplit_once('-') else {
        return (value, 0);
    };
    if suffix.is_empty() || !suffix.bytes().all(|value| value.is_ascii_digit()) {
        return (value, 0);
    }
    suffix
        .parse::<u64>()
        .map_or((value, 0), |suffix| (timestamp, suffix))
}

fn validate_id(id: &str) -> Result<(), PersonaStoreError> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Err(PersonaStoreError::InvalidId);
    }
    Ok(())
}

fn validate_version_id(value: &str) -> Result<(), PersonaStoreError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(PersonaStoreError::InvalidVersion);
    }
    Ok(())
}

fn validate_display_name(value: &str) -> Result<(), PersonaStoreError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 40 || trimmed.chars().any(char::is_control) {
        return Err(PersonaStoreError::InvalidDisplayName);
    }
    Ok(())
}

fn validate_body(value: &str) -> Result<(), PersonaStoreError> {
    if value.trim().is_empty() || value.len() > 256 * 1024 {
        return Err(PersonaStoreError::InvalidBody);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum PersonaStoreError {
    #[error("組み込み persona の場所がありません")]
    MissingBuiltinDirectory,
    #[error("性格 ID は英数字とハイフンの1〜64文字で指定してください")]
    InvalidId,
    #[error("性格の表示名は1〜40文字で指定してください")]
    InvalidDisplayName,
    #[error("性格の本文は1 byte以上256 KiB以下で指定してください")]
    InvalidBody,
    #[error("版 ID が不正です")]
    InvalidVersion,
    #[error("組み込みの性格は上書きできません: {0}")]
    BuiltinReadOnly(String),
    #[error("自分で作った性格ではありません: {0}")]
    NotCustom(String),
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

