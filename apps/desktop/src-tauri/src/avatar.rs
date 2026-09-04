use coosenpai_core::config::{is_valid_avatar_path, ConfigPaths};
use coosenpai_core::persistence::atomic_write_bytes;
use image::{ImageFormat, ImageReader};
use std::fs::{self, File};
use std::io::{self, Cursor};
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

pub(crate) const CONFIG_PATH: &str = "state/avatar.png";

const MAX_DIMENSION: u32 = 128;
const MAX_ENCODED_BYTES: usize = 20 * 1024 * 1024;
const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SOURCE_DIMENSION: u32 = 16_384;
const BACKUP_SUFFIX: &str = ".backup";

#[derive(Debug, Error)]
pub(crate) enum AvatarError {
    #[error("アバター画像が空です")]
    Empty,
    #[error("アバター画像が大きすぎます")]
    TooLarge,
    #[error("PNG または JPEG のアバター画像だけを選べます")]
    UnsupportedFormat,
    #[error("アバター画像を読み込めません: {0}")]
    Decode(#[from] image::ImageError),
    #[error("アバター画像を PNG に変換できません: {0}")]
    Encode(image::ImageError),
    #[error("アバター画像の保存に失敗しました: {0}")]
    Io(#[from] io::Error),
}

pub(crate) fn normalize_image(bytes: &[u8]) -> Result<Vec<u8>, AvatarError> {
    if bytes.is_empty() {
        return Err(AvatarError::Empty);
    }
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(AvatarError::TooLarge);
    }

    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Png) | Some(ImageFormat::Jpeg)
    ) {
        return Err(AvatarError::UnsupportedFormat);
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    reader.limits(limits);
    let image = reader.decode()?.thumbnail(MAX_DIMENSION, MAX_DIMENSION);
    let mut output = Cursor::new(Vec::new());
    image
        .write_to(&mut output, ImageFormat::Png)
        .map_err(AvatarError::Encode)?;
    Ok(output.into_inner())
}

pub(crate) struct AvatarLoadResult {
    pub image_png: Option<Vec<u8>>,
    pub failed: bool,
}

pub(crate) struct StagedAvatar {
    final_path: PathBuf,
    staged_path: PathBuf,
    backup_path: Option<PathBuf>,
    installed: bool,
    finalized: bool,
}

pub(crate) fn stage_normalized(
    paths: &ConfigPaths,
    bytes: &[u8],
) -> Result<StagedAvatar, AvatarError> {
    if bytes.is_empty() {
        return Err(AvatarError::Empty);
    }
    let staged_path = paths.avatar.with_file_name(format!(
        ".{}.{}.tmp",
        paths
            .avatar
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("avatar.png"),
        Uuid::new_v4()
    ));
    atomic_write_bytes(&staged_path, bytes)?;
    Ok(StagedAvatar {
        final_path: paths.avatar.clone(),
        staged_path,
        backup_path: None,
        installed: false,
        finalized: false,
    })
}

impl StagedAvatar {
    pub(crate) fn install(&mut self) -> io::Result<()> {
        if self.installed {
            return Ok(());
        }
        if self.final_path.exists() {
            let backup_path = self.final_path.with_file_name(format!(
                ".{}.{}{}",
                self.final_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("avatar.png"),
                Uuid::new_v4(),
                BACKUP_SUFFIX,
            ));
            fs::rename(&self.final_path, &backup_path)?;
            self.backup_path = Some(backup_path);
        }
        if let Err(error) = fs::rename(&self.staged_path, &self.final_path) {
            self.rollback();
            return Err(error);
        }
        self.installed = true;
        if let Err(error) = sync_parent(&self.final_path) {
            self.rollback();
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn finalize(&mut self) -> io::Result<()> {
        self.finalized = true;
        if let Some(backup_path) = self.backup_path.as_ref() {
            fs::remove_file(backup_path)?;
            self.backup_path = None;
        }
        Ok(())
    }

    fn rollback(&mut self) {
        if self.installed {
            let _ = fs::remove_file(&self.final_path);
            self.installed = false;
        }
        if let Some(backup_path) = self.backup_path.take() {
            let _ = fs::rename(backup_path, &self.final_path);
        }
        let _ = fs::remove_file(&self.staged_path);
    }
}

impl Drop for StagedAvatar {
    fn drop(&mut self) {
        if !self.finalized {
            self.rollback();
        }
    }
}

fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "親ディレクトリがありません"))?;
    File::open(parent)?.sync_all()
}

pub(crate) fn load(paths: &ConfigPaths, configured_path: Option<&str>) -> Option<Vec<u8>> {
    load_with_status(paths, configured_path).image_png
}

pub(crate) fn load_with_status(
    paths: &ConfigPaths,
    configured_path: Option<&str>,
) -> AvatarLoadResult {
    let path = configured_path
        .filter(|value| is_valid_avatar_path(value))
        .map(|value| paths.root.join(value));
    let Some(path) = path else {
        return AvatarLoadResult {
            image_png: None,
            failed: configured_path.is_some(),
        };
    };
    let Ok(bytes) = std::fs::read(path) else {
        return AvatarLoadResult {
            image_png: None,
            failed: true,
        };
    };
    match normalize_image(&bytes) {
        Ok(image_png) => AvatarLoadResult {
            image_png: Some(image_png),
            failed: false,
        },
        Err(_) => AvatarLoadResult {
            image_png: None,
            failed: true,
        },
    }
}

pub(crate) fn cleanup_stale_backups(paths: &ConfigPaths) -> io::Result<()> {
    let parent = paths
        .avatar
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "親ディレクトリがありません"))?;
    if !parent.exists() {
        return Ok(());
    }
    let avatar_name = paths
        .avatar
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("avatar.png");
    let prefix = format!(".{avatar_name}.");
    let mut first_error = None;
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if name.starts_with(&prefix) && name.ends_with(BACKUP_SUFFIX) {
            if let Err(error) = fs::remove_file(path) {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

