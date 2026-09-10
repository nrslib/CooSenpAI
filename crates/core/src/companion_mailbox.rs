use super::*;
use crate::mailbox::{ClaimedEnvelope, MailboxDisposition};
use crate::state::{parse_observation, DEFAULT_OBSERVATION_LIMITS};

pub(super) struct PendingMailboxAck {
    mailbox: Mailbox,
    claimed: ClaimedEnvelope,
    outcome: MailboxOutcome,
}

enum MailboxOutcome {
    Complete(Option<CompanionResponse>),
    Retry(CompanionError),
    InvalidObservation,
}

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
        // 新しい claim や配達を始める前に、元の所有者が ACK だけを再試行する。
        if self.pending_mailbox_ack.is_some() {
            return self.retry_mailbox_ack();
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
            let outcome = match parse_observation(
                claimed.envelope.payload.clone(),
                DEFAULT_OBSERVATION_LIMITS,
            ) {
                Err(_) => MailboxOutcome::InvalidObservation,
                Ok(observation) if !observation.is_companion_signal() => {
                    MailboxOutcome::Complete(None)
                }
                Ok(observation) => {
                    match self
                        .process_mailbox_observations(vec![observation], cancellation.clone())
                        .await
                    {
                        Ok(next) => MailboxOutcome::Complete(next),
                        Err(error) => MailboxOutcome::Retry(error),
                    }
                }
            };
            self.pending_mailbox_ack = Some(PendingMailboxAck {
                mailbox: mailbox.clone(),
                claimed,
                outcome,
            });
            if let Some(next) = self.retry_mailbox_ack()? {
                response = Some(next);
            }
            if self.delivery_backpressure_active() {
                break;
            }
        }
        Ok(response)
    }

    fn retry_mailbox_ack(&mut self) -> Result<Option<CompanionResponse>, CompanionError> {
        let Some(pending) = &self.pending_mailbox_ack else {
            return Ok(None);
        };
        let disposition = match pending.outcome {
            MailboxOutcome::Complete(_) => MailboxDisposition::Complete,
            MailboxOutcome::Retry(_) => MailboxDisposition::Retry,
            MailboxOutcome::InvalidObservation => MailboxDisposition::Fail,
        };
        pending.mailbox.acknowledge(&pending.claimed, disposition)?;
        match self
            .pending_mailbox_ack
            .take()
            .expect("acknowledged claim")
            .outcome
        {
            MailboxOutcome::Complete(response) => Ok(response),
            MailboxOutcome::Retry(error) => Err(error),
            MailboxOutcome::InvalidObservation => Err(MailboxError::InvalidEnvelope.into()),
        }
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
