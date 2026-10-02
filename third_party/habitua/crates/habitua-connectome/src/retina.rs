use std::collections::BTreeMap;
use std::fs;
use std::io::Cursor;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::rate::{RateGraph, RateModelError};

/// RGB image used by the fixed retina projection.
#[derive(Clone, Debug, PartialEq)]
pub struct RgbImage {
    /// Image width in pixels.
    pub width: usize,
    /// Image height in pixels.
    pub height: usize,
    /// Row-major linear RGB values in `[0, 1]`.
    pub pixels: Vec<[f64; 3]>,
}

impl RgbImage {
    /// Constructs an image and validates its dimensions and channel range.
    pub fn new(width: usize, height: usize, pixels: Vec<[f64; 3]>) -> Result<Self, RateModelError> {
        if width == 0
            || height == 0
            || pixels.len() != width * height
            || pixels
                .iter()
                .flatten()
                .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
        {
            return Err(RateModelError::Invalid(
                "RGB image dimensions or values are invalid".to_owned(),
            ));
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// Returns one row-major pixel.
    pub fn pixel(&self, x: usize, y: usize) -> [f64; 3] {
        self.pixels[y * self.width + x]
    }

    /// Resamples an image to the fixed retina input dimensions using bilinear
    /// interpolation.
    pub fn resize_bilinear(&self, width: usize, height: usize) -> Result<Self, RateModelError> {
        if width == 0 || height == 0 {
            return Err(RateModelError::Invalid(
                "resized RGB image dimensions must be positive".to_owned(),
            ));
        }
        if self.width == width && self.height == height {
            return Ok(self.clone());
        }
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            let source_y = if height == 1 {
                0.0
            } else {
                y as f64 * (self.height - 1) as f64 / (height - 1) as f64
            };
            let y0 = source_y.floor() as usize;
            let y1 = (y0 + 1).min(self.height - 1);
            let y_weight = source_y - y0 as f64;
            for x in 0..width {
                let source_x = if width == 1 {
                    0.0
                } else {
                    x as f64 * (self.width - 1) as f64 / (width - 1) as f64
                };
                let x0 = source_x.floor() as usize;
                let x1 = (x0 + 1).min(self.width - 1);
                let x_weight = source_x - x0 as f64;
                let top = self.pixel(x0, y0);
                let top_right = self.pixel(x1, y0);
                let bottom = self.pixel(x0, y1);
                let bottom_right = self.pixel(x1, y1);
                pixels.push(std::array::from_fn(|channel| {
                    let top_value = top[channel] * (1.0 - x_weight) + top_right[channel] * x_weight;
                    let bottom_value =
                        bottom[channel] * (1.0 - x_weight) + bottom_right[channel] * x_weight;
                    top_value * (1.0 - y_weight) + bottom_value * y_weight
                }));
            }
        }
        Self::new(width, height, pixels)
    }

    /// Reads an 8-bit ASCII or binary PPM (`P3` or `P6`) image.
    pub fn read_ppm(path: impl AsRef<Path>) -> Result<Self, RateModelError> {
        let bytes = fs::read(path)?;
        let mut offset = 0;
        let magic = ppm_token(&bytes, &mut offset)?;
        let binary = magic == b"P6";
        let ascii = magic == b"P3";
        if !binary && !ascii {
            return Err(RateModelError::Invalid(
                "screen image must be an ASCII or binary PPM (P3 or P6)".to_owned(),
            ));
        }
        let width = parse_ppm_usize(&ppm_token(&bytes, &mut offset)?, "width")?;
        let height = parse_ppm_usize(&ppm_token(&bytes, &mut offset)?, "height")?;
        let maximum = parse_ppm_usize(&ppm_token(&bytes, &mut offset)?, "maximum")?;
        if width == 0 || height == 0 || maximum != 255 {
            return Err(RateModelError::Invalid(
                "screen PPM dimensions or maximum value are invalid".to_owned(),
            ));
        }
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| RateModelError::Invalid("screen PPM dimensions overflow".to_owned()))?;
        if ascii {
            let mut pixels = Vec::with_capacity(pixel_count);
            for _ in 0..pixel_count {
                let mut pixel = [0.0; 3];
                for channel in &mut pixel {
                    let value = parse_ppm_usize(&ppm_token(&bytes, &mut offset)?, "pixel")?;
                    if value > 255 {
                        return Err(RateModelError::Invalid(
                            "screen PPM pixel value is outside 8-bit range".to_owned(),
                        ));
                    }
                    *channel = f64::from(value as u8) / 255.0;
                }
                pixels.push(pixel);
            }
            return Self::new(width, height, pixels);
        }
        if offset >= bytes.len() || !bytes[offset].is_ascii_whitespace() {
            return Err(RateModelError::Invalid(
                "screen PPM header is missing its data separator".to_owned(),
            ));
        }
        offset += 1;
        if bytes.get(offset.wrapping_sub(1)) == Some(&b'\r') && bytes.get(offset) == Some(&b'\n') {
            offset += 1;
        }
        let byte_count = pixel_count
            .checked_mul(3)
            .ok_or_else(|| RateModelError::Invalid("screen PPM byte count overflow".to_owned()))?;
        if bytes.len().saturating_sub(offset) != byte_count {
            return Err(RateModelError::Invalid(
                "screen PPM pixel byte count does not match dimensions".to_owned(),
            ));
        }
        let (pixel_bytes, remainder) = bytes[offset..].as_chunks::<3>();
        debug_assert!(remainder.is_empty());
        let pixels = pixel_bytes
            .iter()
            .map(|pixel| {
                [
                    f64::from(pixel[0]) / 255.0,
                    f64::from(pixel[1]) / 255.0,
                    f64::from(pixel[2]) / 255.0,
                ]
            })
            .collect();
        Self::new(width, height, pixels)
    }

