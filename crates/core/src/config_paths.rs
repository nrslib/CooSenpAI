use super::PRODUCT_DIR;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub root: PathBuf,
    pub config: PathBuf,
    pub personas: PathBuf,
    pub builtin_personas: Option<PathBuf>,
    pub builtin_tutorial: Option<PathBuf>,
    pub mailbox: PathBuf,
    pub state: PathBuf,
    pub avatar: PathBuf,
    pub onboarding: PathBuf,
    pub conversation_generation: PathBuf,
    pub conversation_reset_intent: PathBuf,
    pub archive: PathBuf,
    pub outbox: PathBuf,
    pub observations: PathBuf,
    pub transcripts: PathBuf,
    pub frame_buffer: PathBuf,
    pub conversation: PathBuf,
    pub attachments: PathBuf,
    pub memory: PathBuf,
    pub memory_daily: PathBuf,
    pub memory_weekly: PathBuf,
    pub memory_jobs: PathBuf,
    pub memory_failed: PathBuf,
    pub memory_facts: PathBuf,
    pub memory_fact_candidates: PathBuf,
    pub memory_fact_usage: PathBuf,
    pub usage: PathBuf,
    pub companion_usage: PathBuf,
    pub companion_presence: PathBuf,
    pub watch_stagnation: PathBuf,
    pub observation_cursor: PathBuf,
    pub notification_processed: PathBuf,
    pub update_check: PathBuf,
    pub model_catalog: PathBuf,
    pub logs: PathBuf,
    pub log: PathBuf,
    pub debug: PathBuf,
    pub watch_lock: PathBuf,
}

impl ConfigPaths {
    pub fn from_root(root: PathBuf) -> Self {
        Self {
            config: root.join("config.json"),
            personas: root.join("personas"),
            builtin_personas: None,
            builtin_tutorial: None,
            mailbox: root.join("mailbox"),
            state: root.join("state"),
            avatar: root.join("state/avatar.png"),
            onboarding: root.join("state/onboarding.json"),
            conversation_generation: root.join("state/conversation-generation.json"),
            conversation_reset_intent: root.join("state/conversation-reset.json"),
            archive: root.join("state/archive"),
            outbox: root.join("state/outbox"),
            observations: root.join("state/observations"),
            transcripts: root.join("state/transcripts"),
            frame_buffer: root.join("state/frames"),
            conversation: root.join("state/conversation"),
            attachments: root.join("state/attachments"),
            memory: root.join("state/memory"),
            memory_daily: root.join("state/memory/daily"),
            memory_weekly: root.join("state/memory/weekly"),
            memory_jobs: root.join("state/memory/jobs"),
            memory_failed: root.join("state/memory/failed"),
            memory_facts: root.join("state/memory/facts.jsonl"),
            memory_fact_candidates: root.join("state/memory/fact-candidates.json"),
            memory_fact_usage: root.join("state/memory/fact-usage.json"),
            usage: root.join("state/usage.json"),
            companion_usage: root.join("state/companion-usage.json"),
            companion_presence: root.join("state/companion-presence.json"),
            watch_stagnation: root.join("state/watch-stagnation.json"),
            observation_cursor: root.join("state/companion-observation-cursor.json"),
            notification_processed: root.join("state/notification-processed.json"),
            update_check: root.join("state/update-check.json"),
            model_catalog: root.join("state/model-catalog.json"),
            logs: root.join("logs"),
            log: root.join("logs/coosenpai.log"),
            debug: root.join("debug"),
            watch_lock: root.join("state/watch.lock"),
            root,
        }
    }

    pub fn for_home(home: &Path) -> Self {
        Self::from_root(home.join(PRODUCT_DIR))
    }

    pub fn with_builtin_personas(mut self, directory: PathBuf) -> Self {
        self.builtin_personas = Some(directory);
        self
    }

    pub fn with_builtin_tutorial(mut self, path: PathBuf) -> Self {
        self.builtin_tutorial = Some(path);
        self
    }
}
