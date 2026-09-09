use super::*;

impl RuntimeActor {
    pub(super) fn accept_companion_response(
        &mut self,
        companion: &CompanionAgent,
        response: &CompanionResponse,
        observation_ids: Vec<String>,
    ) {
        self.companion_decision_sequence += 1;
        self.latest_companion_decision = Some(CompanionDecision {
            sequence: self.companion_decision_sequence,
            occurred_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            emit: response.emit,
            message_kind: response.message_kind.clone(),
            message: response.message.clone(),
            thought: response.thought.clone(),
            observation_ids,
        });
        let Some(thought) = response.thought.as_deref() else {
            return;
        };
        if self.latest_companion_thought.as_deref() == Some(thought) {
            return;
        }
        // 世代を特定できない thought は吹き出し表示側の世代照合で必ず落ちる。
        // 旧世代の thought を新世代として表示する race を遮るため、不明なままにはしない。
        let conversation_generation = match companion.conversation_generation() {
            Ok(generation) => Some(generation),
            Err(error) => {
                if let Some(logger) = &self.logger {
                    let _ = logger.write(
                        "WARN",
                        &format!(
                            "Coo の思考の会話世代を特定できませんでした: error-type=thought-generation ({error})"
                        ),
                    );
                }
                None
            }
        };
        self.latest_companion_thought = Some(thought.to_owned());
        self.latest_companion_thought_generation = conversation_generation;
        if let Some(logger) = &self.logger {
            let _ = logger.write("INFO", &format!("Coo の思考: {thought}"));
        }
    }
}
