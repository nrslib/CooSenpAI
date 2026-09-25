use super::ObserverError;
use serde_json::{json, Value};

pub(crate) struct MicrophoneCommandPrompt {
    pub(crate) ids: Vec<String>,
    pub(crate) companion_name: String,
}

impl MicrophoneCommandPrompt {
    pub(crate) fn append(&self, system: &mut String, prompt: &mut String) {
        system.push_str("\n\n");
        system.push_str(include_str!(
            "../../../builtins/prompts/facets/instructions/microphone-commands.md"
        ));
        prompt.push_str("\nマイク指示の判定対象（hostが現在の許可と入力源を確認したID）:\n");
        prompt.push_str(&crate::prompts::ordered_json_string(&json!({
            "companionDisplayName": self.companion_name,
            "inputIds": self.ids,
        })));
    }

    pub(crate) fn schema(&self) -> Value {
        let mut schema = crate::prompts::observer_schema();
        schema["required"]
            .as_array_mut()
            .expect("observer required")
            .push(json!("microphoneCommandIds"));
        schema["properties"]["microphoneCommandIds"] = json!({
            "type": "array", "items": {"type": "string", "enum": self.ids}
        });
        schema
    }

    pub(crate) fn extract(&self, value: &mut Value) -> Result<Vec<String>, ObserverError> {
        let value = value
            .as_object_mut()
            .and_then(|object| object.remove("microphoneCommandIds"))
            .ok_or(ObserverError::Output)?;
        let ids: Vec<String> = serde_json::from_value(value).map_err(|_| ObserverError::Output)?;
        let mut unique = std::collections::HashSet::new();
        if ids
            .iter()
            .any(|id| !self.ids.contains(id) || !unique.insert(id))
        {
            return Err(ObserverError::Output);
        }
        Ok(ids)
    }
}
