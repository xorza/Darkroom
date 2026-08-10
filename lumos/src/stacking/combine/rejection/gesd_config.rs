//! Generalized Extreme Studentized Deviate: test for up to `k` outliers against Student-t critical
//! values, for stacks large enough to make the test's Gaussian assumption reasonable.

use crate::error::InvalidConfigField;
use crate::stacking::combine::rejection::scratch_buffers::ScratchBuffers;
use statrs::distribution::{ContinuousCDF, StudentsT};

/// Configuration for Generalized Extreme Studentized Deviate (GESD) test.
///
/// A rigorous statistical test for detecting multiple outliers in approximately Gaussian samples.
/// The stacking preset enables it from 15 frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GesdConfig {
    /// Significance level for the test (typically 0.05).
    pub alpha: f32,
    /// Maximum number of outliers to detect.
    /// If `None`, targets 25% of the data within Rosner's validated limits: at most two
    /// candidates below 25 samples and at most ten otherwise.
    pub max_outliers: Option<usize>,
}

impl Default for GesdConfig {
    fn default() -> Self {
        Self {
            alpha: 0.05,
            max_outliers: None,
        }
    }
}

impl GesdConfig {
    pub fn new(alpha: f32, max_outliers: Option<usize>) -> Self {
        Self {
            alpha,
            max_outliers,
        }
    }

    /// Validate the significance level.
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        InvalidConfigField::finite("GESD alpha", "finite and in [0, 1)", self.alpha, |value| {
            (0.0..1.0).contains(&value)
        })
    }

    /// Get the configured maximum or the validation-constrained automatic maximum.
    pub fn max_outliers_for_size(&self, n: usize) -> usize {
        self.max_outliers.unwrap_or_else(|| {
            let validation_cap = if n < 25 { 2 } else { 10 };
            (n / 4).min(validation_cap)
        })
    }

    /// Partition values by GESD test, returning the number of survivors.
    ///
    /// Uses Rosner's two-sided statistic: distance from the sample mean divided by sample standard
    /// deviation, with critical values from the Student-t inverse CDF.
    ///
    /// After return, `values[..remaining]` contains surviving values and
    /// `indices[..remaining]` contains their original frame indices.
    pub(super) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        debug_assert!(!values.is_empty());

        let original_len = values.len();
        scratch.reset_indices(original_len);

        if original_len <= 3 {
            return original_len;
        }

        let max_outliers = self
            .max_outliers_for_size(original_len)
            .min(original_len - 3);
        prepare_gesd_critical_values(self, original_len, max_outliers, scratch);
        let mut len = original_len;
        let mut mean = 0.0f64;
        let mut squared_deviations = 0.0f64;
        for (index, &value) in values.iter().enumerate() {
            let value = f64::from(value);
            let delta = value - mean;
            mean += delta / (index + 1) as f64;
            squared_deviations += delta * (value - mean);
        }

        scratch.gesd.statistics.clear();

        for _ in 0..max_outliers {
            let sample_deviation = (squared_deviations / (len - 1) as f64).sqrt();
            if sample_deviation == 0.0 {
                break;
            }

            let mut max_deviation = 0.0f64;
            let mut max_idx = 0;
            for (idx, &value) in values[..len].iter().enumerate() {
                let deviation = (f64::from(value) - mean).abs();
                if deviation > max_deviation {
                    max_deviation = deviation;
                    max_idx = idx;
                }
            }

            scratch
                .gesd
                .statistics
                .push(max_deviation / sample_deviation);

            let removed = f64::from(values[max_idx]);
            let next_len = len - 1;
            let next_mean = mean - (removed - mean) / next_len as f64;
            // Reverse Welford's update so each candidate needs only the extreme-value scan.
            squared_deviations =
                (squared_deviations - (removed - mean) * (removed - next_mean)).max(0.0);
            mean = next_mean;
            values.swap(max_idx, len - 1);
            scratch.indices.swap(max_idx, len - 1);
            len = next_len;
        }

        let num_outliers = scratch
            .gesd
            .statistics
            .iter()
            .zip(&scratch.gesd.critical_values)
            .rposition(|(statistic, critical)| statistic > critical)
            .map_or(0, |index| index + 1);
        original_len - num_outliers
    }
}

fn prepare_gesd_critical_values(
    config: &GesdConfig,
    sample_count: usize,
    max_outliers: usize,
    scratch: &mut ScratchBuffers,
) {
    if scratch.gesd.sample_count == sample_count
        && scratch.gesd.critical_values.len() == max_outliers
        && scratch.gesd.alpha_bits == config.alpha.to_bits()
    {
        return;
    }

    scratch.gesd.critical_values.clear();
    for removed in 0..max_outliers {
        let live_count = sample_count - removed;
        let live = live_count as f64;
        let probability = 1.0 - f64::from(config.alpha) / (2.0 * live);
        let distribution = StudentsT::new(0.0, 1.0, (live_count - 2) as f64)
            .expect("GESD live sample count guarantees positive degrees of freedom");
        let critical_t = distribution.inverse_cdf(probability);
        let critical =
            (live - 1.0) / (live * (1.0 + (live - 2.0) / (critical_t * critical_t))).sqrt();
        scratch.gesd.critical_values.push(critical);
    }

    scratch.gesd.sample_count = sample_count;
    scratch.gesd.alpha_bits = config.alpha.to_bits();
}
