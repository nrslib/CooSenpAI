use super::{PendingDelivery, OVERSIZED_DELIVERY_REASON};
use crate::persistence::PersistenceError;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) fn delivery_quarantine_record(
    delivery: &PendingDelivery,
) -> Result<(String, Value), PersistenceError> {
    let payload_digest = Sha256::digest(serde_json::to_vec(delivery)?);
    let mut quarantine_digest = Sha256::new();
    quarantine_digest.update(OVERSIZED_DELIVERY_REASON.as_bytes());
    quarantine_digest.update([0]);
    quarantine_digest.update(delivery.remark_id.as_bytes());
    quarantine_digest.update([0]);
    quarantine_digest.update(payload_digest);
    let quarantine_id = format!("{:x}", quarantine_digest.finalize());
    let record = serde_json::json!({
        "schemaVersion": 1,
        "quarantineId": quarantine_id,
        "quarantinedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "reason": OVERSIZED_DELIVERY_REASON,
        "record": delivery,
    });
    Ok((quarantine_id, record))
}
