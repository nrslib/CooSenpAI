#[derive(Default)]
pub(crate) struct SpeechTranscript {
    generation: Option<u64>,
    last_non_empty_partial: Option<String>,
}

impl SpeechTranscript {
    pub(crate) fn begin(&mut self, generation: u64) {
        self.generation = Some(generation);
        self.last_non_empty_partial = None;
    }

    pub(crate) fn remember_partial(&mut self, generation: u64, text: &str) {
        let text = text.trim();
        if self.generation == Some(generation) && !text.is_empty() {
            self.last_non_empty_partial = Some(text.to_owned());
        }
    }

    pub(crate) fn resolve_final(&self, generation: u64, final_text: &str) -> Option<String> {
        if self.generation != Some(generation) {
            return None;
        }
        let final_text = final_text.trim();
        if final_text.is_empty() {
            self.last_non_empty_partial.clone()
        } else {
            Some(final_text.to_owned())
        }
    }

    pub(crate) fn resolve_closed(&self, generation: u64) -> Option<String> {
        self.resolve_final(generation, "")
    }
}

