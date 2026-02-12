//! Unified stacking configuration.
//!
//! This module provides a single `StackConfig` type that encapsulates all stacking
//! parameters: combination method, pixel rejection, normalization, and memory settings.

use crate::math;
use crate::stacking::CacheConfig;
use crate::stacking::cache::ScratchBuffers;
use crate::stacking::rejection::{
    AsymmetricSigmaClipConfig, GesdConfig, LinearFitClipConfig, PercentileClipConfig,
    RejectionResult, SigmaClipConfig, WinsorizedClipConfig,
};

/// Method for combining pixel values across frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CombineMethod {
    /// Mean value. If weights are provided, computes weighted mean.
    #[default]
    Mean,
    /// Median value (implicit outlier rejection).
    Median,
}

/// Pixel rejection algorithm applied before combining.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rejection {
    /// No rejection.
    None,
    /// Iterative sigma clipping from median.
    SigmaClip(SigmaClipConfig),
    /// Sigma clipping with asymmetric thresholds.
    SigmaClipAsymmetric(AsymmetricSigmaClipConfig),
    /// Replace outliers with boundary values (better for small stacks).
    Winsorized(WinsorizedClipConfig),
    /// Fit linear trend, reject deviants (good for gradients).
    LinearFit(LinearFitClipConfig),
    /// Clip lowest/highest percentiles.
    Percentile(PercentileClipConfig),
    /// Generalized ESD test (best for large stacks >50 frames).
    Gesd(GesdConfig),
}

impl Default for Rejection {
    fn default() -> Self {
        Self::SigmaClip(SigmaClipConfig::new(2.5, 3))
    }
}

impl Rejection {
    /// Create sigma clipping with default iterations.
    pub fn sigma_clip(sigma: f32) -> Self {
        Self::SigmaClip(SigmaClipConfig::new(sigma, 3))
    }

    /// Create asymmetric sigma clipping.
    pub fn sigma_clip_asymmetric(sigma_low: f32, sigma_high: f32) -> Self {
        Self::SigmaClipAsymmetric(AsymmetricSigmaClipConfig::new(sigma_low, sigma_high, 3))
    }

    /// Create winsorized sigma clipping.
    pub fn winsorized(sigma: f32) -> Self {
        Self::Winsorized(WinsorizedClipConfig::new(sigma, 3))
    }

    /// Create linear fit clipping with symmetric thresholds.
    pub fn linear_fit(sigma: f32) -> Self {
        Self::LinearFit(LinearFitClipConfig::new(sigma, sigma, 3))
    }

    /// Create percentile clipping with symmetric bounds.
    pub fn percentile(percent: f32) -> Self {
        Self::Percentile(PercentileClipConfig::new(percent, percent))
    }

    /// Create GESD with default alpha.
    pub fn gesd() -> Self {
        Self::Gesd(GesdConfig::new(0.05, None))
    }

