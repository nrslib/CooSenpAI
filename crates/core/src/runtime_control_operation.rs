use super::operation_state::StartResult;
use super::*;

impl RuntimeActor {
    pub(super) fn start_control_operation(
        &mut self,
        command: ControlCommand,
        snapshot_tx: &watch::Sender<RuntimeSnapshot>,
        config_tx: &watch::Sender<Config>,
    ) -> StartResult {
        match command {
            ControlCommand::CompanionObservations { response, .. }
            | ControlCommand::ProcessCompanionMailbox { response, .. }
                if self.observation_delivery == ObservationDelivery::CallerOnly =>
            {
                let _ = response.send(Ok(crate::companion::silent_response()));
                StartResult::Completed
            }
            ControlCommand::Observe {
                frames,
                audio,
                cancellation,
                response,
            } => self.start_observe(frames, audio, cancellation, response, snapshot_tx),
            ControlCommand::Heartbeat {
                stagnation,
                cancellation,
                response,
            } => {
                self.process_heartbeat(stagnation, cancellation, response, snapshot_tx);
                StartResult::Completed
            }
            ControlCommand::CompanionObservations {
                observations,
                context_notice,
                cancellation,
                response,
            } => self.start_companion_observations(
                observations,
                context_notice,
                cancellation,
                response,
                snapshot_tx,
            ),
            ControlCommand::ProcessCompanionMailbox {
                cancellation,
                response,
            } => self.start_companion_mailbox(cancellation, response, snapshot_tx),
            ControlCommand::ReplaceCompanion {
                companion,
                config,
                response,
            } => {
                let result = self.replace_companion_config(*companion, config.map(|value| *value));
                if result.is_ok() {
                    self.operation_cancellation
                        .cancel_current_for_config_update();
                    self.operation_cancellation.renew();
                    let _ = config_tx.send(self.config.clone());
                }
                let _ = response.send(result);
                self.publish(snapshot_tx);
                StartResult::Completed
            }
            ControlCommand::ReplaceConfigWhenIdle {
                config,
                agents,
                response,
            } => {
                let result = self.replace_config(*config, *agents);
                if result.is_ok() {
                    let _ = config_tx.send(self.config.clone());
                }
                let _ = response.send(result);
                self.publish(snapshot_tx);
                StartResult::Completed
            }
            ControlCommand::ConsolidateMemory { period, response } => {
                self.start_consolidate(period, response)
            }
        }
    }
}