    /// Reads an 8-bit PNG image, preserving RGB channels and ignoring alpha.
    pub fn read_png(path: impl AsRef<Path>) -> Result<Self, RateModelError> {
        let bytes = fs::read(path)?;
        let decoder = png::Decoder::new(Cursor::new(bytes));
        let mut reader = decoder
            .read_info()
            .map_err(|error| RateModelError::Invalid(format!("PNG header is invalid: {error}")))?;
        let output_size = reader.output_buffer_size();
        let mut buffer = vec![0_u8; output_size];
        let info = reader.next_frame(&mut buffer).map_err(|error| {
            RateModelError::Invalid(format!("PNG frame cannot be decoded: {error}"))
        })?;
        if info.bit_depth != png::BitDepth::Eight {
            return Err(RateModelError::Invalid(
                "screen PNG must use 8-bit channels".to_owned(),
            ));
        }
        let width = info.width as usize;
        let height = info.height as usize;
        let channels = match info.color_type {
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            png::ColorType::Grayscale => 1,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Indexed => {
                return Err(RateModelError::Invalid(
                    "indexed screen PNG is not supported".to_owned(),
                ));
            }
        };
        let expected = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(channels))
            .ok_or_else(|| RateModelError::Invalid("screen PNG dimensions overflow".to_owned()))?;
        if info.buffer_size() != expected {
            return Err(RateModelError::Invalid(
                "screen PNG pixel byte count does not match dimensions".to_owned(),
            ));
        }
        let pixels = buffer[..info.buffer_size()]
            .chunks_exact(channels)
            .map(|pixel| match info.color_type {
                png::ColorType::Rgb => [
                    f64::from(pixel[0]) / 255.0,
                    f64::from(pixel[1]) / 255.0,
                    f64::from(pixel[2]) / 255.0,
                ],
                png::ColorType::Rgba => [
                    f64::from(pixel[0]) / 255.0,
                    f64::from(pixel[1]) / 255.0,
                    f64::from(pixel[2]) / 255.0,
                ],
                png::ColorType::Grayscale => {
                    let value = f64::from(pixel[0]) / 255.0;
                    [value, value, value]
                }
                png::ColorType::GrayscaleAlpha => {
                    let value = f64::from(pixel[0]) / 255.0;
                    [value, value, value]
                }
                png::ColorType::Indexed => unreachable!("indexed PNG was rejected above"),
            })
            .collect();
        Self::new(width, height, pixels)
    }
}

fn ppm_token(bytes: &[u8], offset: &mut usize) -> Result<Vec<u8>, RateModelError> {
    while *offset < bytes.len() {
        match bytes[*offset] {
            byte if byte.is_ascii_whitespace() => *offset += 1,
            b'#' => {
                while *offset < bytes.len() && bytes[*offset] != b'\n' {
                    *offset += 1;
                }
            }
            _ => break,
        }
    }
    let start = *offset;
    while *offset < bytes.len() && !bytes[*offset].is_ascii_whitespace() && bytes[*offset] != b'#' {
        *offset += 1;
    }
    if start == *offset {
        return Err(RateModelError::Invalid(
            "screen PPM header is incomplete".to_owned(),
        ));
    }
    Ok(bytes[start..*offset].to_vec())
}

fn parse_ppm_usize(token: &[u8], name: &str) -> Result<usize, RateModelError> {
    std::str::from_utf8(token)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| RateModelError::Invalid(format!("screen PPM {name} is invalid")))
}