    /// Partition values by rejection algorithm, returning the number of survivors.
    ///
    /// After return, `values[..remaining]` contains surviving values.
    /// For `Winsorized`, no partitioning occurs (returns `values.len()`).
    pub(crate) fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        match self {
            Rejection::None => values.len(),
            Rejection::SigmaClip(c) => c.reject(values, &mut scratch.indices),
            Rejection::SigmaClipAsymmetric(c) => c.reject(values, &mut scratch.indices),
            Rejection::LinearFit(c) => c.reject(values, &mut scratch.indices),
            Rejection::Gesd(c) => c.reject(values, &mut scratch.indices),
            Rejection::Percentile(c) => c.reject(values),
            Rejection::Winsorized(_) => values.len(),
        }
    }

    /// Reject outliers then compute (weighted) mean.
    ///
    /// Uses index tracking to maintain correct value-weight alignment after rejection
    /// functions partition/reorder the values array.
    pub(crate) fn combine_mean(
        &self,
        values: &mut [f32],
        weights: Option<&[f32]>,
        scratch: &mut ScratchBuffers,
    ) -> RejectionResult {
        // Winsorized and weighted-Percentile are special cases
        match self {
            Rejection::Winsorized(config) => {
                let winsorized =
                    config.winsorize(values, &mut scratch.floats_a, &mut scratch.floats_b);
                let value = match weights {
                    Some(w) => math::weighted_mean_f32(winsorized, w),
                    None => math::mean_f32(winsorized),
                };
                return RejectionResult {
                    value,
                    remaining_count: values.len(),
                };
            }

            Rejection::Percentile(config) if weights.is_some() => {
                let w = weights.unwrap();
                scratch.pairs.clear();
                scratch
                    .pairs
                    .extend(values.iter().zip(w.iter()).map(|(&v, &wt)| (v, wt)));
                scratch
                    .pairs
                    .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

                let n = scratch.pairs.len();
                let low_count = ((config.low_percentile / 100.0) * n as f32).floor() as usize;
                let high_count = ((config.high_percentile / 100.0) * n as f32).floor() as usize;
                let start = low_count;
                let end = n.saturating_sub(high_count);
                let (start, end) = if start >= end {
                    let mid = n / 2;
                    (mid, mid + 1)
                } else {
                    (start, end)
                };

                let remaining = &scratch.pairs[start..end];
                let value = math::weighted_mean_pairs_f32(remaining);
                return RejectionResult {
                    value,
                    remaining_count: remaining.len(),
                };
            }

            _ => {}
        }

        // Common path: reject then compute mean
        let remaining = self.reject(values, scratch);

        let value = match (weights, self) {
            (Some(w), Rejection::None) => math::weighted_mean_f32(values, w),
            (_, Rejection::Percentile(_)) => math::mean_f32(&values[..remaining]),
            (Some(w), _) if remaining > 0 => {
                weighted_mean_indexed(&values[..remaining], w, &scratch.indices[..remaining])
            }
            _ => math::mean_f32(&values[..remaining]),
        };

        RejectionResult {
            value,
            remaining_count: remaining,
        }
    }
}

/// Compute weighted mean using index mapping.
///
/// `indices[i]` maps `values[i]` to `weights[indices[i]]`, maintaining correct
/// alignment after rejection functions have reordered the values array.
fn weighted_mean_indexed(values: &[f32], weights: &[f32], indices: &[usize]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }

    let mut sum = 0.0f32;
    let mut weight_sum = 0.0f32;

    for (i, &v) in values.iter().enumerate() {
        let w = weights[indices[i]];
        sum += v * w;
        weight_sum += w;
    }

    if weight_sum > f32::EPSILON {
        sum / weight_sum
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

/// Frame normalization method applied before stacking.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Normalization {
    /// No normalization.
    #[default]
    None,
    /// Match global median and scale across frames (additive + scaling).
    /// Best for light frames.
    Global,
    /// Scale by ratio of medians (no additive offset).
    /// Best for flat frames where exposure varies.
    Multiplicative,
}

/// Unified configuration for image stacking operations.
///
/// # Examples
///
/// ```ignore
/// use lumos::stacking::{stack, StackConfig, FrameType};
///
/// // Simple sigma-clipped stacking (default)
/// let result = stack(&paths, FrameType::Light, StackConfig::default())?;
///
/// // Using presets
/// let result = stack(&paths, FrameType::Light, StackConfig::sigma_clipped(2.0))?;
/// let result = stack(&paths, FrameType::Light, StackConfig::median())?;
///
/// // Custom configuration
/// let config = StackConfig {
///     rejection: Rejection::sigma_clip_asymmetric(2.0, 3.0),
///     normalization: Normalization::Global,
///     ..Default::default()
/// };
/// let result = stack(&paths, FrameType::Light, config)?;
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct StackConfig {
    /// How to combine pixel values across frames.
    pub method: CombineMethod,
    /// Pixel rejection to remove outliers before combining.
    pub rejection: Rejection,
    /// Per-frame weights (empty = equal weights).
    pub weights: Vec<f32>,
    /// Frame normalization before stacking.
    pub normalization: Normalization,
    /// Cache/memory behavior.
    pub cache: CacheConfig,
}

impl Default for StackConfig {
    fn default() -> Self {
        Self {
            method: CombineMethod::Mean,
            rejection: Rejection::default(),
            weights: vec![],
            normalization: Normalization::None,
            cache: CacheConfig::default(),
        }
    }
}

