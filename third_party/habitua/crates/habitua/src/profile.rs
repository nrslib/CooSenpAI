use std::io::{self, Read, Write};

use crate::persistence::{LoadBudget, read_f64, read_u64, write_f64};

#[derive(Clone, Debug)]
pub(crate) struct FeatureProfile {
    mean: Vec<f64>,
    squared_deviation: Vec<f64>,
    sample_weight: f64,
}

impl FeatureProfile {
    pub(crate) fn new(dimensions: usize) -> Self {
        Self {
            mean: vec![0.0; dimensions],
            squared_deviation: vec![0.0; dimensions],
            sample_weight: 0.0,
        }
    }

    /// Returns the normalized deviation from the running feature profile.
    ///
    /// An empty profile has no baseline and therefore returns `1.0`.
    /// Subsequent observations use the population variance with the configured
    /// positive `variance_floor`, then map the root-mean-square z-score with
    /// `1 - exp(-distance / temporal_shift_scale)`. The profile is updated
    /// only after this value has been calculated by the caller.
    pub(crate) fn deviation(
        &self,
        features: &[f32],
        temporal_shift_scale: f32,
        variance_floor: f32,
    ) -> f32 {
        if self.sample_weight == 0.0 {
            return 1.0;
        }

        let count = self.sample_weight;
        let distance_squared = features
            .iter()
            .zip(self.mean.iter().zip(&self.squared_deviation))
            .map(|(feature, (mean, squared_deviation))| {
                let variance = (*squared_deviation / count).max(0.0);
                let scale = variance.sqrt().max(f64::from(variance_floor));
                let z_score = (f64::from(*feature) - *mean) / scale;
                z_score * z_score
            })
            .sum::<f64>();
        let normalized_distance = (distance_squared / features.len() as f64).sqrt();
        (1.0 - (-normalized_distance / f64::from(temporal_shift_scale)).exp()) as f32
    }

    pub(crate) fn update_weighted(&mut self, features: &[f32], weight: f32) -> bool {
        if !weight.is_finite() || weight <= 0.0 {
            return false;
        }
        let weight = f64::from(weight);
        let next_weight = self.sample_weight + weight;
        if !next_weight.is_finite() {
            return false;
        }
        if self.sample_weight == 0.0 {
            for (index, feature) in features.iter().enumerate() {
                self.mean[index] = f64::from(*feature);
            }
            self.sample_weight = weight;
            return true;
        }

        for (index, feature) in features.iter().enumerate() {
            let value = f64::from(*feature);
            let delta = value - self.mean[index];
            self.mean[index] += delta * weight / next_weight;
            let delta_after_update = value - self.mean[index];
            self.squared_deviation[index] += weight * delta * delta_after_update;
        }
        self.sample_weight = next_weight;
        true
    }

    pub(crate) fn save(&self, writer: &mut dyn Write) -> io::Result<()> {
        write_f64(writer, self.sample_weight)?;
        for mean in &self.mean {
            write_f64(writer, *mean)?;
        }
        for squared_deviation in &self.squared_deviation {
            write_f64(writer, *squared_deviation)?;
        }
        Ok(())
    }

    pub(crate) fn reserve_load_budget(&self, load_budget: &mut LoadBudget) -> io::Result<()> {
        load_budget.reserve(
            self.mean
                .len()
                .checked_mul(2)
                .ok_or_else(|| invalid_data("feature profile dimensions overflowed"))?,
        )
    }

    pub(crate) fn load(
        reader: &mut dyn Read,
        dimensions: usize,
        load_budget: &mut LoadBudget,
        format_version: u16,
    ) -> io::Result<Self> {
        let sample_weight = if format_version >= crate::persistence::CURRENT_FORMAT_VERSION {
            read_f64(reader)?
        } else {
            read_u64(reader)? as f64
        };
        if !sample_weight.is_finite() || sample_weight < 0.0 {
            return Err(invalid_data("feature profile sample weight is invalid"));
        }
        let profile_elements = dimensions
            .checked_mul(2)
            .ok_or_else(|| invalid_data("feature profile dimensions overflowed"))?;
        load_budget.reserve(profile_elements)?;
        let mut mean = Vec::with_capacity(dimensions);
        for _ in 0..dimensions {
            let value = read_f64(reader)?;
            if !value.is_finite() {
                return Err(invalid_data("feature profile mean is not finite"));
            }
            mean.push(value);
        }
        let mut squared_deviation = Vec::with_capacity(dimensions);
        for _ in 0..dimensions {
            let value = read_f64(reader)?;
            if !value.is_finite() || value < 0.0 {
                return Err(invalid_data("feature profile variance is invalid"));
            }
            squared_deviation.push(value);
        }
        if sample_weight == 0.0
            && (mean.iter().any(|value| *value != 0.0)
                || squared_deviation.iter().any(|value| *value != 0.0))
        {
            return Err(invalid_data("empty feature profile contains state"));
        }
        Ok(Self {
            mean,
            squared_deviation,
            sample_weight,
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sample_weight == 0.0
    }

    pub(crate) fn can_update(&self, weight: f32) -> bool {
        let next_weight = self.sample_weight + f64::from(weight);
        weight.is_finite() && weight > 0.0 && next_weight.is_finite()
    }
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
