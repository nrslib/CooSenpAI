use coosenpai_core::attachments::{bound_text_attachment, BoundedTextAttachment};
use coosenpai_core::ports::{ClipboardReader, PortError, RuntimeLogger, SelectedTextCopyPort};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CLIPBOARD_POLL_TIMEOUT: Duration = Duration::from_millis(1000);

pub(super) struct PreparedSelectedTextAttachment {
    pub attachment: Option<BoundedTextAttachment>,
    pub accessibility_permission_required: bool,
}

enum ClipboardPollResult {
    Changed(Option<String>),
    Unchanged,
    Cancelled,
}

pub(super) async fn prepare_selected_text_attachment(
    copier: &dyn SelectedTextCopyPort,
    reader: &dyn ClipboardReader,
    logger: &dyn RuntimeLogger,
    cancellation: &CancellationToken,
) -> Result<Option<PreparedSelectedTextAttachment>, PortError> {
    if cancellation.is_cancelled() {
        return Ok(None);
    }
    let previous = reader.read_text()?;
    if cancellation.is_cancelled() {
        return Ok(None);
    }
    let copy_result = copier.synthesize_copy().await;
    if cancellation.is_cancelled() {
        let _ = wait_for_clipboard_change(reader, previous.as_deref(), cancellation).await?;
        return Ok(None);
    }
    let copy_allowed = copy_result?;
    if !copy_allowed {
        let _ = logger.write(
            "INFO",
            "アクセシビリティ未許可のため既存クリップボードを使用しました",
        );
        return Ok(Some(prepared_text_attachment(previous.as_deref(), true)));
    }

    match wait_for_clipboard_change(reader, previous.as_deref(), cancellation).await? {
        ClipboardPollResult::Changed(text) => {
            if cancellation.is_cancelled() {
                return Ok(None);
            }
            let _ = logger.write("INFO", "合成コピーで選択中の文章を取得しました");
            Ok(Some(prepared_text_attachment(text.as_deref(), false)))
        }
        ClipboardPollResult::Unchanged => {
            if cancellation.is_cancelled() {
                return Ok(None);
            }
            let _ = logger.write(
                "INFO",
                "クリップボード変化なしのため既存クリップボードを使用しました",
            );
            Ok(Some(prepared_text_attachment(previous.as_deref(), false)))
        }
        ClipboardPollResult::Cancelled => Ok(None),
    }
}

async fn wait_for_clipboard_change(
    reader: &dyn ClipboardReader,
    previous: Option<&str>,
    cancellation: &CancellationToken,
) -> Result<ClipboardPollResult, PortError> {
    let result = tokio::time::timeout(CLIPBOARD_POLL_TIMEOUT, async {
        let mut cancelled = cancellation.is_cancelled();
        loop {
            let current = match reader.read_text() {
                Ok(current) => current,
                Err(_error) if cancelled || cancellation.is_cancelled() => {
                    cancelled = true;
                    tokio::time::sleep(CLIPBOARD_POLL_INTERVAL).await;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if !cancelled && !cancellation.is_cancelled() && current.as_deref() != previous {
                return Ok(ClipboardPollResult::Changed(current));
            }
            cancelled |= cancellation.is_cancelled();
            if cancelled {
                // キャンセル後も Cmd+C の遅延反映を排出するため、監視窓の終端まで待つ。
                tokio::time::sleep(CLIPBOARD_POLL_INTERVAL).await;
            } else {
                tokio::select! {
                    () = cancellation.cancelled() => cancelled = true,
                    () = tokio::time::sleep(CLIPBOARD_POLL_INTERVAL) => {}
                }
            }
        }
    })
    .await;

    match result {
        Ok(result) => result,
        Err(_) if cancellation.is_cancelled() => Ok(ClipboardPollResult::Cancelled),
        Err(_) => Ok(ClipboardPollResult::Unchanged),
    }
}

fn prepared_text_attachment(
    text: Option<&str>,
    accessibility_permission_required: bool,
) -> PreparedSelectedTextAttachment {
    PreparedSelectedTextAttachment {
        attachment: prepare_text_attachment(text),
        accessibility_permission_required,
    }
}

fn prepare_text_attachment(text: Option<&str>) -> Option<BoundedTextAttachment> {
    text.and_then(bound_text_attachment)
}

#[cfg(test)]
pub(super) fn prepare_clipboard_attachment(
    reader: &dyn ClipboardReader,
) -> Result<Option<BoundedTextAttachment>, PortError> {
    let text = reader.read_text()?;
    Ok(prepare_text_attachment(text.as_deref()))
}
