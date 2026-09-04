use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompanionReminder {
    pub id: String,
    pub time: String,
    pub theme: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppConfig {
    #[serde(default)]
    pub launch_at_login: bool,
    #[serde(default = "default_check_for_updates")]
    pub check_for_updates: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            check_for_updates: true,
        }
    }
}

fn default_check_for_updates() -> bool {
    true
}