impl StackConfig {
    // ========== Presets ==========

    /// Preset: sigma-clipped mean (most common for light frames).
    pub fn sigma_clipped(sigma: f32) -> Self {
        Self {
            rejection: Rejection::sigma_clip(sigma),
            ..Default::default()
        }
    }

    /// Preset: median stacking (no explicit rejection needed).
    pub fn median() -> Self {
        Self {
            method: CombineMethod::Median,
            rejection: Rejection::None,
            ..Default::default()
        }
    }

    /// Preset: simple mean (fastest, for bias frames).
    pub fn mean() -> Self {
        Self {
            method: CombineMethod::Mean,
            rejection: Rejection::None,
            ..Default::default()
        }
    }

    /// Preset: weighted mean with explicit weights.
    pub fn weighted(weights: Vec<f32>) -> Self {
        Self {
            weights,
            ..Default::default()
        }
    }

    /// Preset: winsorized sigma clipping (better for small stacks).
    pub fn winsorized(sigma: f32) -> Self {
        Self {
            rejection: Rejection::winsorized(sigma),
            ..Default::default()
        }
    }

    /// Preset: linear fit clipping (good for sky gradients).
    pub fn linear_fit(sigma: f32) -> Self {
        Self {
            rejection: Rejection::linear_fit(sigma),
            ..Default::default()
        }
    }

    /// Preset: percentile clipping (simple, for small stacks <10).
    pub fn percentile(percent: f32) -> Self {
        Self {
            rejection: Rejection::percentile(percent),
            ..Default::default()
        }
    }

    /// Preset: GESD (rigorous, for large stacks >50).
    pub fn gesd() -> Self {
        Self {
            rejection: Rejection::gesd(),
            ..Default::default()
        }
    }

    // ========== Validation ==========

    /// Validate configuration parameters.
    ///
    /// # Panics
    ///
    /// Panics if parameters are invalid (e.g., negative sigma, invalid percentiles).
    pub fn validate(&self) {
        // Config structs validate in their constructors, but validate() can be
        // called on configs built via struct literal syntax, so re-check here.
        match self.rejection {
            Rejection::None => {}
            Rejection::SigmaClip(c) => {
                assert!(c.sigma > 0.0, "Sigma must be positive");
                assert!(c.max_iterations > 0, "Iterations must be at least 1");
            }
            Rejection::SigmaClipAsymmetric(c) => {
                assert!(c.sigma_low > 0.0, "Sigma low must be positive");
                assert!(c.sigma_high > 0.0, "Sigma high must be positive");
                assert!(c.max_iterations > 0, "Iterations must be at least 1");
            }
            Rejection::Winsorized(c) => {
                assert!(c.sigma > 0.0, "Sigma must be positive");
                assert!(c.max_iterations > 0, "Iterations must be at least 1");
            }
            Rejection::LinearFit(c) => {
                assert!(c.sigma_low > 0.0, "Sigma low must be positive");
                assert!(c.sigma_high > 0.0, "Sigma high must be positive");
                assert!(c.max_iterations > 0, "Iterations must be at least 1");
            }
            Rejection::Percentile(c) => {
                assert!(
                    (0.0..=50.0).contains(&c.low_percentile),
                    "Low percentile must be 0-50"
                );
                assert!(
                    (0.0..=50.0).contains(&c.high_percentile),
                    "High percentile must be 0-50"
                );
                assert!(
                    c.low_percentile + c.high_percentile < 100.0,
                    "Total clipping must be < 100%"
                );
            }
            Rejection::Gesd(c) => {
                assert!((0.0..1.0).contains(&c.alpha), "Alpha must be 0-1");
            }
        }

        if !self.weights.is_empty() {
            assert!(
                self.weights.iter().all(|&w| w >= 0.0),
                "Weights must be non-negative"
            );
        }
    }