/// Fixed choices for the screen-to-retina transform.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetinaMapConfig {
    /// Working image width.
    pub width: usize,
    /// Working image height.
    pub height: usize,
    /// Whether the normalized optic-lobe y coordinate is flipped.
    pub flip_y: bool,
    /// Whether to give R7 cells an explicitly artificial grayscale proxy.
    pub r7_grayscale_proxy: bool,
    /// External input multiplier from the plan.
    pub input_multiplier: f64,
    /// Receptor types admitted to the external input.
    pub input_types: Vec<String>,
    /// Policy for cells whose soma side is unknown.
    pub unknown_side_policy: String,
    /// Rule used when a receptor has no assigned optic-lobe coordinate.
    pub coordinate_rule: String,
}

impl Default for RetinaMapConfig {
    fn default() -> Self {
        Self {
            width: 256,
            height: 144,
            flip_y: true,
            r7_grayscale_proxy: false,
            input_multiplier: 5.0,
            input_types: vec!["R1-R6".to_owned(), "R8p".to_owned(), "R8y".to_owned()],
            unknown_side_policy: "retain".to_owned(),
            coordinate_rule: "assigned_ol_hex_then_weighted_outgoing_target".to_owned(),
        }
    }
}

/// Image channel assigned to one receptor type.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RetinaChannel {
    /// Linear luminance for R1-R6.
    Luminance,
    /// Linear blue channel for R8p.
    Blue,
    /// Linear green channel for R8y.
    Green,
}

/// One receptor-to-pixel mapping entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetinaMapEntry {
    /// Graph neuron index.
    pub neuron_index: u32,
    /// Type annotation used to choose the channel.
    pub type_name: String,
    /// Soma side retained from the annotation.
    pub side: Option<String>,
    /// Normalized image x coordinate.
    pub x: f64,
    /// Normalized image y coordinate.
    pub y: f64,
    /// Fixed channel assignment.
    pub channel: RetinaChannel,
    /// `self` or `weighted_outgoing_target`.
    pub coordinate_source: String,
    /// Contact-weighted coordinate spread used when the coordinate was inferred.
    pub inferred_coordinate_spread: Option<f64>,
    /// Contact count used for the inferred coordinate.
    pub inferred_contact_count: Option<u64>,
    /// Target type with the largest contact count for the inferred coordinate.
    pub inferred_majority_target_type: Option<String>,
    /// Fraction of inferred contacts belonging to the majority target type.
    pub inferred_majority_ratio: Option<f64>,
    /// Contact counts by target type used for inference.
    pub inferred_target_type_counts: BTreeMap<String, u64>,
}

/// Audit counts for construction and use of a retina map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetinaAudit {
    /// Receptor types admitted to the external input.
    pub input_types: Vec<String>,
    /// Fixed policy for unknown soma side.
    pub unknown_side_policy: String,
    /// Coordinate rule used for this map.
    pub coordinate_rule: String,
    /// Number of annotated receptor cells considered.
    pub receptor_neuron_count: usize,
    /// Number of mapped receptor cells.
    pub mapped_neuron_count: usize,
    /// Number without own or inferred coordinates.
    pub missing_coordinate_count: usize,
    /// Number with a known type but no supported RGB channel.
    pub unsupported_type_count: usize,
    /// Number with an L-side annotation.
    pub left_count: usize,
    /// Number with an R-side annotation.
    pub right_count: usize,
    /// Number with no recognized side.
    pub unknown_side_count: usize,
    /// Counts by unmapped type.
    pub unsupported_types: BTreeMap<String, usize>,
    /// Number of mapped entries whose coordinate was inferred.
    pub inferred_coordinate_count: usize,
    /// Contact-weighted coordinate spread for each inferred entry.
    pub inferred_coordinate_spread: Vec<f64>,
    /// Total contacts used by coordinate inference.
    pub inferred_contact_count: u64,
    /// Majority target-type ratio for each inferred entry.
    pub inferred_majority_ratio: Vec<f64>,
    /// Target-type contact counts accumulated over all inferences.
    pub inferred_target_type_counts: BTreeMap<String, u64>,
    /// Number of mapped neurons with unknown side.
    pub unknown_side_mapped_count: usize,
}

/// Immutable screen-to-receptor map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetinaMap {
    /// Mapping configuration.
    pub config: RetinaMapConfig,
    /// Deterministic per-neuron entries.
    pub entries: Vec<RetinaMapEntry>,
    /// Construction audit.
    pub audit: RetinaAudit,
}

/// Result of applying one image to a map.
#[derive(Clone, Debug, PartialEq)]
pub struct RetinaInput {
    /// Full graph-sized external input vector.
    pub input: Vec<f64>,
    /// Map audit copied for the evaluation record.
    pub audit: RetinaAudit,
}

