use coosenpai_core::attachments::{bound_text_attachment, BoundedTextAttachment};
use coosenpai_core::ports::{
    ClipboardReader, PortError, RuntimeLogger, SelectedTextCopyOutcome, SelectedTextCopyPort,
    SELECTED_TEXT_POLL_INTERVAL, SELECTED_TEXT_POLL_TIMEOUT,
};
use tokio_util::sync::CancellationToken;

pub(super) struct PreparedSelectedTextAttachment {
    pub attachment: Option<BoundedTextAttachment>,
    pub accessibility_permission_required: bool,
}

#[derive(Debug)]
pub(super) enum SelectedTextAttachmentError {
    ReleaseTimeout,
    ClipboardUnchanged,
    CopyEvent(PortError),
    ClipboardRead(PortError),
}

enum ClipboardPollResult {
    Changed,
    Unchanged,
    Cancelled,
}

pub(super) async fn prepare_selected_text_attachment(
    copier: &dyn SelectedTextCopyPort,
    reader: &dyn ClipboardReader,
    logger: &dyn RuntimeLogger,
    cancellation: &CancellationToken,
) -> Result<Option<PreparedSelectedTextAttachment>, SelectedTextAttachmentError> {
    if cancellation.is_cancelled() {
        return Ok(None);
    }
    let previous = reader
        .read_text()
        .map_err(SelectedTextAttachmentError::ClipboardRead)?;
    if cancellation.is_cancelled() {
        return Ok(None);
    }
    let copy_outcome = copier
        .synthesize_copy(cancellation)
        .await
        .map_err(SelectedTextAttachmentError::CopyEvent)?;
    let change_count_before_post = match copy_outcome {
        SelectedTextCopyOutcome::Cancelled => return Ok(None),
        SelectedTextCopyOutcome::ReleaseTimeout => {
            return Err(SelectedTextAttachmentError::ReleaseTimeout)
        }
        SelectedTextCopyOutcome::PermissionDenied => {
            let _ = logger.write(
                "INFO",
                "アクセシビリティ未許可のため既存クリップボードを使用しました",
            );
            return Ok(Some(prepared_text_attachment(previous.as_deref(), true)));
        }
        SelectedTextCopyOutcome::Sent {
            change_count_before_post,
        } => change_count_before_post,
    };
    if cancellation.is_cancelled() {
        let _ = wait_for_clipboard_change(reader, change_count_before_post, cancellation).await?;
        return Ok(None);
    }

    match wait_for_clipboard_change(reader, change_count_before_post, cancellation).await? {
        ClipboardPollResult::Changed => {
            if cancellation.is_cancelled() {
                return Ok(None);
            }
            let text = reader
                .read_text()
                .map_err(SelectedTextAttachmentError::ClipboardRead)?;
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
            Err(SelectedTextAttachmentError::ClipboardUnchanged)
        }
        ClipboardPollResult::Cancelled => Ok(None),
    }
}

async fn wait_for_clipboard_change(
    reader: &dyn ClipboardReader,
    previous_change_count: i64,
    cancellation: &CancellationToken,
) -> Result<ClipboardPollResult, SelectedTextAttachmentError> {
    let result = tokio::time::timeout(SELECTED_TEXT_POLL_TIMEOUT, async {
        let mut cancelled = cancellation.is_cancelled();
        loop {
            let current_change_count = match reader.change_count() {
                Ok(current) => current,
                Err(_error) if cancelled || cancellation.is_cancelled() => {
                    cancelled = true;
                    tokio::time::sleep(SELECTED_TEXT_POLL_INTERVAL).await;
                    continue;
                }
                Err(error) => return Err(SelectedTextAttachmentError::ClipboardRead(error)),
            };
            if !cancelled
                && !cancellation.is_cancelled()
                && current_change_count != previous_change_count
            {
                return Ok(ClipboardPollResult::Changed);
            }
            cancelled |= cancellation.is_cancelled();
            if cancelled {
                // キャンセル後も Cmd+C の遅延反映を排出するため、監視窓の終端まで待つ。
                tokio::time::sleep(SELECTED_TEXT_POLL_INTERVAL).await;
            } else {
                tokio::select! {
                    () = cancellation.cancelled() => cancelled = true,
                    () = tokio::time::sleep(SELECTED_TEXT_POLL_INTERVAL) => {}
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
