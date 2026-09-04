use crate::provider::{ProviderEventSink, ProviderUsage};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub(super) enum ProviderStreamUpdate {
    Delta {
        input_id: String,
        text: String,
    },
    Reset {
        input_id: String,
    },
    Usage {
        input_id: String,
        usage: ProviderUsage,
    },
}

pub(super) struct RuntimeProviderEvents {
    pub(super) input_id: String,
    pub(super) sender: mpsc::UnboundedSender<ProviderStreamUpdate>,
    pub(super) accepted_mid_turn_ids: Arc<Mutex<HashSet<String>>>,
}

impl ProviderEventSink for RuntimeProviderEvents {
    fn delta(&self, text: &str) {
        let _ = self.sender.send(ProviderStreamUpdate::Delta {
            input_id: self.input_id.clone(),
            text: text.to_owned(),
        });
    }

    fn usage(&self, usage: &ProviderUsage) {
        let _ = self.sender.send(ProviderStreamUpdate::Usage {
            input_id: self.input_id.clone(),
            usage: usage.clone(),
        });
    }

    fn reset(&self) {
        let _ = self.sender.send(ProviderStreamUpdate::Reset {
            input_id: self.input_id.clone(),
        });
    }

    fn mid_turn_accepted(&self, source_id: &str) {
        match self.accepted_mid_turn_ids.lock() {
            Ok(mut ids) => {
                ids.insert(source_id.to_owned());
            }
            Err(poisoned) => {
                poisoned.into_inner().insert(source_id.to_owned());
            }
        }
    }
}