impl RetinaMap {
    /// Builds a fixed map, inferring missing receptor coordinates from weighted targets.
    pub fn from_graph(graph: &RateGraph, config: RetinaMapConfig) -> Result<Self, RateModelError> {
        if config.width == 0
            || config.height == 0
            || !config.input_multiplier.is_finite()
            || config.input_multiplier <= 0.0
            || config.input_types.is_empty()
            || config.input_types.windows(2).any(|pair| pair[0] >= pair[1])
            || config.unknown_side_policy != "retain"
            || config.coordinate_rule != "assigned_ol_hex_then_weighted_outgoing_target"
            || config.input_types.iter().any(|type_name| {
                channel_for_type(type_name, config.r7_grayscale_proxy, &config.input_types)
                    .is_none()
            })
        {
            return Err(RateModelError::Invalid(
                "retina map configuration is invalid".to_owned(),
            ));
        }
        let mut coordinates = Vec::with_capacity(graph.neuron_count());
        for metadata in &graph.neuron_metadata {
            coordinates.push(own_coordinate(metadata));
        }
        let mut candidates = Vec::new();
        for (index, metadata) in graph.neuron_metadata.iter().enumerate() {
            let Some(type_name) = metadata.type_name.as_deref() else {
                continue;
            };
            if !is_receptor_type(type_name) {
                continue;
            }
            let (coordinate, inference, source) = if let Some(coordinate) = coordinates[index] {
                (Some(coordinate), None, "self".to_owned())
            } else {
                let inferred = infer_target_coordinate(graph, index, &coordinates);
                (
                    inferred.as_ref().map(|value| value.coordinate),
                    inferred,
                    "weighted_outgoing_target".to_owned(),
                )
            };
            candidates.push((
                index,
                type_name.to_owned(),
                metadata.soma_side.clone(),
                channel_for_type(type_name, config.r7_grayscale_proxy, &config.input_types),
                coordinate,
                inference,
                source,
            ));
        }
        let bounds = coordinate_bounds(
            coordinates
                .iter()
                .flatten()
                .copied()
                .chain(candidates.iter().filter_map(|candidate| candidate.4)),
        );
        let mut entries = Vec::new();
        let mut audit = RetinaAudit {
            receptor_neuron_count: candidates.len(),
            mapped_neuron_count: 0,
            missing_coordinate_count: 0,
            unsupported_type_count: 0,
            left_count: 0,
            right_count: 0,
            unknown_side_count: 0,
            unsupported_types: BTreeMap::new(),
            input_types: config.input_types.clone(),
            unknown_side_policy: config.unknown_side_policy.clone(),
            coordinate_rule: config.coordinate_rule.clone(),
            inferred_coordinate_count: 0,
            inferred_coordinate_spread: Vec::new(),
            inferred_contact_count: 0,
            inferred_majority_ratio: Vec::new(),
            inferred_target_type_counts: BTreeMap::new(),
            unknown_side_mapped_count: 0,
        };
        for (index, type_name, side, channel, coordinate, inference, source) in candidates {
            match side.as_deref() {
                Some("L") => audit.left_count += 1,
                Some("R") => audit.right_count += 1,
                _ => audit.unknown_side_count += 1,
            }
            let Some(channel) = channel else {
                audit.unsupported_type_count += 1;
                *audit.unsupported_types.entry(type_name).or_default() += 1;
                continue;
            };
            let Some((x, y)) = coordinate else {
                audit.missing_coordinate_count += 1;
                continue;
            };
            if let Some(inference) = &inference {
                audit.inferred_coordinate_count += 1;
                audit.inferred_coordinate_spread.push(inference.spread);
                audit.inferred_contact_count += inference.contact_count;
                audit.inferred_majority_ratio.push(inference.majority_ratio);
                for (target_type, count) in &inference.target_type_counts {
                    *audit
                        .inferred_target_type_counts
                        .entry(target_type.clone())
                        .or_default() += count;
                }
            }
            if side.as_deref() != Some("L") && side.as_deref() != Some("R") {
                audit.unknown_side_mapped_count += 1;
            }
            let (x_min, x_max, y_min, y_max) = bounds;
            entries.push(RetinaMapEntry {
                neuron_index: index as u32,
                type_name,
                side,
                x: normalize(x, x_min, x_max),
                y: normalize(y, y_min, y_max),
                channel,
                coordinate_source: source,
                inferred_coordinate_spread: inference.as_ref().map(|value| value.spread),
                inferred_contact_count: inference.as_ref().map(|value| value.contact_count),
                inferred_majority_target_type: inference
                    .as_ref()
                    .and_then(|value| value.majority_target_type.clone()),
                inferred_majority_ratio: inference.as_ref().map(|value| value.majority_ratio),
                inferred_target_type_counts: inference
                    .map(|value| value.target_type_counts)
                    .unwrap_or_default(),
            });
            audit.mapped_neuron_count += 1;
        }
        Ok(Self {
            config,
            entries,
            audit,
        })
    }

