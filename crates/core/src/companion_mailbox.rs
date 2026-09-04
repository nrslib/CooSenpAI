use super::*;
use crate::state::{parse_observation, DEFAULT_OBSERVATION_LIMITS};

impl CompanionAgent {
    pub async fn process_incoming_mailbox(
        &mut self,
        cancellation: CancellationToken,
    ) -> Result<CompanionResponse, CompanionError> {
        if self.delivery_ownership == DeliveryOwnership::None {
            self.initialize_storage()?;
            return Ok(silent_response());
        }
        self.initialize_storage()?;
        let delivery_was_blocked = self.delivery_backpressure_active();
        self.deliver_outbox()?;
        self.retry_pending_remarks()?;
        if delivery_was_blocked || self.delivery_backpressure_active() {
            return Ok(silent_response());
        }
        let Some(mailbox) = self.incoming_mailbox.clone() else {
            return self.observations(Vec::new(), cancellation).await;
        };
        let mut response = silent_response();
        while let Some(claimed) = mailbox.claim()? {
            let observation = match parse_observation(
                claimed.envelope.payload.clone(),
                DEFAULT_OBSERVATION_LIMITS,
            ) {
                Ok(observation) => observation,
                Err(_) => {
                    mailbox.fail(claimed)?;
                    continue;
                }
            };
            if !observation.is_companion_signal() {
                mailbox.complete(claimed)?;
                continue;
            }
            match self
                .observations(vec![observation], cancellation.clone())
                .await
            {
                Ok(next) => {
                    response = next;
                    mailbox.complete(claimed)?;
                    if self.delivery_backpressure_active() {
                        break;
                    }
                }
                Err(error) => {
                    mailbox.retry(claimed)?;
                    return Err(error);
                }
            }
        }
        Ok(response)
    }
}
