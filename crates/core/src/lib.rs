//! CooSenpAI の platform 非依存なユースケースと契約。

pub mod attachments;
pub mod companion;
pub mod companion_assertiveness;
pub mod companion_cursor;
pub mod companion_storage;
pub mod config;
pub mod conversation_archive;
pub mod debug;
pub mod frame_buffer;
pub mod image_processing;
pub mod interactive_process;
pub mod logging;
pub mod mailbox;
pub mod memory;
pub mod notification;
pub mod observer;
pub mod onboarding;
pub mod onboarding_notice;
pub mod outbox;
pub mod persistence;
pub mod persona;
pub mod persona_store;
pub mod ports;
pub mod presence;
pub mod process;
mod prompt_json;
pub mod prompts;
pub mod provider;
pub mod provider_api_keys;
pub mod provider_storage;
pub mod recent_observations;
pub mod runtime;
pub mod state;
pub mod timing;
pub mod usage;
pub mod watch_coordinator;

pub use config::{Config, ConfigError, ConfigPaths};
pub use interactive_process::{
    InteractiveProcess, InteractiveProcessControl, InteractiveProcessError,
    InteractiveProcessEvent, InteractiveProcessRequest,
};
pub use process::{ProcessError, ProcessOutput, ProcessRequest, ProcessRunner};
