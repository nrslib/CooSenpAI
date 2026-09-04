use super::{AttachmentOcrFailureKind, CompanionAgent, CompanionError};
use crate::state::ObservationRecord;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub(super) const MAX_OBSERVATION_IMAGE_ATTACHMENTS: usize = 3;
pub(super) const MAX_PROVIDER_IMAGE_ATTACHMENTS: usize = 8;

impl CompanionAgent {
    pub(super) fn observation_image_paths(
        &self,
        observations: &[ObservationRecord],
        observation_frame_paths: &HashMap<String, Vec<PathBuf>>,
    ) -> Vec<PathBuf> {
        let mut selected = Vec::new();
        for path in
            super::helpers::ordered_observation_image_paths(observations, observation_frame_paths)
        {
            if selected.len() >= MAX_OBSERVATION_IMAGE_ATTACHMENTS {
                break;
            }
            match readable_image_file(&path) {
                Ok(()) => selected.push(path),
                Err(error) => self.log_observation_image_skip(&path, &error),
            }
        }
        selected
    }

    pub(super) fn bounded_provider_image_paths(
        &self,
        user_image_paths: Vec<PathBuf>,
        observation_image_paths: Vec<PathBuf>,
    ) -> Vec<PathBuf> {
        user_image_paths
            .into_iter()
            .chain(observation_image_paths)
            .take(MAX_PROVIDER_IMAGE_ATTACHMENTS)
            .collect()
    }

    pub(super) async fn prepare_image_attachments(
        &self,
        image_paths: Vec<PathBuf>,
        cancellation: CancellationToken,
    ) -> Result<(Vec<PathBuf>, Option<String>), CompanionError> {
        let image_paths = image_paths
            .into_iter()
            .take(MAX_PROVIDER_IMAGE_ATTACHMENTS)
            .collect::<Vec<_>>();
        if image_paths.is_empty() {
            return Ok((image_paths, None));
        }
        let capabilities = self
            .provider
            .resolve_model_capabilities(
                Some(self.config.model.as_str()),
                cancellation.child_token(),
                Duration::from_millis(self.config.timeout_ms),
            )
            .await
            .map_err(|_| CompanionError::AttachmentOcr(AttachmentOcrFailureKind::Capability))?;
        let supports_images = capabilities
            .ok_or(CompanionError::AttachmentOcr(
                AttachmentOcrFailureKind::Capability,
            ))?
            .image_input;
        if supports_images {
            return Ok((image_paths, None));
        }
        let mut text = Vec::new();
        if let Some(ocr) = &self.attachment_ocr {
            for path in &image_paths {
                let blocks = ocr
                    .recognize(
                        path,
                        "accurate",
                        Duration::from_secs(5),
                        cancellation.child_token(),
                    )
                    .await
                    .map_err(|_| {
                        CompanionError::AttachmentOcr(AttachmentOcrFailureKind::Recognition)
                    })?;
                text.extend(blocks.into_iter().map(|block| block.text));
            }
        } else {
            return Err(CompanionError::AttachmentOcr(
                AttachmentOcrFailureKind::HelperUnavailable,
            ));
        }
        let text = text.join("\n").chars().take(8_192).collect::<String>();
        if text.is_empty() {
            return Err(CompanionError::AttachmentOcr(
                AttachmentOcrFailureKind::NoText,
            ));
        }
        Ok((Vec::new(), Some(text)))
    }
}

fn readable_image_file(path: &Path) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "観察フレーム画像が通常ファイルではありません",
        ));
    }
    let mut file = File::open(path)?;
    let mut first_byte = [0_u8; 1];
    let _ = file.read(&mut first_byte)?;
    Ok(())
}