    /// Applies the map and nfly-style `input_multiplier * (2C - 1)` encoding.
    pub fn encode(
        &self,
        image: &RgbImage,
        neuron_count: usize,
    ) -> Result<RetinaInput, RateModelError> {
        if image.width != self.config.width || image.height != self.config.height {
            return Err(RateModelError::Invalid(
                "retina image dimensions do not match map".to_owned(),
            ));
        }
        let mut input = vec![0.0; neuron_count];
        for entry in &self.entries {
            let value = sample(image, entry.x, entry.y, entry.channel, self.config.flip_y);
            input[entry.neuron_index as usize] = self.config.input_multiplier * (2.0 * value - 1.0);
        }
        Ok(RetinaInput {
            input,
            audit: self.audit.clone(),
        })
    }
}

/// Three categories used by the fixed screen-fixture generator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenFixtureKind {
    /// Identical images on both sides of a pair.
    Same,
    /// One local row or cursor-like change.
    Near,
    /// A different deterministic screen pattern.
    Unrelated,
}

/// A deterministic RGB fixture and its content hash.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScreenFixture {
    /// Pair category.
    pub kind: ScreenFixtureKind,
    /// Pair index.
    pub pair_index: usize,
    /// Side label.
    pub side: String,
    /// Image width.
    pub width: usize,
    /// Image height.
    pub height: usize,
    /// Content hash over dimensions and bytes.
    pub sha256: String,
    #[serde(skip)]
    pixels: Vec<[f64; 3]>,
}

impl ScreenFixture {
    /// Generates a reproducible screen-like fixture without external assets.
    pub fn generate(
        kind: ScreenFixtureKind,
        pair_index: usize,
        side: &str,
        width: usize,
        height: usize,
    ) -> Result<Self, RateModelError> {
        if side != "a" && side != "b" {
            return Err(RateModelError::Invalid(
                "screen fixture side must be a or b".to_owned(),
            ));
        }
        let mut pixels = Vec::with_capacity(width * height);
        const UNRELATED_SCENE_OFFSET: u64 = 1_u64 << 32;
        for y in 0..height {
            for x in 0..width {
                let scene_id = match kind {
                    ScreenFixtureKind::Same | ScreenFixtureKind::Near => pair_index as u64,
                    ScreenFixtureKind::Unrelated => {
                        UNRELATED_SCENE_OFFSET + pair_index as u64 * 2 + u64::from(side == "b")
                    }
                };
                let mut rgb = screen_scene_pixel(scene_id, x, y, width, height);
                if kind == ScreenFixtureKind::Near && side == "b" {
                    // A near pair changes a broad, low-contrast screen state so
                    // the sampled neural input remains different even when no
                    // receptor lands in the local status region.
                    let shift = 0.035 + deterministic_unit(scene_id ^ 0x5a17) * 0.015;
                    rgb = [
                        (rgb[0] + shift).min(0.96),
                        (rgb[1] + shift * 0.9).min(0.96),
                        (rgb[2] + shift * 0.8).min(0.96),
                    ];
                    let status_region =
                        x >= width / 2 && x < width.saturating_mul(7) / 8 && y < height / 6;
                    if status_region {
                        rgb = [
                            (rgb[0] + 0.08).min(0.98),
                            (rgb[1] * 0.85).min(0.96),
                            (rgb[2] * 0.85).min(0.96),
                        ];
                    }
                }
                pixels.push(rgb);
            }
        }
        let image = RgbImage::new(width, height, pixels.clone())?;
        let sha256 = image_sha256(&image);
        Ok(Self {
            kind,
            pair_index,
            side: side.to_owned(),
            width,
            height,
            sha256,
            pixels,
        })
    }

    /// Returns the in-memory image.
    pub fn image(&self) -> Result<RgbImage, RateModelError> {
        RgbImage::new(self.width, self.height, self.pixels.clone())
    }

    /// Writes a dependency-free PPM fixture for external inspection.
    pub fn write_ppm(&self, path: impl AsRef<Path>) -> Result<(), RateModelError> {
        let mut bytes = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        for pixel in &self.pixels {
            bytes.extend(pixel.map(|value| (value * 255.0).round() as u8));
        }
        fs::write(path, bytes)?;
        Ok(())
    }
}

