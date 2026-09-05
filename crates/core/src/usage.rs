use crate::config::local_date;
use crate::persistence::{atomic_write_json, PersistenceError, SiblingLock};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ObserverUsage {
    pub date: String,
    pub ai_calls: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompanionUsage {
    pub date: String,
    pub total_calls: u32,
    pub proactive_calls: u32,
    #[serde(default)]
    pub proactive_emit_ids: Vec<String>,
    pub session_summaries: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_limit_notice_date: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionCallKind {
    User,
    Proactive,
    SessionSummary,
}

#[derive(Debug, Error)]
pub enum UsageError {
    #[error("usage I/O に失敗しました: {0}")]
    Io(#[from] io::Error),
    #[error("usage の JSON が不正です: {0}")]
    Json(#[from] serde_json::Error),
    #[error("usage の lock を取得できません: {0}")]
    Lock(#[from] PersistenceError),
}

pub fn is_proactive_limit_reached(proactive_calls: u32, limit: Option<u32>) -> bool {
    limit.is_some_and(|limit| proactive_calls >= limit)
}

pub fn load_observer(path: &Path, date: &str) -> Result<ObserverUsage, UsageError> {
    let lock_path = lock_path(path);
    let _lock = SiblingLock::acquire(&lock_path)?;
    let current = read_observer(path)?;
    if current.date == date {
        return Ok(current);
    }
    let next = ObserverUsage {
        date: date.to_owned(),
        ai_calls: 0,
    };
    write_snapshot(path, &next)?;
    Ok(next)
}

pub fn record_observer_attempt(path: &Path, date: &str) -> Result<ObserverUsage, UsageError> {
    let lock_path = lock_path(path);
    let _lock = SiblingLock::acquire(&lock_path)?;
    let current = read_observer(path)?;
    let next = if current.date == date {
        ObserverUsage {
            date: date.to_owned(),
            ai_calls: current.ai_calls.saturating_add(1),
        }
    } else {
        ObserverUsage {
            date: date.to_owned(),
            ai_calls: 1,
        }
    };
    write_snapshot(path, &next)?;
    Ok(next)
}

/// 上限確認と observer 呼び出しの予約を同じ lock の中で行う。
pub fn try_reserve_observer(
    path: &Path,
    date: &str,
    limit: u32,
) -> Result<Option<ObserverUsage>, UsageError> {
    let lock_path = lock_path(path);
    let _lock = SiblingLock::acquire(&lock_path)?;
    let current = read_observer(path)?;
    let day_changed = current.date != date;
    let next = if current.date == date {
        current
    } else {
        ObserverUsage {
            date: date.to_owned(),
            ai_calls: 0,
        }
    };
    if next.ai_calls >= limit {
        if day_changed {
            write_snapshot(path, &next)?;
        }
        return Ok(None);
    }
    let reserved = ObserverUsage {
        date: date.to_owned(),
        ai_calls: next.ai_calls.saturating_add(1),
    };
    write_snapshot(path, &reserved)?;
    Ok(Some(reserved))
}

pub fn load_companion(path: &Path, date: &str) -> Result<CompanionUsage, UsageError> {
    let lock_path = lock_path(path);
    let _lock = SiblingLock::acquire(&lock_path)?;
    let current = read_companion(path)?;
    if current.date == date {
        return Ok(current);
    }
    let next = companion_for_date(current, date);
    write_snapshot(path, &next)?;
    Ok(next)
}

pub fn try_record_limit_notice(path: &Path, date: &str) -> Result<bool, UsageError> {
    let lock_path = lock_path(path);
    let _lock = SiblingLock::acquire(&lock_path)?;
    let current = read_companion(path)?;
    let day_changed = current.date != date;
    let mut next = companion_for_date(current, date);
    if next.last_limit_notice_date.as_deref() == Some(date) {
        if day_changed {
            write_snapshot(path, &next)?;
        }
        return Ok(false);
    }
    next.last_limit_notice_date = Some(date.to_owned());
    write_snapshot(path, &next)?;
    Ok(true)
}

pub fn record_companion_attempt(
    path: &Path,
    date: &str,
    kind: CompanionCallKind,
) -> Result<CompanionUsage, UsageError> {
    let lock_path = lock_path(path);
    let _lock = SiblingLock::acquire(&lock_path)?;
    let current = read_companion(path)?;
    let mut next = companion_for_date(current, date);
    next.total_calls = next.total_calls.saturating_add(1);
    match kind {
        CompanionCallKind::User => {}
        CompanionCallKind::Proactive => {}
        CompanionCallKind::SessionSummary => {
            next.session_summaries = next.session_summaries.saturating_add(1)
        }
    }
    write_snapshot(path, &next)?;
    Ok(next)
}

/// 実際に発話する自発応答だけを、上限確認と同じ lock 内で加算する。
pub fn try_record_proactive_emit(
    path: &Path,
    date: &str,
    proactive_limit: Option<u32>,
    emit_id: &str,
) -> Result<Option<CompanionUsage>, UsageError> {
    let lock_path = lock_path(path);
    let _lock = SiblingLock::acquire(&lock_path)?;
    let current = read_companion(path)?;
    let day_changed = current.date != date;
    let mut next = companion_for_date(current, date);
    if next.proactive_emit_ids.iter().any(|id| id == emit_id) {
        if day_changed {
            write_snapshot(path, &next)?;
        }
        return Ok(Some(next));
    }
    if proactive_limit.is_some_and(|limit| next.proactive_calls >= limit) {
        if day_changed {
            write_snapshot(path, &next)?;
        }
        return Ok(None);
    }
    next.proactive_calls = next.proactive_calls.saturating_add(1);
    next.proactive_emit_ids.push(emit_id.to_owned());
    write_snapshot(path, &next)?;
    Ok(Some(next))
}

pub fn forget_proactive_emit_id(path: &Path, emit_id: &str) -> Result<(), UsageError> {
    let lock_path = lock_path(path);
    let _lock = SiblingLock::acquire(&lock_path)?;
    let mut current = read_companion(path)?;
    let before = current.proactive_emit_ids.len();
    current.proactive_emit_ids.retain(|id| id != emit_id);
    if current.proactive_emit_ids.len() != before {
        write_snapshot(path, &current)?;
    }
    Ok(())
}

fn read_observer(path: &Path) -> Result<ObserverUsage, UsageError> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ObserverUsage {
            date: String::new(),
            ai_calls: 0,
        }),
        Err(error) => Err(error.into()),
    }
}

fn read_companion(path: &Path) -> Result<CompanionUsage, UsageError> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(empty_companion("")),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn quarantine_invalid_companion(path: &Path) -> Result<bool, UsageError> {
    let lock_path = lock_path(path);
    let _lock = SiblingLock::acquire(&lock_path)?;
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if serde_json::from_slice::<CompanionUsage>(&bytes).is_ok() {
        return Ok(false);
    }
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage の親 directory がありません",
        )
    })?;
    let failed = parent.join("failed");
    fs::create_dir_all(&failed)?;
    set_private_directory_mode(&failed)?;
    let target = failed.join(format!(
        "companion-usage-invalid-{}.json",
        uuid::Uuid::new_v4()
    ));
    fs::rename(path, target)?;
    Ok(true)
}

fn empty_companion(date: &str) -> CompanionUsage {
    CompanionUsage {
        date: date.to_owned(),
        total_calls: 0,
        proactive_calls: 0,
        proactive_emit_ids: Vec::new(),
        session_summaries: 0,
        last_limit_notice_date: None,
    }
}

fn companion_for_date(current: CompanionUsage, date: &str) -> CompanionUsage {
    if current.date == date {
        return current;
    }
    CompanionUsage {
        last_limit_notice_date: current.last_limit_notice_date,
        proactive_emit_ids: current.proactive_emit_ids,
        ..empty_companion(date)
    }
}

fn write_snapshot<T: Serialize>(path: &Path, value: &T) -> Result<(), UsageError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        set_private_directory_mode(parent)?;
    }
    atomic_write_json(path, value)?;
    Ok(())
}

fn lock_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        ".{}.lock",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("usage")
    ))
}

fn set_private_directory_mode(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn today_observer_usage(path: &Path) -> Result<ObserverUsage, UsageError> {
    load_observer(path, &local_date())
}

