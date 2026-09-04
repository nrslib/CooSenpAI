use super::{
    build_memory_block, select_schedule, DailySummary, FactRecord, FactStore, JobFailureKind,
    MemoryContext, MemoryFactError, MemoryJob, MemoryJobKind, MemoryJobPhase, MemoryRetrievalInput,
    MemorySearchRecord, MemoryServiceError, MemoryStore, ScheduleInput, SummaryState,
    WeeklySummary,
};
use super::{canonical_daily_source, memory_job_id, SourceInput, SourceInputKind};
use crate::config::ConfigPaths;
use crate::provider::{
    ProviderCall, ProviderClient, ProviderError, ProviderErrorKind, ProviderResult,
};
use async_trait::async_trait;
use chrono::{NaiveDate, TimeZone, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Expected {
    source_digest: String,
    source_ids: Vec<String>,
    truncated: bool,
    skipped_invalid_count: usize,
    job_id: String,
}

#[test]
fn daily_source_and_job_id_match_byte_golden() {
    let fixture = fixture_directory();
    let conversation = lines(&fixture.join("conversation.jsonl"));
    let observations = lines(&fixture.join("observations.jsonl"));
    let inputs = source_inputs(&conversation, &observations);
    let snapshot = canonical_daily_source(&inputs, 204_800).expect("canonical source");
    let expected_bytes = fs::read(fixture.join("canonical.jsonl")).expect("canonical fixture");
    let expected: Expected =
        serde_json::from_slice(&fs::read(fixture.join("expected.json")).expect("expected fixture"))
            .expect("expected json");

    assert_eq!(snapshot.canonical_bytes, expected_bytes);
    assert_eq!(snapshot.source_digest, expected.source_digest);
    assert_eq!(snapshot.source_ids, expected.source_ids);
    assert_eq!(snapshot.truncated, expected.truncated);
    assert_eq!(
        snapshot.skipped_invalid_count,
        expected.skipped_invalid_count
    );
    assert_eq!(
        memory_job_id(
            "daily",
            "2026-08-27",
            &snapshot.source_digest,
            1,
            "codex",
            "default",
        ),
        expected.job_id
    );
}

#[test]
fn source_limit_omits_whole_oldest_records() {
    let fixture = fixture_directory();
    let conversation = lines(&fixture.join("conversation.jsonl"));
    let observations = lines(&fixture.join("observations.jsonl"));
    let inputs = source_inputs(&conversation, &observations);
    let full = canonical_daily_source(&inputs, usize::MAX).expect("full source");
    let first_record_bytes = full
        .canonical_bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .map(|index| index + 1)
        .expect("first newline");

    let truncated =
        canonical_daily_source(&inputs, full.canonical_bytes.len() - first_record_bytes)
            .expect("truncated source");

    assert!(truncated.truncated);
    assert_eq!(truncated.source_ids, ["u2", "o1", "o2"]);
    assert!(!truncated.canonical_bytes.ends_with(b"\n\n"));
    assert!(truncated.canonical_bytes.ends_with(b"\n"));
}

fn fixture_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/memory")
}

fn lines(path: &std::path::Path) -> Vec<Vec<u8>> {
    fs::read(path)
        .expect("fixture")
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

fn source_inputs<'a>(
    conversation: &'a [Vec<u8>],
    observations: &'a [Vec<u8>],
) -> Vec<SourceInput<'a>> {
    conversation
        .iter()
        .map(|line| SourceInput {
            kind: SourceInputKind::Conversation,
            line,
        })
        .chain(observations.iter().map(|line| SourceInput {
            kind: SourceInputKind::Observation,
            line,
        }))
        .collect()
}