fn screen_scene_pixel(scene_id: u64, x: usize, y: usize, width: usize, height: usize) -> [f64; 3] {
    let nx = x as f64 / width.saturating_sub(1).max(1) as f64;
    let ny = y as f64 / height.saturating_sub(1).max(1) as f64;
    let noise = deterministic_unit(
        scene_id
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .wrapping_add((x as u64).wrapping_mul(0x517c_c1b7_2722_0a95))
            .wrapping_add((y as u64).wrapping_mul(0x6eed_0e9d_a4d9_4a4f)),
    );
    let background = [
        0.10 + 0.18 * nx + 0.035 * noise,
        0.13 + 0.12 * (1.0 - ny) + 0.025 * noise,
        0.18 + 0.16 * (1.0 - nx) + 0.02 * noise,
    ];
    let panel = x >= width / 12
        && x < width.saturating_mul(11) / 12
        && y >= height / 5
        && y < height.saturating_mul(4) / 5;
    let top_bar = y < height / 8;
    let side_panel = x < width / 5 && y >= height / 5 && y < height.saturating_mul(4) / 5;
    let mut rgb = if panel {
        [
            background[0] + 0.10,
            background[1] + 0.10,
            background[2] + 0.08,
        ]
    } else {
        background
    };
    if top_bar {
        rgb = [0.16 + 0.10 * nx, 0.20 + 0.08 * nx, 0.28 + 0.12 * (1.0 - nx)];
    }
    if side_panel {
        rgb = [rgb[0] * 0.78, rgb[1] * 0.86, (rgb[2] + 0.08).min(0.92)];
    }

    let text_band = panel && (y / 5 + scene_id as usize).is_multiple_of(9) && x % 13 < 9;
    if text_band {
        let accent = deterministic_unit(scene_id ^ (y as u64).wrapping_mul(31));
        rgb = [
            (0.60 + 0.18 * accent).min(0.92),
            (0.64 + 0.16 * (1.0 - accent)).min(0.92),
            0.76,
        ];
    }
    let marker = x > width.saturating_mul(3) / 4
        && y > height.saturating_mul(2) / 5
        && y < height.saturating_mul(3) / 5
        && (x / 6 + y / 6 + scene_id as usize).is_multiple_of(5);
    if marker {
        rgb = [0.78, 0.34 + 0.06 * noise, 0.20 + 0.10 * noise];
    }
    rgb.map(|value| value.clamp(0.04, 0.96))
}

fn deterministic_unit(mut value: u64) -> f64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    (value >> 11) as f64 / (1_u64 << 53) as f64
}

