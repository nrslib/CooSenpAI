use super::*;

impl RuntimeActor {
    pub(super) fn accept_companion_thought(&mut self, response: &CompanionResponse) {
        let Some(thought) = response.thought.as_deref() else {
            return;
        };
        if self.latest_companion_thought.as_deref() == Some(thought) {
            return;
        }
        self.latest_companion_thought = Some(thought.to_owned());
        if let Some(logger) = &self.logger {
            let _ = logger.write("INFO", &format!("Coo の思考: {thought}"));
        }
    }
}
