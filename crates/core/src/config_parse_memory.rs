use super::{boolean, issue, nonnegative_u32, positive_u64, positive_usize, unknown_keys};
use crate::config::{Config, ConfigError, ConfigValidationIssue, MemoryConfig};
use serde_json::{Map, Value};

pub(super) fn parse_memory(
    object: &Map<String, Value>,
    issues: &mut Vec<ConfigValidationIssue>,
) -> MemoryConfig {
    issues.extend(unknown_keys(
        object,
        &[
            "enabled",
            "providerConsent",
            "graceMinutes",
            "dailyRetentionDays",
            "weeklyRetentionWeeks",
            "jobRetentionDays",
            "sourceMaxBytes",
            "promptMaxBytes",
            "factLimit",
            "factMaxBytes",
            "candidateLimit",
            "candidateMaxBytes",
            "storageMaxBytes",
            "factPromptDailyLimit",
        ],
        "memory",
    ));
    let grace_minutes = match object.get("graceMinutes") {
        None => 60,
        Some(value) => match value.as_u64() {
            Some(value) => value,
            None => {
                issues.push(issue(
                    "memory.graceMinutes",
                    "0以上1440以下の整数で指定してください。",
                ));
                60
            }
        },
    };
    let config = MemoryConfig {
        enabled: boolean(object, "enabled", false, "memory.enabled", issues),
        provider_consent: boolean(
            object,
            "providerConsent",
            false,
            "memory.providerConsent",
            issues,
        ),
        grace_minutes,
        daily_retention_days: positive_u64(
            object,
            "dailyRetentionDays",
            90,
            "memory.dailyRetentionDays",
            issues,
        ),
        weekly_retention_weeks: positive_u64(
            object,
            "weeklyRetentionWeeks",
            52,
            "memory.weeklyRetentionWeeks",
            issues,
        ),
        job_retention_days: positive_u64(
            object,
            "jobRetentionDays",
            30,
            "memory.jobRetentionDays",
            issues,
        ),
        source_max_bytes: positive_usize(
            object,
            "sourceMaxBytes",
            204_800,
            "memory.sourceMaxBytes",
            issues,
        ),
        prompt_max_bytes: positive_usize(
            object,
            "promptMaxBytes",
            16_384,
            "memory.promptMaxBytes",
            issues,
        ),
        fact_limit: positive_usize(object, "factLimit", 1_000, "memory.factLimit", issues),
        fact_max_bytes: positive_usize(
            object,
            "factMaxBytes",
            2_097_152,
            "memory.factMaxBytes",
            issues,
        ),
        candidate_limit: positive_usize(
            object,
            "candidateLimit",
            50,
            "memory.candidateLimit",
            issues,
        ),
        candidate_max_bytes: positive_usize(
            object,
            "candidateMaxBytes",
            65_536,
            "memory.candidateMaxBytes",
            issues,
        ),
        storage_max_bytes: positive_usize(
            object,
            "storageMaxBytes",
            10_485_760,
            "memory.storageMaxBytes",
            issues,
        ),
        fact_prompt_daily_limit: nonnegative_u32(
            object,
            "factPromptDailyLimit",
            3,
            "memory.factPromptDailyLimit",
            issues,
        ),
    };
    if let Err(ConfigError::Validation(validation)) = crate::config::validate_config(&Config {
        memory: config.clone(),
        ..Config::default()
    }) {
        issues.extend(
            validation
                .into_iter()
                .filter(|issue| issue.path.starts_with("memory.")),
        );
    }
    config
}
