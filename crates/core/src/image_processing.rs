use crate::ports::OwnWindowBounds;
use chrono::{DateTime, Utc};
use image::{DynamicImage, ImageFormat, ImageReader, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use std::io::{self, Cursor};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Semaphore;

const REDUCED_WIDTH: u32 = 128;
const REDUCED_HEIGHT: u32 = 72;
const MAX_ENCODED_BYTES: usize = 32 * 1024 * 1024;
const MAX_DIMENSION: u32 = 16_384;
const MAX_DECODED_PIXELS: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ExcludedBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub fn own_window_exclusions(
    own_windows: &OwnWindowBounds,
    now: DateTime<Utc>,
) -> Option<Vec<ExcludedBounds>> {
    if !own_windows.is_fresh_at(now) {
        return None;
    }
    let mut exclusions = Vec::with_capacity(own_windows.bounds.len());
    for bounds in own_windows.bounds.iter().copied() {
        if !bounds.x.is_finite()
            || !bounds.y.is_finite()
            || !bounds.width.is_finite()
            || !bounds.height.is_finite()
            || bounds.width <= 0.0
            || bounds.height <= 0.0
        {
            return None;
        }
        exclusions.push(ExcludedBounds {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        });
    }
    Some(exclusions)
}

#[derive(Debug, Clone, Copy)]
pub struct ImageLimits {
    pub max_width: u32,
    pub max_height: u32,
    pub max_decoded_bytes: u64,
}

impl Default for ImageLimits {
    fn default() -> Self {
        Self {
            max_width: MAX_DIMENSION,
            max_height: MAX_DIMENSION,
            max_decoded_bytes: MAX_DECODED_PIXELS * 4,
        }
    }
}

#[derive(Debug, Error)]
pub enum ImageError {
    #[error("画像が大きすぎます")]
    TooLarge,
    #[error("PNG を読み込めません: {0}")]
    Decode(#[from] image::ImageError),
    #[error("PNG を書き出せません: {0}")]
    Encode(#[source] image::ImageError),
    #[error("PNG 入力が空です")]
    Empty,
    #[error("画像処理の I/O に失敗しました: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone)]
pub struct ProcessedImage {
    pub width: u32,
    pub height: u32,
    pub provider_png: Vec<u8>,
    pub masked_png: Vec<u8>,
    pub comparison_pixels: Vec<f64>,
    pub comparison_hash: String,
}

pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

pub async fn process_png(
    bytes: Vec<u8>,
    target_width: u32,
    ignored_top_pixels: u32,
    excluded_bounds: Vec<ExcludedBounds>,
    semaphore: Arc<Semaphore>,
) -> Result<ProcessedImage, ImageError> {
    let _permit = semaphore
        .acquire_owned()
        .await
        .map_err(|_| ImageError::TooLarge)?;
    tokio::task::spawn_blocking(move || {
        process_png_sync(
            &bytes,
            target_width,
            ignored_top_pixels,
            &excluded_bounds,
            ImageLimits::default(),
        )
    })
    .await
    .map_err(|_| ImageError::TooLarge)?
}

pub fn process_png_sync(
    bytes: &[u8],
    target_width: u32,
    ignored_top_pixels: u32,
    excluded_bounds: &[ExcludedBounds],
    limits: ImageLimits,
) -> Result<ProcessedImage, ImageError> {
    if bytes.is_empty() || bytes.len() > MAX_ENCODED_BYTES {
        return Err(if bytes.is_empty() {
            ImageError::Empty
        } else {
            ImageError::TooLarge
        });
    }
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut decode_limits = image::Limits::default();
    decode_limits.max_image_width = Some(limits.max_width);
    decode_limits.max_image_height = Some(limits.max_height);
    decode_limits.max_alloc = Some(limits.max_decoded_bytes);
    reader.limits(decode_limits);
    let image = reader.decode()?.to_rgba8();
    let width = image.width();
    let height = image.height();
    let decoded_bytes = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(ImageError::TooLarge)?;
    if width == 0
        || height == 0
        || width > limits.max_width
        || height > limits.max_height
        || decoded_bytes > limits.max_decoded_bytes
    {
        return Err(ImageError::TooLarge);
    }

    let mask = make_mask(width, height, excluded_bounds);
    let comparison_pixels = reduce_pixels(&image, &mask, ignored_top_pixels);
    let comparison_hash = hash_reduced_pixels(&comparison_pixels);
    let masked = apply_mask(&image, &mask);
    let provider = nearest_resize(&masked, target_width);
    let provider_png = encode_png(&provider)?;
    let masked_png = encode_png(&masked)?;
    Ok(ProcessedImage {
        width,
        height,
        provider_png,
        masked_png,
        comparison_pixels,
        comparison_hash,
    })
}

fn make_mask(width: u32, height: u32, bounds: &[ExcludedBounds]) -> Vec<bool> {
    let mut mask = vec![false; (width as usize) * (height as usize)];
    for bounds in bounds {
        let left = bounds.x.floor().max(0.0) as u32;
        let top = bounds.y.floor().max(0.0) as u32;
        let right = (bounds.x + bounds.width)
            .ceil()
            .max(0.0)
            .min(f64::from(width)) as u32;
        let bottom = (bounds.y + bounds.height)
            .ceil()
            .max(0.0)
            .min(f64::from(height)) as u32;
        for y in top.min(height)..bottom.min(height) {
            for x in left.min(width)..right.min(width) {
                mask[(y * width + x) as usize] = true;
            }
        }
    }
    mask
}

fn reduce_pixels(image: &RgbaImage, mask: &[bool], ignored_top_pixels: u32) -> Vec<f64> {
    let width = image.width();
    let height = image.height();
    let mut pixels = Vec::with_capacity((REDUCED_WIDTH * REDUCED_HEIGHT) as usize);
    for index in 0..(REDUCED_WIDTH * REDUCED_HEIGHT) {
        let target_y = index / REDUCED_WIDTH;
        let target_x = index % REDUCED_WIDTH;
        let source_top = target_y * height / REDUCED_HEIGHT;
        let source_bottom = ((target_y + 1) * height / REDUCED_HEIGHT).max(source_top + 1);
        let source_left = target_x * width / REDUCED_WIDTH;
        let source_right = ((target_x + 1) * width / REDUCED_WIDTH).max(source_left + 1);
        let mut total = 0.0;
        let mut count = 0u64;
        for y in source_top..source_bottom.min(height) {
            for x in source_left..source_right.min(width) {
                if y < ignored_top_pixels || mask[(y * width + x) as usize] {
                    continue;
                }
                let pixel = image.get_pixel(x, y).0;
                total += (f64::from(pixel[0]) * 0.299
                    + f64::from(pixel[1]) * 0.587
                    + f64::from(pixel[2]) * 0.114)
                    / 255.0;
                count += 1;
            }
        }
        pixels.push(if count == 0 {
            0.0
        } else {
            total / count as f64
        });
    }
    pixels
}

fn apply_mask(image: &RgbaImage, mask: &[bool]) -> RgbaImage {
    let mut output = image.clone();
    for (index, pixel) in output.pixels_mut().enumerate() {
        if mask[index] {
            *pixel = Rgba([0, 0, 0, 255]);
        }
    }
    output
}

fn nearest_resize(image: &RgbaImage, target_width: u32) -> RgbaImage {
    let output_width = image.width().min(target_width.max(1));
    let output_height = ((f64::from(image.height()) * f64::from(output_width)
        / f64::from(image.width()))
    .round() as u32)
        .max(1);
    let mut output = RgbaImage::new(output_width, output_height);
    for y in 0..output_height {
        for x in 0..output_width {
            let source_x = (u64::from(x) * u64::from(image.width()) / u64::from(output_width))
                .min(u64::from(image.width() - 1)) as u32;
            let source_y = (u64::from(y) * u64::from(image.height()) / u64::from(output_height))
                .min(u64::from(image.height() - 1)) as u32;
            output.put_pixel(x, y, *image.get_pixel(source_x, source_y));
        }
    }
    output
}

fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, ImageError> {
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(ImageError::Encode)?;
    Ok(bytes.into_inner())
}

pub fn hash_reduced_pixels(pixels: &[f64]) -> String {
    let mut hash = 2_166_136_261u32;
    for pixel in pixels {
        let value = (pixel * 65_535.0).round() as u32;
        hash ^= value;
        hash = hash.wrapping_mul(16_777_619);
    }
    format!("{REDUCED_WIDTH}x{REDUCED_HEIGHT}:{hash}")
}

