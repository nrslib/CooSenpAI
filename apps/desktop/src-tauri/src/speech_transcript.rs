#[derive(Default)]
pub(crate) struct SpeechTranscript {
    generation: Option<u64>,
}

impl SpeechTranscript {
    pub(crate) fn begin(&mut self, generation: u64) {
        self.generation = Some(generation);
    }

    pub(crate) fn resolve_final(&self, generation: u64, final_text: &str) -> Option<String> {
        if self.generation != Some(generation) {
            return None;
        }
        let final_text = final_text.trim();
        if final_text.is_empty() {
            None
        } else {
            Some(final_text.to_owned())
        }
    }
}

