use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TemporaryAssertivenessSelection {
    pub value: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct TemporaryAssertiveness {
    inner: Arc<RwLock<Option<TemporaryAssertivenessSelection>>>,
}

impl TemporaryAssertiveness {
    pub fn set(&self, value: String, expires_at: DateTime<Utc>) {
        *self
            .inner
            .write()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(TemporaryAssertivenessSelection { value, expires_at });
    }

    pub fn current(&self, now: DateTime<Utc>) -> Option<TemporaryAssertivenessSelection> {
        self.inner
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .filter(|selection| selection.expires_at > now)
            .cloned()
    }

    pub fn effective(&self, persistent: &str, now: DateTime<Utc>) -> String {
        self.current(now)
            .map_or_else(|| persistent.to_owned(), |selection| selection.value)
    }

    pub fn clear_if_expires_at(&self, expected: DateTime<Utc>) -> bool {
        let mut current = self
            .inner
            .write()
            .unwrap_or_else(|error| error.into_inner());
        if current
            .as_ref()
            .is_some_and(|selection| selection.expires_at == expected)
        {
            *current = None;
            return true;
        }
        false
    }
}

