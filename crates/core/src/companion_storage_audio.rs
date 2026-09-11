use serde_json::Value;

/// 旧 untagged 読込で本文と入力源が脱落した音声を、journal の ID 参照へ移す。
pub(super) fn migrate_stripped_audio_context(context: &mut Value) -> bool {
    let Some(object) = context.as_object_mut() else {
        return false;
    };
    // 不正な既存 ID リストを補完して正常なデータに見せない。
    if object
        .get("pendingAudioIds")
        .is_some_and(|ids| !ids.is_array())
    {
        return false;
    }
    let Some(observations) = object.get_mut("observations").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut migrated = Vec::new();
    observations.retain(|observation| {
        if let Some(id) = stripped_audio_id(observation) {
            migrated.push(Value::String(id.to_owned()));
            false
        } else {
            true
        }
    });
    if migrated.is_empty() {
        return false;
    }
    let ids = object
        .entry("pendingAudioIds")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("validated audio IDs");
    for id in migrated {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    true
}

fn stripped_audio_id(value: &Value) -> Option<&str> {
    let object = value.as_object()?;
    let keys = [
        "kind",
        "schemaVersion",
        "id",
        "createdAt",
        "windowStart",
        "windowEnd",
    ];
    if object.len() != keys.len()
        || !keys.iter().all(|key| object.contains_key(*key))
        || object.get("kind")?.as_str()? != "audio"
        || object.get("schemaVersion")?.as_u64()? != 1
        || ["createdAt", "windowStart", "windowEnd"].iter().any(|key| {
            object
                .get(*key)
                .and_then(Value::as_str)
                .is_none_or(|time| chrono::DateTime::parse_from_rfc3339(time).is_err())
        })
    {
        return None;
    }
    object.get("id")?.as_str().filter(|id| !id.is_empty())
}