/// Hashes an image's dimensions and 8-bit RGB representation.
pub fn image_sha256(image: &RgbImage) -> String {
    let mut hasher = Sha256::new();
    hasher.update((image.width as u64).to_le_bytes());
    hasher.update((image.height as u64).to_le_bytes());
    for pixel in &image.pixels {
        for value in pixel {
            hasher.update([(*value * 255.0).round() as u8]);
        }
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn own_coordinate(metadata: &crate::rate::RateNeuronMetadata) -> Option<(f64, f64)> {
    match (metadata.assigned_ol_hex1, metadata.assigned_ol_hex2) {
        (Some(h1), Some(h2)) if h1.is_finite() && h2.is_finite() => {
            Some((h1 + 0.5 * h2, 3.0_f64.sqrt() * 0.5 * h2))
        }
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct InferredCoordinate {
    coordinate: (f64, f64),
    spread: f64,
    contact_count: u64,
    majority_target_type: Option<String>,
    majority_ratio: f64,
    target_type_counts: BTreeMap<String, u64>,
}

fn infer_target_coordinate(
    graph: &RateGraph,
    source: usize,
    coordinates: &[Option<(f64, f64)>],
) -> Option<InferredCoordinate> {
    let mut total = 0.0;
    let mut x = 0.0;
    let mut y = 0.0;
    let mut target_type_counts = BTreeMap::new();
    let mut target_coordinates = Vec::new();
    let start = graph.outgoing_offsets[source] as usize;
    let end = graph.outgoing_offsets[source + 1] as usize;
    for edge in &graph.outgoing_edges[start..end] {
        let Some((target_x, target_y)) = coordinates[edge.target as usize] else {
            continue;
        };
        let weight = edge.contact_count as f64;
        x += target_x * weight;
        y += target_y * weight;
        total += weight;
        if let Some(type_name) = graph.neuron_metadata[edge.target as usize]
            .type_name
            .as_ref()
        {
            *target_type_counts.entry(type_name.clone()).or_default() += edge.contact_count;
        }
        target_coordinates.push((target_x, target_y, weight));
    }
    if total == 0.0 {
        return None;
    }
    let coordinate = (x / total, y / total);
    let variance = target_coordinates
        .into_iter()
        .map(|(target_x, target_y, weight)| {
            let dx = target_x - coordinate.0;
            let dy = target_y - coordinate.1;
            weight * (dx * dx + dy * dy)
        })
        .sum::<f64>()
        / total;
    let (majority_target_type, majority_count) = target_type_counts
        .iter()
        .max_by_key(|(_, count)| **count)
        .map(|(type_name, count)| (Some(type_name.clone()), *count))
        .unwrap_or((None, 0));
    Some(InferredCoordinate {
        coordinate,
        spread: variance.sqrt(),
        contact_count: total as u64,
        majority_target_type,
        majority_ratio: majority_count as f64 / total,
        target_type_counts,
    })
}

fn is_receptor_type(type_name: &str) -> bool {
    type_name == "R1-R6" || type_name.starts_with("R7") || type_name.starts_with("R8")
}

fn channel_for_type(
    type_name: &str,
    r7_proxy: bool,
    input_types: &[String],
) -> Option<RetinaChannel> {
    if !input_types.iter().any(|value| value == type_name) {
        return None;
    }
    if type_name == "R1-R6" {
        Some(RetinaChannel::Luminance)
    } else if type_name == "R8p" {
        Some(RetinaChannel::Blue)
    } else if type_name == "R8y" {
        Some(RetinaChannel::Green)
    } else if r7_proxy && type_name.starts_with("R7") {
        Some(RetinaChannel::Luminance)
    } else {
        None
    }
}

fn coordinate_bounds(coordinates: impl Iterator<Item = (f64, f64)>) -> (f64, f64, f64, f64) {
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    for (x, y) in coordinates {
        bounds = Some(match bounds {
            Some((x_min, x_max, y_min, y_max)) => {
                (x_min.min(x), x_max.max(x), y_min.min(y), y_max.max(y))
            }
            None => (x, x, y, y),
        });
    }
    bounds.unwrap_or((0.0, 1.0, 0.0, 1.0))
}

fn normalize(value: f64, minimum: f64, maximum: f64) -> f64 {
    if maximum > minimum {
        ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0)
    } else {
        0.5
    }
}

fn sample(image: &RgbImage, x: f64, y: f64, channel: RetinaChannel, flip_y: bool) -> f64 {
    let y = if flip_y { 1.0 - y } else { y };
    let pixel_x = x.clamp(0.0, 1.0) * (image.width.saturating_sub(1) as f64);
    let pixel_y = y.clamp(0.0, 1.0) * (image.height.saturating_sub(1) as f64);
    let x0 = pixel_x.floor() as usize;
    let y0 = pixel_y.floor() as usize;
    let x1 = (x0 + 1).min(image.width - 1);
    let y1 = (y0 + 1).min(image.height - 1);
    let tx = pixel_x - x0 as f64;
    let ty = pixel_y - y0 as f64;
    let value = |pixel: [f64; 3]| match channel {
        RetinaChannel::Luminance => 0.2126 * pixel[0] + 0.7152 * pixel[1] + 0.0722 * pixel[2],
        RetinaChannel::Blue => pixel[2],
        RetinaChannel::Green => pixel[1],
    };
    let top = value(image.pixel(x0, y0)) * (1.0 - tx) + value(image.pixel(x1, y0)) * tx;
    let bottom = value(image.pixel(x0, y1)) * (1.0 - tx) + value(image.pixel(x1, y1)) * tx;
    top * (1.0 - ty) + bottom * ty
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate::{RateIncomingEdge, RateNeuronMetadata};

    #[test]
    fn map_uses_color_channels_and_reports_r7_as_unmapped_by_default() {
        let metadata = vec![
            metadata("R1-R6", Some((0.0, 0.0))),
            metadata("R8p", Some((1.0, 0.0))),
            metadata("R7y", Some((0.0, 1.0))),
        ];
        let graph = RateGraph::new(
            vec![0, 1, 2],
            vec![0, 0, 0, 0],
            Vec::new(),
            vec![0, 0, 0, 0],
            Vec::<RateIncomingEdge>::new(),
            vec![1, 1, 1],
            metadata,
            BTreeMap::new(),
        )
        .expect("graph");
        let map = RetinaMap::from_graph(
            &graph,
            RetinaMapConfig {
                width: 2,
                height: 2,
                flip_y: false,
                ..RetinaMapConfig::default()
            },
        )
        .expect("map");
        assert_eq!(map.audit.mapped_neuron_count, 2);
        assert_eq!(map.audit.missing_coordinate_count, 0);
        assert_eq!(map.audit.unsupported_type_count, 1);
        assert_eq!(
            map.audit.input_types,
            vec!["R1-R6".to_owned(), "R8p".to_owned(), "R8y".to_owned()]
        );
        assert_eq!(map.audit.unknown_side_policy, "retain");
        assert_eq!(
            map.audit.coordinate_rule,
            "assigned_ol_hex_then_weighted_outgoing_target"
        );
        assert_eq!(map.audit.unknown_side_mapped_count, 2);
        let image = RgbImage::new(
            2,
            2,
            vec![
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 1.0],
            ],
        )
        .expect("image");
        let encoded = map.encode(&image, 3).expect("encoded");
        assert!(
            (encoded.input[0] - (2.0 * (0.2126 * 0.0 + 0.7152 * 0.0 + 0.0722) - 1.0) * 5.0).abs()
                < 1.0e-12
        );
        assert!((encoded.input[1] - 5.0).abs() < 1.0e-12);
        assert_eq!(encoded.input[2], 0.0);
    }

    #[test]
    fn inferred_coordinates_and_unknown_side_policy_are_audited() {
        let mut receptor = metadata("R1-R6", None);
        receptor.body_id = 0;
        let mut target = metadata("L1", Some((1.0, 2.0)));
        target.body_id = 1;
        let graph = RateGraph::new(
            vec![0, 1],
            vec![0, 1, 1],
            vec![crate::rate::RateOutgoingEdge {
                target: 1,
                contact_count: 2,
            }],
            vec![0, 0, 1],
            vec![RateIncomingEdge {
                source: 0,
                edge_id: 0,
            }],
            vec![1, 1],
            vec![receptor, target],
            BTreeMap::new(),
        )
        .expect("graph");
        let map = RetinaMap::from_graph(&graph, RetinaMapConfig::default()).expect("map");
        assert_eq!(map.audit.inferred_coordinate_count, 1);
        assert_eq!(map.audit.inferred_contact_count, 2);
        assert_eq!(map.audit.inferred_coordinate_spread, vec![0.0]);
        assert_eq!(map.audit.inferred_majority_ratio, vec![1.0]);
        assert_eq!(map.audit.inferred_target_type_counts["L1"], 2);
        assert_eq!(map.audit.unknown_side_mapped_count, 1);
        assert_eq!(map.entries[0].coordinate_source, "weighted_outgoing_target");
        assert_eq!(
            map.entries[0].inferred_majority_target_type.as_deref(),
            Some("L1")
        );
    }

    #[test]
    fn input_types_and_r7_policy_are_explicit() {
        let graph = RateGraph::new(
            vec![0],
            vec![0, 0],
            Vec::new(),
            vec![0, 0],
            Vec::new(),
            vec![1],
            vec![metadata("R7y", Some((0.0, 0.0)))],
            BTreeMap::new(),
        )
        .expect("graph");
        let rejected = RetinaMap::from_graph(
            &graph,
            RetinaMapConfig {
                input_types: vec!["R7y".to_owned()],
                ..RetinaMapConfig::default()
            },
        );
        assert!(rejected.is_err());
        let mapped = RetinaMap::from_graph(
            &graph,
            RetinaMapConfig {
                input_types: vec!["R7y".to_owned()],
                r7_grayscale_proxy: true,
                ..RetinaMapConfig::default()
            },
        )
        .expect("R7 grayscale proxy should be explicit");
        assert_eq!(mapped.audit.input_types, vec!["R7y".to_owned()]);
        assert_eq!(mapped.audit.mapped_neuron_count, 1);
    }

    #[test]
    fn screen_fixture_categories_have_the_declared_pair_relationships() {
        let same_a =
            ScreenFixture::generate(ScreenFixtureKind::Same, 0, "a", 32, 16).expect("same a");
        let same_b =
            ScreenFixture::generate(ScreenFixtureKind::Same, 0, "b", 32, 16).expect("same b");
        let near_a =
            ScreenFixture::generate(ScreenFixtureKind::Near, 0, "a", 32, 16).expect("near a");
        let near_b =
            ScreenFixture::generate(ScreenFixtureKind::Near, 0, "b", 32, 16).expect("near b");
        let unrelated_a = ScreenFixture::generate(ScreenFixtureKind::Unrelated, 0, "a", 32, 16)
            .expect("unrelated a");
        let unrelated_b = ScreenFixture::generate(ScreenFixtureKind::Unrelated, 0, "b", 32, 16)
            .expect("unrelated b");
        assert_eq!(same_a.sha256, same_b.sha256);
        assert_ne!(near_a.sha256, near_b.sha256);
        assert_ne!(unrelated_a.sha256, unrelated_b.sha256);
    }

    #[test]
    fn unrelated_scene_namespace_does_not_overlap_same_scene_namespace() {
        let held_out_same =
            ScreenFixture::generate(ScreenFixtureKind::Same, 12, "a", 32, 16).expect("same");
        let training_unrelated =
            ScreenFixture::generate(ScreenFixtureKind::Unrelated, 6, "a", 32, 16)
                .expect("unrelated");
        assert_ne!(held_out_same.sha256, training_unrelated.sha256);
    }

    fn metadata(type_name: &str, coordinate: Option<(f64, f64)>) -> RateNeuronMetadata {
        RateNeuronMetadata {
            body_id: 0,
            type_name: Some(type_name.to_owned()),
            class: None,
            superclass: None,
            subclass: None,
            soma_side: None,
            root_side: None,
            assigned_ol_hex1: coordinate.map(|value| value.0),
            assigned_ol_hex2: coordinate.map(|value| value.1),
            selected_neurotransmitter: None,
            neurotransmitter_source: "test".to_owned(),
            source_sign: 1,
        }
    }
}
