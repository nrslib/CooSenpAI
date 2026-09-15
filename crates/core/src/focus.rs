use crate::ports::{FocusElement, FocusElementPort, FOCUS_ELEMENT_TIMEOUT};
use tokio_util::sync::CancellationToken;

pub async fn read_focused_element(
    port: &dyn FocusElementPort,
    cancellation: &CancellationToken,
    target_bundle_id: Option<&str>,
) -> Option<FocusElement> {
    if cancellation.is_cancelled() {
        return None;
    }
    let read = tokio::time::timeout(
        FOCUS_ELEMENT_TIMEOUT,
        port.read_focused_element(target_bundle_id),
    );
    tokio::pin!(read);
    tokio::select! {
        _ = cancellation.cancelled() => None,
        result = &mut read => result.ok().and_then(Result::ok).flatten().map(FocusElement::bounded),
    }
}

