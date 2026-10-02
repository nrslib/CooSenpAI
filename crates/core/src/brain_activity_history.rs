use crate::brain_activity::{
    BrainActivityEntry, BrainActivityHistory, BrainActivityObservation, BrainActivityPreview,
    BrainActivityRecord, BrainActivitySummary,
};
use image::{codecs::png::PngEncoder, ImageEncoder, ImageReader};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashSet, VecDeque};
use std::io::{Cursor, Read, Write};

pub(crate) const MAX_ACTIVITY_OBSERVATIONS: usize = 5;
const MAX_PREVIEW_INPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_PREVIEW_INPUTS: usize = 8;
const MAX_PREVIEW_BYTES: usize = 1024 * 1024;
const MAX_PREVIEW_WIDTH: u32 = 640;
const MAX_PREVIEW_HEIGHT: u32 = 360;

#[derive(Debug)]
pub(crate) struct BrainActivityStore {
    generation: u64,
    revision: u64,
    latest: Option<BrainActivitySummary>,
    entries: VecDeque<BrainActivityRecord>,
}
impl Default for BrainActivityStore {
    fn default() -> Self {
        Self {
            generation: 1,
            revision: 0,
            latest: None,
            entries: VecDeque::new(),
        }
    }
}
impl BrainActivityStore {
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
    pub(crate) fn reset(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.revision = 0;
        self.latest = None;
        self.entries.clear();
        self.generation
    }
    pub(crate) fn insert(&mut self, generation: u64, observation: BrainActivityObservation) {
        if generation != self.generation {
            return;
        }
        self.revision = self.revision.saturating_add(1);
        self.latest = Some(observation.summary());
        if !observation
            .modules
            .iter()
            .any(|module| module.activity.is_some())
        {
            return;
        }
        self.entries
            .retain(|entry| entry.observation.input_id != observation.input_id);
        while self.entries.len() >= MAX_ACTIVITY_OBSERVATIONS {
            self.entries.pop_back();
        }
        self.entries.push_front(BrainActivityRecord {
            generation,
            record_id: self.revision,
            observation,
        });
    }
    pub(crate) fn history(&self) -> BrainActivityHistory {
        BrainActivityHistory {
            generation: self.generation,
            revision: self.revision,
            latest: self.latest.clone(),
            entries: self
                .entries
                .iter()
                .map(|record| BrainActivityEntry {
                    record_id: record.record_id,
                    observation: record.observation.summary(),
                })
                .collect(),
        }
    }
    pub(crate) fn record(
        &self,
        generation: u64,
        record_id: u64,
        input_id: &str,
    ) -> Option<BrainActivityRecord> {
        if generation != self.generation {
            return None;
        }
        self.entries
            .iter()
            .find(|entry| entry.record_id == record_id && entry.observation.input_id == input_id)
            .cloned()
    }
    pub(crate) fn for_input(&self, input_id: &str) -> Option<BrainActivityObservation> {
        // The legacy accessor reports the latest state, including cache-only updates.
        if let Some(latest) = self
            .latest
            .as_ref()
            .filter(|latest| latest.input_id == input_id)
        {
            if let Some(record) = self.entries.front().filter(|entry| {
                entry.record_id == self.revision && entry.observation.input_id == input_id
            }) {
                return Some(record.observation.clone());
            }
            return Some(BrainActivityObservation {
                input_id: latest.input_id.clone(),
                occurred_at: latest.occurred_at.clone(),
                mode: latest.mode,
                decision: latest.decision.clone(),
                modules: latest
                    .modules
                    .iter()
                    .map(|module| crate::brain_activity::BrainActivityModule {
                        module_index: module.module_index,
                        status: module.status.clone(),
                        unavailable_reason: module.unavailable_reason.clone(),
                        activity: None,
                        evaluation: None,
                        hold_reason: None,
                        error: None,
                        preview: None,
                    })
                    .collect(),
            });
        }
        self.entries
            .iter()
            .find(|entry| entry.observation.input_id == input_id)
            .map(|entry| entry.observation.clone())
    }
}

pub(crate) fn attach_previews(observation: &mut BrainActivityObservation, request: &Value) {
    let hashes: HashSet<_> = observation
        .modules
        .iter()
        .filter_map(|module| {
            module
                .activity
                .as_ref()
                .map(|activity| activity.image_sha256.as_str())
        })
        .collect();
    if hashes.is_empty() {
        return;
    }
    let params = &request["params"];
    let candidates = params["image_path"].as_str().into_iter().chain(
        params["context"]["frames"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|frame| frame["image_path"].as_str()),
    );
    let mut seen = HashSet::new();
    let mut previews = Vec::new();
    for path in candidates
        .filter(|path| !path.is_empty())
        .filter(|path| seen.insert(*path))
        .take(MAX_PREVIEW_INPUTS)
    {
        if let Some(preview) = matching_preview(path, &hashes) {
            previews.push(preview);
        }
    }
    for module in &mut observation.modules {
        if let Some(activity) = &module.activity {
            module.preview = previews
                .iter()
                .find(|preview| preview.image_sha256 == activity.image_sha256)
                .cloned();
        }
    }
}

fn matching_preview(path: &str, hashes: &HashSet<&str>) -> Option<BrainActivityPreview> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take((MAX_PREVIEW_INPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_PREVIEW_INPUT_BYTES {
        return None;
    }
    let dimensions = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    let limits = crate::image_processing::ImageLimits::default();
    if dimensions.0 == 0
        || dimensions.1 == 0
        || dimensions.0 > limits.max_width
        || dimensions.1 > limits.max_height
        || u64::from(dimensions.0) * u64::from(dimensions.1) * 4 > limits.max_decoded_bytes
    {
        return None;
    }
    let mut reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .ok()?;
    let mut decode_limits = image::Limits::default();
    decode_limits.max_image_width = Some(limits.max_width);
    decode_limits.max_image_height = Some(limits.max_height);
    decode_limits.max_alloc = Some(limits.max_decoded_bytes);
    reader.limits(decode_limits);
    let image = reader.decode().ok()?.to_rgb8();
    // MaleCNS helper hashes dimensions as little-endian u64 then row-major RGB8.
    let mut hasher = Sha256::new();
    hasher.update(u64::from(image.width()).to_le_bytes());
    hasher.update(u64::from(image.height()).to_le_bytes());
    hasher.update(image.as_raw());
    let image_sha256 = format!("{:x}", hasher.finalize());
    if !hashes.contains(image_sha256.as_str()) {
        return None;
    }
    let thumbnail = image::DynamicImage::ImageRgb8(image)
        .thumbnail(MAX_PREVIEW_WIDTH, MAX_PREVIEW_HEIGHT)
        .to_rgb8();
    let mut output = LimitedPreview(Vec::new());
    PngEncoder::new(&mut output)
        .write_image(
            &thumbnail,
            thumbnail.width(),
            thumbnail.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    Some(BrainActivityPreview {
        image_sha256,
        width: thumbnail.width(),
        height: thumbnail.height(),
        png_hex: output.0.iter().map(|byte| format!("{byte:02x}")).collect(),
    })
}
struct LimitedPreview(Vec<u8>);
impl Write for LimitedPreview {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_PREVIEW_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("activity preview exceeds byte limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
