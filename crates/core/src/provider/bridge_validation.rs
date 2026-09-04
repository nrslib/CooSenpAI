use super::{
    ProviderCall, ProviderCapabilities, ProviderError, ProviderErrorKind, ProviderName,
    SessionRequest,
};
use serde_json::Value;

pub(super) fn session_json<'a>(
    provider: ProviderName,
    model: Option<&'a str>,
    request: &'a SessionRequest,
) -> Result<(&'static str, Option<&'a str>), ProviderError> {
    match request {
        SessionRequest::New => Ok(("new", None)),
        SessionRequest::Ephemeral => Ok(("ephemeral", None)),
        SessionRequest::Resume(session) => {
            if session.provider != provider {
                return Err(ProviderError {
                    kind: ProviderErrorKind::Unsupported,
                    message: "セッションの provider が現在の provider と一致しません。".to_owned(),
                });
            }
            if session.model.as_deref() != model.filter(|value| *value != "default") {
                return Err(ProviderError {
                    kind: ProviderErrorKind::InvalidModel,
                    message: "セッションのモデルが現在の設定と一致しません。".to_owned(),
                });
            }
            Ok(("resume", Some(session.id.as_str())))
        }
    }
}

pub(super) fn validate_call(
    capabilities: &ProviderCapabilities,
    input: &ProviderCall,
) -> Result<(), ProviderError> {
    if !input.tools_disabled {
        return Err(ProviderError {
            kind: ProviderErrorKind::Unsupported,
            message: "provider bridge はツールを有効にした呼び出しを拒否します。".to_owned(),
        });
    }
    if !input.images.is_empty() && !capabilities.image_input {
        return Err(ProviderError {
            kind: ProviderErrorKind::Unsupported,
            message: "provider は画像入力に対応していません。".to_owned(),
        });
    }
    if input.output_schema.is_some() && !capabilities.effective_structured_output {
        return Err(ProviderError {
            kind: ProviderErrorKind::Unsupported,
            message: "provider は structured output に対応していません。".to_owned(),
        });
    }
    if input
        .effort
        .as_deref()
        .is_some_and(|effort| effort != "default")
        && !capabilities.effort
    {
        return Err(ProviderError {
            kind: ProviderErrorKind::Unsupported,
            message: "provider は effort 指定に対応していません。".to_owned(),
        });
    }
    if matches!(&input.session, SessionRequest::Resume(_)) && !capabilities.session_resume {
        return Err(ProviderError {
            kind: ProviderErrorKind::Unsupported,
            message: "provider は session resume に対応していません。".to_owned(),
        });
    }
    Ok(())
}

pub(super) fn parse_error_kind(kind: Option<&str>) -> ProviderErrorKind {
    match kind {
        Some("auth") => ProviderErrorKind::Auth,
        Some("unsupported") => ProviderErrorKind::Unsupported,
        Some("invalid-model") => ProviderErrorKind::InvalidModel,
        Some("invalid-output") | Some("protocol") => ProviderErrorKind::InvalidOutput,
        _ => ProviderErrorKind::Retryable,
    }
}

pub(super) fn remove_null_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|_, value| !value.is_null());
            for value in object.values_mut() {
                remove_null_fields(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                remove_null_fields(value);
            }
        }
        _ => {}
    }
}

pub(super) fn retryable(message: &str) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::Retryable,
        message: message.to_owned(),
    }
}

pub(super) fn invalid_output(message: &str) -> ProviderError {
    ProviderError {
        kind: ProviderErrorKind::InvalidOutput,
        message: message.to_owned(),
    }
}
