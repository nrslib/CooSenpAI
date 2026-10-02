use serde::{Deserialize, Serialize};

use crate::rate::RateModelError;

/// Configuration for the readout-side, activity-dependent habituation state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HabituationConfig {
    /// Recovery time constant in the caller's seconds.
    pub recovery_tau_s: f64,
    /// Decay per unit normalized activity and presentation amount.
    pub beta: f64,
    /// Small denominator used when deciding whether a response exists.
    pub response_epsilon: f64,
    /// Minimum summed normalized activity for a sufficient response.
    pub minimum_response_activity: f64,
}

impl Default for HabituationConfig {
    fn default() -> Self {
        Self {
            recovery_tau_s: 60.0,
            beta: 0.2,
            response_epsilon: 1.0e-9,
            minimum_response_activity: 1.0e-6,
        }
    }
}

impl HabituationConfig {
    fn validate(&self) -> Result<(), RateModelError> {
        if !self.recovery_tau_s.is_finite()
            || self.recovery_tau_s <= 0.0
            || !self.beta.is_finite()
            || self.beta < 0.0
            || !self.response_epsilon.is_finite()
            || self.response_epsilon <= 0.0
            || !self.minimum_response_activity.is_finite()
            || self.minimum_response_activity < 0.0
        {
            return Err(RateModelError::Invalid(
                "habituation configuration is invalid".to_owned(),
            ));
        }
        Ok(())
    }
}

/// State kept between observations for one stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HabituationState {
    /// Per-neuron availability, initially one.
    pub availability: Vec<f64>,
    /// Last logical observation time.
    pub last_time_s: Option<f64>,
    /// Fixed habituation configuration.
    pub config: HabituationConfig,
}

/// Output of one habituation observation.
#[derive(Clone, Debug, PartialEq)]
pub struct HabituationResult {
    /// Activity after applying the short-term decay.
    pub habituated_activity: Vec<f64>,
    /// Activity-dependent normalized change before decay.
    pub normalized_activity: Vec<f64>,
    /// Novelty score, absent when the response is too small to assess.
    pub novelty_score: Option<f64>,
    /// Whether the readout had enough activity for the score.
    pub response_sufficient: bool,
    /// Availability immediately before this presentation's decay.
    pub availability_before_decay: Vec<f64>,
}

impl HabituationState {
    /// Creates an independent habituation stream.
    pub fn new(neuron_count: usize, config: HabituationConfig) -> Result<Self, RateModelError> {
        config.validate()?;
        Ok(Self {
            availability: vec![1.0; neuron_count],
            last_time_s: None,
            config,
        })
    }

    /// Resets availability and time without changing the configuration.
    pub fn reset(&mut self) {
        self.availability.fill(1.0);
        self.last_time_s = None;
    }

    /// Applies recovery, computes a readout-side decay, then records presentation use.
    pub fn observe(
        &mut self,
        activity: &[f64],
        rest: &[f64],
        habituation_scale: &[f64],
        now_s: f64,
        presentation_amount: f64,
    ) -> Result<HabituationResult, RateModelError> {
        self.config.validate()?;
        if activity.len() != self.availability.len()
            || rest.len() != activity.len()
            || habituation_scale.len() != activity.len()
            || activity
                .iter()
                .chain(rest)
                .chain(habituation_scale)
                .any(|value| !value.is_finite())
            || !now_s.is_finite()
            || !presentation_amount.is_finite()
            || presentation_amount < 0.0
        {
            return Err(RateModelError::Invalid(
                "habituation observation has invalid dimensions or values".to_owned(),
            ));
        }
        if let Some(previous) = self.last_time_s
            && now_s < previous
        {
            return Err(RateModelError::Invalid(
                "habituation observation time moved backwards".to_owned(),
            ));
        }
        if let Some(previous) = self.last_time_s {
            let recovery = (-((now_s - previous) / self.config.recovery_tau_s)).exp();
            for availability in &mut self.availability {
                *availability = 1.0 - (1.0 - *availability) * recovery;
            }
        }
        let availability_before_decay = self.availability.clone();
        let mut normalized_activity = Vec::with_capacity(activity.len());
        let mut habituated_activity = Vec::with_capacity(activity.len());
        let mut denominator = 0.0;
        let mut novelty_numerator = 0.0;
        for index in 0..activity.len() {
            let scale = habituation_scale[index]
                .abs()
                .max(self.config.response_epsilon);
            let q = ((activity[index] - rest[index]).abs() / scale).clamp(0.0, 1.0);
            normalized_activity.push(q);
            habituated_activity.push(
                rest[index] + availability_before_decay[index] * (activity[index] - rest[index]),
            );
            denominator += q;
            novelty_numerator += q * availability_before_decay[index];
            self.availability[index] *= (-self.config.beta * q * presentation_amount).exp();
        }
        self.last_time_s = Some(now_s);
        let response_sufficient = denominator >= self.config.minimum_response_activity;
        let novelty_score = response_sufficient.then(|| novelty_numerator / denominator);
        Ok(HabituationResult {
            habituated_activity,
            normalized_activity,
            novelty_score,
            response_sufficient,
            availability_before_decay,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_activity_habituates_and_recovers() {
        let mut state = HabituationState::new(2, HabituationConfig::default()).expect("state");
        let activity = [1.0, -1.0];
        let rest = [0.0, 0.0];
        let scale = [1.0, 1.0];
        let first = state
            .observe(&activity, &rest, &scale, 0.0, 1.0)
            .expect("first");
        let second = state
            .observe(&activity, &rest, &scale, 0.0, 1.0)
            .expect("second");
        assert!(
            first.novelty_score.expect("first score") > second.novelty_score.expect("second score")
        );
        assert!(second.habituated_activity[0] < first.habituated_activity[0]);
        let recovered = state
            .observe(&activity, &rest, &scale, 600.0, 0.0)
            .expect("recovered");
        assert!(
            recovered.novelty_score.expect("recovered score")
                > second.novelty_score.expect("second score")
        );
    }

    #[test]
    fn no_response_is_reported_instead_of_being_called_novel() {
        let mut state = HabituationState::new(1, HabituationConfig::default()).expect("state");
        let result = state
            .observe(&[0.0], &[0.0], &[1.0], 0.0, 1.0)
            .expect("observation");
        assert!(!result.response_sufficient);
        assert!(result.novelty_score.is_none());
    }
}