#[test]
fn crash_recovery_matches_quint_phases_at_each_failpoint() {
    let directory = tempfile::tempdir().expect("tempdir");
    let paths = ConfigPaths::from_root(directory.path().join(".coosenpai"));
    crate::config::ensure_layout(&paths).expect("layout");
    let store = MemoryStore::new(paths);

    let reserved = job(MemoryJobPhase::Reserved, "2026-08-28");
    store.save_job(&reserved).expect("reserved");
    let mut recovered = store
        .load_job(MemoryJobKind::Daily, "2026-08-27")
        .expect("load")
        .expect("job");
    recovered.recover_after_crash("2026-08-28", "2026-08-28T01:01:00Z");
    assert_eq!(recovered.phase, MemoryJobPhase::Reserved);

    recovered.mark_calling("2026-08-28T01:02:00Z");
    store.save_job(&recovered).expect("calling");
    let mut recovered = store
        .load_job(MemoryJobKind::Daily, "2026-08-27")
        .expect("load")
        .expect("job");
    recovered.recover_after_crash("2026-08-28", "2026-08-28T01:03:00Z");
    assert_eq!(recovered.phase, MemoryJobPhase::Failed);
    assert_eq!(recovered.failure_kind, Some(JobFailureKind::Indeterminate));

    let mut generated = job(MemoryJobPhase::Calling, "2026-08-28");
    generated.mark_generated("durable text".to_owned(), "2026-08-28T01:04:00Z");
    store.save_job(&generated).expect("generated");
    let mut recovered = store
        .load_job(MemoryJobKind::Daily, "2026-08-27")
        .expect("load")
        .expect("job");
    recovered.recover_after_crash("2026-08-29", "2026-08-29T01:00:00Z");
    assert_eq!(recovered.phase, MemoryJobPhase::Generated);
    assert_eq!(recovered.generated_text.as_deref(), Some("durable text"));
}

#[test]
fn scheduler_handles_backlog_week_boundary_dst_and_existing_slots() {
    let local_now = NaiveDate::from_ymd_opt(2026, 3, 9)
        .expect("date")
        .and_hms_opt(3, 0, 0)
        .expect("time");
    let schedule = select_schedule(&ScheduleInput {
        local_now,
        grace_minutes: 60,
        available_daily_periods: dates(&["2026-03-05", "2026-03-07", "2026-03-08"]),
        current_daily_periods: dates(&["2026-03-07"]),
        failed_or_stale_daily_periods: dates(&["2026-03-07"]),
        daily_jobs_today: Vec::new(),
        weekly_jobs_this_week: Vec::new(),
        stale_weekly_periods: vec!["2026-W08".to_owned()],
    });

    assert_eq!(schedule.daily, ["2026-03-08", "2026-03-05"]);
    assert_eq!(schedule.weekly, ["2026-W10", "2026-W08"]);
    assert_eq!(schedule.delayed_daily, 1);

    let already_run = select_schedule(&ScheduleInput {
        local_now,
        grace_minutes: 60,
        available_daily_periods: dates(&["2026-03-08"]),
        current_daily_periods: Vec::new(),
        failed_or_stale_daily_periods: Vec::new(),
        daily_jobs_today: vec!["2026-03-08".to_owned()],
        weekly_jobs_this_week: vec!["2026-W10".to_owned()],
        stale_weekly_periods: Vec::new(),
    });
    assert!(already_run.daily.is_empty());
    assert!(already_run.weekly.is_empty());
}

#[test]
fn scheduler_grace_can_cross_midnight_without_duplicate_periods() {
    let before = NaiveDate::from_ymd_opt(2026, 8, 29)
        .expect("date")
        .and_hms_opt(23, 59, 0)
        .expect("time");
    let after = NaiveDate::from_ymd_opt(2026, 8, 30)
        .expect("date")
        .and_hms_opt(0, 0, 0)
        .expect("time");
    let input = |local_now| ScheduleInput {
        local_now,
        grace_minutes: 1_440,
        available_daily_periods: dates(&["2026-08-28", "2026-08-29"]),
        current_daily_periods: Vec::new(),
        failed_or_stale_daily_periods: Vec::new(),
        daily_jobs_today: Vec::new(),
        weekly_jobs_this_week: Vec::new(),
        stale_weekly_periods: Vec::new(),
    };
    assert!(select_schedule(&input(before)).daily.is_empty());
    assert_eq!(select_schedule(&input(after)).daily, ["2026-08-28"]);
}

pub(super) fn job(phase: MemoryJobPhase, day: &str) -> MemoryJob {
    MemoryJob {
        schema_version: 1,
        job_id: "job-id".to_owned(),
        kind: MemoryJobKind::Daily,
        period: "2026-08-27".to_owned(),
        day: day.to_owned(),
        phase,
        source_digest: "digest".to_owned(),
        source_ids: vec!["source".to_owned()],
        source_truncated: false,
        skipped_invalid_count: 0,
        prompt_version: 1,
        provider: "codex".to_owned(),
        model: "default".to_owned(),
        created_at: "2026-08-28T01:00:00Z".to_owned(),
        updated_at: "2026-08-28T01:00:00Z".to_owned(),
        generated_text: None,
        failure_kind: None,
    }
}