    /// Get normalized weights (sum to 1.0).
    pub fn normalized_weights(&self) -> Vec<f32> {
        assert!(!self.weights.is_empty(), "Cannot normalize empty weights");
        let sum: f32 = self.weights.iter().sum();
        if sum > f32::EPSILON {
            self.weights.iter().map(|w| w / sum).collect()
        } else {
            let n = self.weights.len();
            vec![1.0 / n as f32; n]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = StackConfig::default();
        assert_eq!(config.method, CombineMethod::Mean);
        assert!(matches!(config.rejection, Rejection::SigmaClip(..)));
        assert!(config.weights.is_empty());
        assert_eq!(config.normalization, Normalization::None);
    }

    #[test]
    fn test_sigma_clipped_preset() {
        let config = StackConfig::sigma_clipped(2.0);
        assert_eq!(config.method, CombineMethod::Mean);
        assert!(matches!(
            config.rejection,
            Rejection::SigmaClip(c) if (c.sigma - 2.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn test_median_preset() {
        let config = StackConfig::median();
        assert_eq!(config.method, CombineMethod::Median);
        assert_eq!(config.rejection, Rejection::None);
    }

    #[test]
    fn test_weighted_preset() {
        let config = StackConfig::weighted(vec![1.0, 2.0, 3.0]);
        assert_eq!(config.method, CombineMethod::Mean);
        assert_eq!(config.weights.len(), 3);
    }

    #[test]
    fn test_struct_update_syntax() {
        let config = StackConfig {
            rejection: Rejection::SigmaClipAsymmetric(AsymmetricSigmaClipConfig::new(2.0, 3.0, 5)),
            normalization: Normalization::Global,
            ..Default::default()
        };
        assert!(matches!(
            config.rejection,
            Rejection::SigmaClipAsymmetric(..)
        ));
        assert_eq!(config.normalization, Normalization::Global);
    }

    #[test]
    #[should_panic(expected = "Cannot normalize empty weights")]
    fn test_normalized_weights_empty_panics() {
        let config = StackConfig::default();
        config.normalized_weights();
    }

    #[test]
    fn test_normalized_weights_provided() {
        let config = StackConfig::weighted(vec![1.0, 2.0, 3.0]);
        let weights = config.normalized_weights();
        assert_eq!(weights.len(), 3);
        assert!((weights.iter().sum::<f32>() - 1.0).abs() < f32::EPSILON);
        // First weight should be smallest
        assert!(weights[0] < weights[1]);
        assert!(weights[1] < weights[2]);
    }

    #[test]
    fn test_validate_valid_config() {
        let config = StackConfig::sigma_clipped(2.5);
        config.validate(); // Should not panic
    }

    #[test]
    #[should_panic(expected = "Sigma must be positive")]
    fn test_validate_invalid_sigma() {
        let config = StackConfig {
            rejection: Rejection::SigmaClip(SigmaClipConfig {
                sigma: -1.0,
                max_iterations: 3,
            }),
            ..Default::default()
        };
        config.validate();
    }

    #[test]
    #[should_panic(expected = "Low percentile must be 0-50")]
    fn test_validate_invalid_percentile() {
        let config = StackConfig {
            rejection: Rejection::Percentile(PercentileClipConfig {
                low_percentile: 60.0,
                high_percentile: 10.0,
            }),
            ..Default::default()
        };
        config.validate();
    }

    #[test]
    fn test_rejection_constructors() {
        let r = Rejection::sigma_clip(2.0);
        assert!(matches!(r, Rejection::SigmaClip(c) if (c.sigma - 2.0).abs() < f32::EPSILON));

        let r = Rejection::winsorized(3.0);
        assert!(matches!(r, Rejection::Winsorized(c) if (c.sigma - 3.0).abs() < f32::EPSILON));

        let r = Rejection::linear_fit(2.5);
        assert!(matches!(r, Rejection::LinearFit(c)
            if (c.sigma_low - 2.5).abs() < f32::EPSILON && (c.sigma_high - 2.5).abs() < f32::EPSILON));

        let r = Rejection::percentile(15.0);
        assert!(matches!(r, Rejection::Percentile(c)
            if (c.low_percentile - 15.0).abs() < f32::EPSILON && (c.high_percentile - 15.0).abs() < f32::EPSILON));

        let r = Rejection::gesd();
        assert!(matches!(r, Rejection::Gesd(c)
            if (c.alpha - 0.05).abs() < f32::EPSILON && c.max_outliers.is_none()));
    }
}
