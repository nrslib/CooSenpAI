use super::{CaptureOrigin, ReadyAttachment, ReadyCapture};

pub(super) async fn prepare_image(
    png: Vec<u8>,
    origin: CaptureOrigin,
    conversation: crate::command_guard::GenerationStamp,
) -> Result<ReadyCapture, String> {
    if png.len() as u64 > super::MAX_PREVIEW_BYTES {
        return Err("選択した画像が大きすぎます".to_owned());
    }
    let directory = tempfile::Builder::new()
        .prefix("coosenpai-selection-")
        .tempdir()
        .map_err(|e| format!("範囲選択の一時ファイルを作成できませんでした: {e}"))?;
    let path = directory.path().join("capture.png");
    tokio::fs::write(&path, png)
        .await
        .map_err(|error| format!("選択した画像を保存できません: {error}"))?;
    Ok(ReadyCapture {
        conversation,
        id: uuid::Uuid::new_v4().to_string(),
        attachment: ReadyAttachment::Image {
            path,
            _directory: directory,
        },
        origin,
        accessibility_permission_required: false,
    })
}

#[cfg(test)]
pub(super) async fn guide_permission_error(
    settings: &dyn coosenpai_core::ports::SystemSettingsPort,
    logger: &dyn coosenpai_core::ports::RuntimeLogger,
    cancellation: &CancellationToken,
    error: &str,
) {
    if error != crate::platform::SCREENSHOT_ACCESS_REQUIRED || cancellation.is_cancelled() {
        return;
    }
    let _ = logger.write("DEBUG", "範囲選択: 段階=accessibility-settings-open");
    if let Err(error) = settings
        .open(
            coosenpai_core::ports::SystemSettingsPane::Accessibility,
            cancellation.clone(),
        )
        .await
    {
        let _ = logger.write(
            "WARN",
            &format!("アクセシビリティ設定を開けません: {error}"),
        );
    }
}

