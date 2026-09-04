use super::*;
use crate::persistence::atomic_write_bytes;
use chrono::DateTime;
use sha2::{Digest, Sha256};

const SUMMARY_TEXT_MAX_BYTES: usize = 16 * 1_024;

pub(super) fn load_daily(
    store: &MemoryStore,
    period: &str,
) -> Result<Option<DailySummary>, MemoryStoreError> {
    let path = store.paths.memory_daily.join(format!("{period}.json"));
    load_validated(store, &path, |summary: &DailySummary| {
        summary.schema_version == super::super::MEMORY_SCHEMA_VERSION
            && summary.local_date == period
            && summary_text_valid(&summary.text, &summary.text_digest)
            && metadata_valid(&summary.time_zone_id, &summary.generated_at)
    })
}

pub(super) fn daily_summaries(store: &MemoryStore) -> Result<Vec<DailySummary>, MemoryStoreError> {
    validated_directory(
        store,
        &store.paths.memory_daily,
        |path, summary: &DailySummary| {
            path.file_stem().and_then(|value| value.to_str()) == Some(&summary.local_date)
                && summary.schema_version == super::super::MEMORY_SCHEMA_VERSION
                && validate_daily_period(&summary.local_date).is_ok()
                && summary_text_valid(&summary.text, &summary.text_digest)
                && metadata_valid(&summary.time_zone_id, &summary.generated_at)
        },
    )
}

pub(super) fn weekly_summaries(
    store: &MemoryStore,
) -> Result<Vec<WeeklySummary>, MemoryStoreError> {
    validated_directory(
        store,
        &store.paths.memory_weekly,
        |path, summary: &WeeklySummary| {
            path.file_stem().and_then(|value| value.to_str()) == Some(&summary.period)
                && summary.schema_version == super::super::MEMORY_SCHEMA_VERSION
                && validate_weekly_period(&summary.period).is_ok()
                && summary_text_valid(&summary.text, &summary.text_digest)
                && metadata_valid(&summary.time_zone_id, &summary.generated_at)
        },
    )
}

fn validated_directory<T>(
    store: &MemoryStore,
    directory: &Path,
    valid: impl Fn(&Path, &T) -> bool,
) -> Result<Vec<T>, MemoryStoreError>
where
    T: serde::de::DeserializeOwned,
{
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    paths.sort();
    let mut summaries = Vec::new();
    for path in paths {
        if let Some(summary) = load_validated(store, &path, |summary| valid(&path, summary))? {
            summaries.push(summary);
        }
    }
    Ok(summaries)
}

fn load_validated<T>(
    store: &MemoryStore,
    path: &Path,
    valid: impl FnOnce(&T) -> bool,
) -> Result<Option<T>, MemoryStoreError>
where
    T: serde::de::DeserializeOwned,
{
    let _lock = SiblingLock::acquire(&job_lock_path(path))?;
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    match serde_json::from_slice::<T>(&bytes) {
        Ok(summary) if valid(&summary) => Ok(Some(summary)),
        Ok(_) | Err(_) => {
            quarantine(store, path, &bytes)?;
            Ok(None)
        }
    }
}

fn quarantine(store: &MemoryStore, path: &Path, bytes: &[u8]) -> Result<(), MemoryStoreError> {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("summary");
    let digest = format!("{:x}", Sha256::digest(bytes));
    let target = store
        .paths
        .memory_failed
        .join(format!("summary-{stem}-{digest}.json"));
    if !target.exists() {
        atomic_write_bytes(&target, bytes)?;
    }
    fs::remove_file(path)?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn summary_text_valid(text: &str, digest: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= SUMMARY_TEXT_MAX_BYTES
        && format!("{:x}", Sha256::digest(text.as_bytes())) == digest
}

fn metadata_valid(time_zone_id: &str, generated_at: &str) -> bool {
    !time_zone_id.trim().is_empty() && DateTime::parse_from_rfc3339(generated_at).is_ok()
}
