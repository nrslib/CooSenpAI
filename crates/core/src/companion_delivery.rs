use super::{CompanionAgent, CompanionError};
use crate::companion_storage::PendingDelivery;
use crate::config::PENDING_DELIVERY_ITEM_MAX_BYTES;
use std::mem;

#[derive(Debug, Clone)]
pub(super) struct PendingRemark {
    pub(super) delivery: PendingDelivery,
}

impl CompanionAgent {
    pub fn pending_delivery_status(&self) -> (usize, bool) {
        (
            self.pending_remarks.len(),
            self.delivery_backpressure_active(),
        )
    }

    pub(super) fn pending_delivery_payload_bytes(&self) -> usize {
        self.pending_remarks
            .iter()
            .map(|pending| pending.delivery.payload_size_bytes())
            .fold(0, usize::saturating_add)
    }

    pub(super) fn delivery_capacity_reached(&self) -> bool {
        self.pending_remarks.len() >= self.config.pending_delivery_limit
            || self.pending_delivery_payload_bytes()
                > self
                    .config
                    .pending_delivery_max_bytes
                    .saturating_sub(PENDING_DELIVERY_ITEM_MAX_BYTES)
    }

    pub(super) fn delivery_backpressure_active(&self) -> bool {
        self.outbox_enqueue_blocked || self.delivery_capacity_reached()
    }

    pub(super) fn publish_remark(
        &mut self,
        delivery: &PendingDelivery,
    ) -> Result<bool, CompanionError> {
        if self.outgoing_mailboxes.is_empty() {
            if self.storage.is_some() {
                self.outbox_enqueue_blocked = true;
                self.log_outbox_failure();
                return Ok(false);
            }
            return Ok(true);
        }
        let payload = serde_json::to_value(crate::notification::RemarkEnvelope {
            conversation_generation: self
                .storage
                .as_ref()
                .map(crate::companion_storage::CompanionStorage::conversation_generation)
                .transpose()?
                .unwrap_or(0),
            entry_id: delivery.remark_id.clone(),
            message: delivery.message.clone(),
            message_kind: delivery.message_kind.clone(),
            notification_priority: delivery.notification_priority.clone(),
            caused_by: delivery.observation_ids.last().cloned(),
            caused_by_ids: delivery.observation_ids.clone(),
        })?;
        let Some(storage) = self.storage.clone() else {
            for index in 0..self.outgoing_mailboxes.len() {
                let failed = self.outgoing_mailboxes[index]
                    .publish_with_identity(
                        "remark".to_owned(),
                        delivery.remark_id.clone(),
                        delivery.created_at.clone(),
                        payload.clone(),
                    )
                    .is_err();
                if failed {
                    self.log_outbox_failure();
                    return Ok(false);
                }
            }
            return Ok(true);
        };
        let recipients = self
            .outgoing_mailboxes
            .iter()
            .map(|mailbox| mailbox.recipient_name().to_owned())
            .fold(Vec::new(), |mut recipients, recipient| {
                if !recipients.iter().any(|value| value == &recipient) {
                    recipients.push(recipient);
                }
                recipients
            });
        let outbox = storage.outbox();
        if outbox
            .enqueue(
                &delivery.remark_id,
                &delivery.created_at,
                "remark",
                payload,
                &recipients,
            )
            .is_err()
        {
            self.outbox_enqueue_blocked = true;
            self.log_outbox_failure();
            return Ok(false);
        }
        if outbox.deliver_pending(&storage.mailbox_directory).is_err() {
            self.log_outbox_failure();
        }
        Ok(true)
    }

    pub(super) fn retry_pending_remarks(&mut self) -> Result<(), CompanionError> {
        self.outbox_enqueue_blocked = false;
        let pending = mem::take(&mut self.pending_remarks);
        for (index, item) in pending.iter().enumerate() {
            let mut current = item.clone();
            if !self.ensure_pending_remark_counted(&current.delivery)? {
                self.complete_pending_remark(&current.delivery)?;
                continue;
            }
            if let Err(error) = self.append_pending_remark_conversation(&current.delivery) {
                self.restore_pending_remarks(current, &pending[index + 1..]);
                return Err(error);
            }
            if !current.delivery.enqueued {
                match self.publish_remark(&current.delivery) {
                    Ok(true) => {
                        if let Err(error) =
                            self.mark_pending_remark_enqueued(&current.delivery.remark_id)
                        {
                            self.restore_pending_remarks(current, &pending[index + 1..]);
                            return Err(error);
                        }
                        current.delivery.enqueued = true;
                    }
                    Ok(false) => {
                        self.outbox_enqueue_blocked = true;
                        self.queue_pending_remark(current.delivery);
                        for item in &pending[index + 1..] {
                            self.queue_pending_remark(item.delivery.clone());
                        }
                        break;
                    }
                    Err(error) => {
                        self.restore_pending_remarks(current, &pending[index + 1..]);
                        return Err(error);
                    }
                }
            }
            if let Err(error) = self.complete_pending_remark(&current.delivery) {
                self.restore_pending_remarks(current, &pending[index + 1..]);
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn queue_pending_remark(&mut self, delivery: PendingDelivery) {
        if let Some(pending) = self
            .pending_remarks
            .iter_mut()
            .find(|pending| pending.delivery.remark_id == delivery.remark_id)
        {
            pending.delivery.enqueued |= delivery.enqueued;
            return;
        }
        self.pending_remarks.push(PendingRemark { delivery });
    }

    fn restore_pending_remarks(&mut self, current: PendingRemark, remaining: &[PendingRemark]) {
        self.queue_pending_remark(current.delivery);
        for item in remaining {
            self.queue_pending_remark(item.delivery.clone());
        }
    }

    pub(super) fn deliver_outbox(&self) -> Result<(), CompanionError> {
        let Some(storage) = &self.storage else {
            return Ok(());
        };
        if storage
            .outbox()
            .deliver_pending(&storage.mailbox_directory)
            .is_err()
        {
            self.log_outbox_failure();
        }
        Ok(())
    }

    pub(super) fn log_outbox_failure(&self) {
        if let Some(logger) = &self.logger {
            let _ = logger.write(
                "WARN",
                "companion mailbox配信を保留しました: error-type=mailbox",
            );
        }
    }
}
