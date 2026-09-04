use super::super::bridge_validation::{invalid_output, remove_null_fields, session_json};
use super::super::{ProviderCall, ProviderError, ProviderName, SessionRequest};
use serde_json::{json, Value};
use std::path::Path;

const REQUEST_LINE_LIMIT: usize = 2 * 1024 * 1024;
const REQUEST_MESSAGE_LIMIT: usize = 1024 * 1024;

pub(super) fn send_request_value(
    id: &str,
    provider: ProviderName,
    executable: Option<&Path>,
    cwd: &Path,
    input: &ProviderCall,
    session_mode: &str,
    session_id: Option<&str>,
) -> Value {
    let mut request = json!({
        "id": id,
        "op": "send",
        "provider": provider,
        "session": {"mode": session_mode, "id": session_id},
        "model": input.model,
        "effort": input.effort,
        "systemPrompt": input.system_prompt,
        "message": input.prompt,
        "images": input.images.iter().map(|image| &image.path).collect::<Vec<_>>(),
        "schema": input.output_schema,
        "executable": executable,
        "cwd": cwd,
        "toolsDisabled": input.tools_disabled,
        "timeoutMs": u64::try_from(input.timeout.as_millis()).unwrap_or(u64::MAX),
    });
    remove_null_fields(&mut request);
    request
}

pub(super) fn serialize_request_line(request: &Value) -> Result<Vec<u8>, ProviderError> {
    if request
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| message.len() > REQUEST_MESSAGE_LIMIT)
    {
        return Err(invalid_output(
            "provider bridge の message が 1 MiB を超えています。",
        ));
    }
    let mut line = serde_json::to_vec(request)
        .map_err(|_| invalid_output("provider bridge の要求を JSON 化できません。"))?;
    line.push(b'\n');
    if line.len() > REQUEST_LINE_LIMIT {
        return Err(invalid_output(
            "provider bridge の要求行が 2 MiB を超えています。",
        ));
    }
    Ok(line)
}

pub(super) fn send_request_fits(input: &ProviderCall) -> bool {
    let provider = match &input.session {
        SessionRequest::Resume(session) => session.provider,
        SessionRequest::New | SessionRequest::Ephemeral => ProviderName::Opencode,
    };
    let Ok((session_mode, session_id)) =
        session_json(provider, input.model.as_deref(), &input.session)
    else {
        return false;
    };
    let conservative_path = Path::new(
        "/xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    );
    let request = send_request_value(
        "00000000-0000-0000-0000-000000000000",
        provider,
        Some(conservative_path),
        conservative_path,
        input,
        session_mode,
        session_id,
    );
    serialize_request_line(&request).is_ok()
}