fn dates(values: &[&str]) -> Vec<NaiveDate> {
    values
        .iter()
        .map(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").expect("date"))
        .collect()
}

#[test]
fn fact_candidates_require_user_provenance_and_confirmation_is_idempotent() {
    let directory = tempfile::tempdir().expect("tempdir");
    let paths = ConfigPaths::from_root(directory.path().join(".coosenpai"));
    crate::config::ensure_layout(&paths).expect("layout");
    let store = FactStore::new(paths);
    let config = crate::config::MemoryConfig::default();
    let allowed = HashSet::from(["runtime-user-1".to_owned()]);

    let rejected = store.validate_candidates(
        &serde_json::json!([{
            "text": "観察だけを根拠にした候補",
            "sourceUserMessageIds": ["observation-1"]
        }]),
        &serde_json::json!([]),
        &allowed,
        "2026-08-28T01:00:00Z",
        &config,
    );
    assert!(matches!(rejected, Err(MemoryFactError::Validation { .. })));

    let snapshot = store
        .validate_candidates(
            &serde_json::json!([{
                "text": "ユーザーは Rust を使っている",
                "sourceUserMessageIds": ["runtime-user-1"]
            }]),
            &serde_json::json!([]),
            &allowed,
            "2026-08-28T01:00:00Z",
            &config,
        )
        .expect("candidate");
    let candidate_id = snapshot.candidates[0].id.clone();
    let first = store
        .confirm(
            &candidate_id,
            "confirmation-1",
            "2026-08-28T01:01:00Z",
            &config,
        )
        .expect("confirm");
    let second = store
        .confirm(
            &candidate_id,
            "confirmation-1",
            "2026-08-28T01:01:00Z",
            &config,
        )
        .expect("idempotent confirm");
    assert_eq!(second.id, first.id);
    assert_eq!(store.active_facts().expect("facts").len(), 1);
    assert_eq!(
        store
            .active_facts()
            .expect("facts")
            .get(&first.id)
            .and_then(|record| record.text.as_deref()),
        Some("ユーザーは Rust を使っている")
    );
}

#[test]
fn fact_limit_refuses_new_data_and_compaction_preserves_tombstones_semantics() {
    let directory = tempfile::tempdir().expect("tempdir");
    let paths = ConfigPaths::from_root(directory.path().join(".coosenpai"));
    crate::config::ensure_layout(&paths).expect("layout");
    let facts_path = paths.memory_facts.clone();
    let store = FactStore::new(paths);
    let config = crate::config::MemoryConfig {
        fact_limit: 1,
        ..crate::config::MemoryConfig::default()
    };
    let allowed = HashSet::from(["runtime-user-1".to_owned()]);

    let candidate_ids = ["一つ目", "二つ目"]
        .into_iter()
        .map(|text| {
            store
                .validate_candidates(
                    &serde_json::json!([{
                        "text": text,
                        "sourceUserMessageIds": ["runtime-user-1"]
                    }]),
                    &serde_json::json!([]),
                    &allowed,
                    "2026-08-28T01:00:00Z",
                    &config,
                )
                .expect("candidate")
                .candidates
                .into_iter()
                .find(|candidate| candidate.text == text)
                .expect("created candidate")
                .id
        })
        .collect::<Vec<_>>();
    let first = store
        .confirm(
            &candidate_ids[0],
            "confirmation-1",
            "2026-08-28T01:01:00Z",
            &config,
        )
        .expect("first fact");
    assert!(matches!(
        store.confirm(
            &candidate_ids[1],
            "confirmation-2",
            "2026-08-28T01:02:00Z",
            &config,
        ),
        Err(MemoryFactError::Capacity)
    ));
    store
        .delete(&first.id, "delete-1", "2026-08-28T01:03:00Z")
        .expect("delete");
    assert!(store.active_facts().expect("facts").is_empty());
    store.compact().expect("compact");
    assert!(store.active_facts().expect("facts").is_empty());
    assert_eq!(fs::read_to_string(facts_path).expect("facts file"), "");
}

#[test]
fn deterministic_retrieval_matches_byte_golden_and_filters_stale_data() {
    let daily_text = "Rust の設計方針を決めた。".to_owned();
    let daily_text_digest = format!("{:x}", Sha256::digest(daily_text.as_bytes()));
    let daily = vec![DailySummary {
        schema_version: 1,
        local_date: "2026-08-27".to_owned(),
        time_zone_id: "Asia/Tokyo".to_owned(),
        source_digest: "source".to_owned(),
        source_ids: vec!["search-1".to_owned()],
        truncated: false,
        prompt_version: 1,
        provider: "codex".to_owned(),
        model: "default".to_owned(),
        generated_at: "2026-08-28T01:00:00Z".to_owned(),
        text: daily_text,
        text_digest: daily_text_digest.clone(),
        state: SummaryState::Current,
    }];
    let stale_daily = DailySummary {
        local_date: "2026-08-26".to_owned(),
        state: SummaryState::Stale,
        text: "添付してはいけない".to_owned(),
        ..daily[0].clone()
    };
    let facts = vec![FactRecord {
        schema_version: 1,
        id: "fact-1".to_owned(),
        text: Some("ユーザーは Rust を使う".to_owned()),
        source_user_message_ids: vec!["runtime-user-1".to_owned()],
        confirmation_id: "confirmation-1".to_owned(),
        confirmed_at: "2026-08-27T01:00:00Z".to_owned(),
        tombstone: false,
    }];
    let (weekly_source, weekly_dependencies) =
        super::canonical::canonical_weekly_source(&daily, "2026-W35", 204_800)
            .expect("weekly source");
    let weekly_text = "Rust の実装を継続している。".to_owned();
    let weekly = vec![WeeklySummary {
        schema_version: 1,
        period: "2026-W35".to_owned(),
        time_zone_id: "Asia/Tokyo".to_owned(),
        source_digest: weekly_source.source_digest,
        source_ids: weekly_source.source_ids,
        truncated: weekly_source.truncated,
        prompt_version: 1,
        provider: "codex".to_owned(),
        model: "default".to_owned(),
        generated_at: "2026-08-24T02:00:00Z".to_owned(),
        text_digest: format!("{:x}", Sha256::digest(weekly_text.as_bytes())),
        text: weekly_text,
        state: SummaryState::Current,
        depends_on: weekly_dependencies,
    }];
    let search = vec![
        MemorySearchRecord {
            id: "search-1".to_owned(),
            kind: "conversation".to_owned(),
            created_at: "2026-08-27T12:00:00Z".to_owned(),
            text: "Rust の設計を相談した".to_owned(),
        },
        MemorySearchRecord {
            id: "recent-1".to_owned(),
            kind: "conversation".to_owned(),
            created_at: "2026-08-28T00:00:00Z".to_owned(),
            text: "Rust の直近会話".to_owned(),
        },
    ];
    let mut all_daily = daily;
    all_daily.push(stale_daily);
    let block = build_memory_block(&MemoryRetrievalInput {
        query: "Ｒｕｓｔ 設計",
        today: NaiveDate::from_ymd_opt(2026, 8, 28).expect("date"),
        search_records: &search,
        daily: &all_daily,
        facts: &facts,
        weekly: &weekly,
        recent_ids: &HashSet::from(["recent-1".to_owned()]),
        source_max_bytes: 204_800,
        max_bytes: 16_384,
    })
    .expect("memory block");

    assert_eq!(
        block.serialized.as_bytes(),
        fs::read(fixture_directory().join("retrieval.txt"))
            .expect("retrieval fixture")
            .as_slice()
    );
    assert_eq!(
        block.included_ids,
        ["search-1", "2026-08-27", "fact-1", "2026-W35"]
    );
    assert_eq!(block.used_fact_ids, ["fact-1"]);
    assert!(block.serialized.len() <= 16_384);
    assert!(!block.serialized.contains("添付してはいけない"));
    assert!(!block.serialized.contains("直近会話"));

    let extra_text = "追加の日次要約".to_owned();
    let extra_daily = DailySummary {
        local_date: "2026-08-26".to_owned(),
        text_digest: format!("{:x}", Sha256::digest(extra_text.as_bytes())),
        text: extra_text,
        state: SummaryState::Current,
        ..all_daily[0].clone()
    };
    all_daily.push(extra_daily);
    let without_outdated_weekly = build_memory_block(&MemoryRetrievalInput {
        query: "unmatched",
        today: NaiveDate::from_ymd_opt(2026, 8, 28).expect("date"),
        search_records: &[],
        daily: &all_daily,
        facts: &[],
        weekly: &weekly,
        recent_ids: &HashSet::new(),
        source_max_bytes: 204_800,
        max_bytes: 16_384,
    })
    .expect("memory block");
    assert!(!without_outdated_weekly
        .serialized
        .contains("Rust の実装を継続している。"));
}

#[test]
fn memory_context_requires_consent_and_rechecks_available_source_digest() {
    let directory = tempfile::tempdir().expect("tempdir");
    let paths = ConfigPaths::from_root(directory.path().join(".coosenpai"));
    crate::config::ensure_layout(&paths).expect("layout");
    let period = "2026-08-27";
    fs::write(
        paths.conversation.join(format!("{period}.jsonl")),
        "{\"schemaVersion\":1,\"id\":\"user-1\",\"createdAt\":\"2026-08-27T01:00:00Z\",\"role\":\"user\",\"message\":\"変更後\"}\n",
    )
    .expect("conversation");
    MemoryStore::new(paths.clone())
        .save_daily(&DailySummary {
            schema_version: 1,
            local_date: period.to_owned(),
            time_zone_id: "Asia/Tokyo".to_owned(),
            source_digest: "old-source".to_owned(),
            source_ids: vec!["user-1".to_owned()],
            truncated: false,
            prompt_version: 1,
            provider: "codex".to_owned(),
            model: "default".to_owned(),
            generated_at: "2026-08-28T01:00:00Z".to_owned(),
            text: "添付してはいけない古い要約".to_owned(),
            text_digest: "old-text".to_owned(),
            state: SummaryState::Current,
        })
        .expect("summary");
    let now = Utc
        .with_ymd_and_hms(2026, 8, 28, 2, 0, 0)
        .single()
        .expect("time");
    let disabled = MemoryContext::new(
        paths.clone(),
        crate::config::MemoryConfig {
            enabled: true,
            provider_consent: false,
            ..crate::config::MemoryConfig::default()
        },
    )
    .build("変更後", &HashSet::new(), now)
    .expect("disabled context");
    assert!(disabled.serialized.is_empty());

    let enabled = MemoryContext::new(
        paths,
        crate::config::MemoryConfig {
            enabled: true,
            provider_consent: true,
            ..crate::config::MemoryConfig::default()
        },
    )
    .build("一致しない", &HashSet::new(), now)
    .expect("enabled context");
    assert!(!enabled.serialized.contains("添付してはいけない古い要約"));
}

#[test]
fn fact_update_is_only_applied_after_confirmation_and_is_idempotent() {
    let directory = tempfile::tempdir().expect("tempdir");
    let paths = ConfigPaths::from_root(directory.path().join(".coosenpai"));
    crate::config::ensure_layout(&paths).expect("layout");
    let store = FactStore::new(paths);
    let config = crate::config::MemoryConfig::default();
    let allowed = HashSet::from(["user-1".to_owned()]);
    let candidate = store
        .validate_candidates(
            &serde_json::json!([{"text":"古い事実","sourceUserMessageIds":["user-1"]}]),
            &serde_json::json!([]),
            &allowed,
            "2026-08-28T01:00:00Z",
            &config,
        )
        .expect("candidate")
        .candidates[0]
        .clone();
    let fact = store
        .confirm(
            &candidate.id,
            "fact-confirmation",
            "2026-08-28T01:01:00Z",
            &config,
        )
        .expect("fact");
    let update = store
        .validate_candidates(
            &serde_json::json!([]),
            &serde_json::json!([{
                "operation":"rewrite",
                "factIds":[fact.id],
                "replacement":"新しい事実",
                "reason":"状況が変わった"
            }]),
            &allowed,
            "2026-08-28T01:02:00Z",
            &config,
        )
        .expect("update")
        .updates[0]
        .clone();
    assert_eq!(store.active_facts().expect("before").len(), 1);
    store
        .confirm_update(
            &update.id,
            "update-confirmation",
            "2026-08-28T01:03:00Z",
            &config,
        )
        .expect("apply");
    store
        .confirm_update(
            &update.id,
            "update-confirmation",
            "2026-08-28T01:03:00Z",
            &config,
        )
        .expect("idempotent apply");
    let active = store.active_facts().expect("after");
    assert_eq!(active.len(), 1);
    assert_eq!(
        active.values().next().and_then(|fact| fact.text.as_deref()),
        Some("新しい事実")
    );
}

#[test]
fn fact_candidate_unknown_keys_and_serialized_capacity_are_bounded() {
    let directory = tempfile::tempdir().expect("tempdir");
    let paths = ConfigPaths::from_root(directory.path().join(".coosenpai"));
    crate::config::ensure_layout(&paths).expect("layout");
    let store = FactStore::new(paths.clone());
    let allowed = HashSet::from(["user-1".to_owned()]);
    let config = crate::config::MemoryConfig {
        candidate_max_bytes: 4_096,
        ..crate::config::MemoryConfig::default()
    };
    assert!(matches!(
        store.validate_candidates(
            &serde_json::json!([{
                "text":"候補",
                "sourceUserMessageIds":["user-1"],
                "unexpected":true
            }]),
            &serde_json::json!([]),
            &allowed,
            "2026-08-28T01:00:00Z",
            &config,
        ),
        Err(MemoryFactError::Validation { .. })
    ));
    let candidates = (0..5)
        .map(|index| {
            serde_json::json!({
                "text": format!("{index}-{}", "あ".repeat(490)),
                "sourceUserMessageIds":["user-1"]
            })
        })
        .collect::<Vec<_>>();
    let saved = store
        .validate_candidates(
            &serde_json::Value::Array(candidates),
            &serde_json::json!([]),
            &allowed,
            "2026-08-28T01:00:00Z",
            &config,
        )
        .expect("bounded candidates");
    assert!(saved.candidates.len() < 5);
    assert!(
        fs::metadata(paths.memory_fact_candidates)
            .expect("candidate metadata")
            .len()
            <= 4_096
    );
}

struct SummaryProvider {
    calls: Arc<Mutex<usize>>,
    fail: bool,
}

#[async_trait]
impl ProviderClient for SummaryProvider {
    async fn call(
        &self,
        _input: ProviderCall,
        _cancellation: CancellationToken,
    ) -> Result<ProviderResult, ProviderError> {
        *self.calls.lock().expect("calls") += 1;
        if self.fail {
            return Err(ProviderError {
                kind: ProviderErrorKind::Retryable,
                message: "temporary".to_owned(),
            });
        }
        Ok(ProviderResult {
            text: String::new(),
            value: Some(serde_json::json!({"text":"昨日は Rust の実装を進めた。"})),
            session: None,
        })
    }
}

#[tokio::test]
async fn service_commits_once_and_does_not_retry_a_failed_job_on_the_same_day() {
    let directory = tempfile::tempdir().expect("tempdir");
    let paths = ConfigPaths::from_root(directory.path().join(".coosenpai"));
    crate::config::ensure_layout(&paths).expect("layout");
    let period = chrono::Local::now()
        .date_naive()
        .pred_opt()
        .expect("previous day")
        .to_string();
    fs::write(
        paths.conversation.join(format!("{period}.jsonl")),
        format!(
            "{{\"schemaVersion\":1,\"id\":\"user-memory\",\"createdAt\":\"{period}T01:00:00+09:00\",\"role\":\"user\",\"message\":\"Rust の実装\",\"notificationPriority\":\"none\"}}\n"
        ),
    )
    .expect("conversation");
    let config = crate::config::MemoryConfig {
        enabled: true,
        provider_consent: true,
        ..crate::config::MemoryConfig::default()
    };
    let calls = Arc::new(Mutex::new(0));
    let mut service = super::MemoryService::new(
        Arc::new(SummaryProvider {
            calls: calls.clone(),
            fail: false,
        }),
        MemoryStore::new(paths.clone()),
        config.clone(),
        crate::config::CompanionConfig::default(),
    );
    service
        .consolidate(&period, CancellationToken::new())
        .await
        .expect("consolidate");
    service
        .consolidate(&period, CancellationToken::new())
        .await
        .expect("idempotent");
    assert_eq!(*calls.lock().expect("calls"), 1);

    let failed_period = chrono::Local::now()
        .date_naive()
        .checked_sub_days(chrono::Days::new(2))
        .expect("older day")
        .to_string();
    fs::write(
        paths.conversation.join(format!("{failed_period}.jsonl")),
        format!(
            "{{\"schemaVersion\":1,\"id\":\"user-failed\",\"createdAt\":\"{failed_period}T01:00:00+09:00\",\"role\":\"user\",\"message\":\"失敗境界\",\"notificationPriority\":\"none\"}}\n"
        ),
    )
    .expect("conversation");
    let failed_calls = Arc::new(Mutex::new(0));
    let mut failed = super::MemoryService::new(
        Arc::new(SummaryProvider {
            calls: failed_calls.clone(),
            fail: true,
        }),
        MemoryStore::new(paths),
        config,
        crate::config::CompanionConfig::default(),
    );
    assert!(failed
        .consolidate(&failed_period, CancellationToken::new())
        .await
        .is_err());
    assert!(matches!(
        failed
            .consolidate(&failed_period, CancellationToken::new())
            .await,
        Err(MemoryServiceError::DeferredAfterFailure)
    ));
    assert_eq!(*failed_calls.lock().expect("calls"), 1);
}
