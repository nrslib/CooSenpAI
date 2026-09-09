use super::*;
use crate::state::{parse_observation, DEFAULT_OBSERVATION_LIMITS};

impl CompanionAgent {
    pub async fn process_incoming_mailbox(
        &mut self,
        cancellation: CancellationToken,
    ) -> Result<CompanionResponse, CompanionError> {
        self.process_incoming_mailbox_decision(cancellation)
            .await
            .map(|decision| decision.unwrap_or_else(silent_response))
    }

    pub(crate) async fn process_incoming_mailbox_decision(
        &mut self,
        cancellation: CancellationToken,
    ) -> Result<Option<CompanionResponse>, CompanionError> {
        if self.delivery_ownership == DeliveryOwnership::None {
            self.initialize_storage()?;
            return Ok(None);
        }
        self.initialize_storage()?;
        let delivery_was_blocked = self.delivery_backpressure_active();
        self.deliver_outbox()?;
        self.retry_pending_remarks()?;
        if delivery_was_blocked || self.delivery_backpressure_active() {
            return Ok(None);
        }
        let Some(mailbox) = self.incoming_mailbox.clone() else {
            return self
                .process_mailbox_observations(Vec::new(), cancellation)
                .await;
        };
        let mut response = None;
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
                .process_mailbox_observations(vec![observation], cancellation.clone())
                .await
            {
                Ok(next) => {
                    if next.is_some() {
                        response = next;
                    }
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

    async fn process_mailbox_observations(
        &mut self,
        observations: Vec<ObservationRecord>,
        cancellation: CancellationToken,
    ) -> Result<Option<CompanionResponse>, CompanionError> {
        let candidate = self
            .process_observations_candidate(observations, None, cancellation)
            .await?;
        let decision_produced = candidate.decision_produced;
        let response = self.commit_proactive_candidate(candidate)?;
        Ok(decision_produced.then_some(response))
    }
}
