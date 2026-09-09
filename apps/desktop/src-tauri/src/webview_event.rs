use serde::Serialize;
use tauri::{AppHandle, Manager};

// Tauri's event bus evaluates payloads in every WebView, even with emit_to.
// Conversation-bearing events must instead be evaluated only in their recipient.
pub(crate) fn emit_to(
    app: &AppHandle,
    label: &str,
    event: &str,
    payload: &impl Serialize,
) -> tauri::Result<()> {
    let window = app
        .get_webview_window(label)
        .ok_or(tauri::Error::WindowNotFound)?;
    window.eval(script(event, payload)?)
}

fn script(event: &str, payload: &impl Serialize) -> serde_json::Result<String> {
    // Parse JSON rather than interpolating an object literal: this also preserves
    // keys such as __proto__ without giving them JavaScript object-literal meaning.
    let data = serde_json::to_string(&(event, payload))?;
    let literal = serde_json::to_string(&data)?
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    Ok(format!(
        "(()=>{{const [name,detail]=JSON.parse({literal});window.dispatchEvent(new CustomEvent(name,{{detail}}));}})();"
    ))
}

