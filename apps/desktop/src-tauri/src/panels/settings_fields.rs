use super::patch;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Deserialize, Serialize, PartialEq)]
pub(super) struct Field {
    pub value: Value,
    pub patch: Value,
}
pub(super) type Fields = BTreeMap<String, Field>;

pub(super) fn check_shape(fields: &Fields, expected: &Fields) -> Result<(), String> {
    if fields.keys().eq(expected.keys()) {
        Ok(())
    } else {
        Err("フォームの項目構成が一致しません".into())
    }
}

pub(super) fn changed(previous: &Fields, incoming: &Fields) -> Fields {
    incoming
        .iter()
        .filter(|(key, field)| previous.get(*key) != Some(*field))
        .map(|(key, field)| (key.clone(), field.clone()))
        .collect()
}

pub(super) fn candidate(base: &Value, baseline: &Fields, current: &Fields) -> Value {
    changed(baseline, current)
        .values()
        .fold(base.clone(), |config, field| {
            patch::merge(&config, &field.patch)
        })
}
