//! Unified stacking configuration.
//!
//! This module provides a single `StackConfig` type that encapsulates all stacking
//! parameters: combination method, pixel rejection, normalization, and memory settings.

use crate::stacking::CacheConfig;
use crate::stacking::rejection::Rejection;

/// Method for combining pixel values across frames.
///
/// Rejection is only available with `Mean` — median is already robust to outliers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CombineMethod {
    /// Mean value with optional rejection. If weights are provided, computes weighted mean.
    Mean(Rejection),
    /// Median value (implicit outlier rejection, no explicit rejection needed).
    Median,
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
///     method: CombineMethod::Mean(Rejection::sigma_clip_asymmetric(2.0, 3.0)),
///     normalization: Normalization::Global,
///     ..Default::default()
/// };
/// let result = stack(&paths, FrameType::Light, config)?;
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct StackConfig {
    /// How to combine pixel values across frames.
    /// For `Mean`, includes the rejection algorithm.
    pub method: CombineMethod,
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
            method: CombineMethod::Mean(Rejection::default()),
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
            method: CombineMethod::Mean(Rejection::sigma_clip(sigma)),
            ..Default::default()
        }
    }

    /// Preset: median stacking (implicit outlier rejection).
    pub fn median() -> Self {
        Self {
            method: CombineMethod::Median,
            ..Default::default()
        }
    }

    /// Preset: simple mean without rejection (fastest, for bias frames).
    pub fn mean() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::None),
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
            method: CombineMethod::Mean(Rejection::winsorized(sigma)),
            ..Default::default()
        }
    }

    /// Preset: linear fit clipping (good for sky gradients).
    pub fn linear_fit(sigma: f32) -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::linear_fit(sigma)),
            ..Default::default()
        }
    }

    /// Preset: percentile clipping (simple, for small stacks <10).
    pub fn percentile(percent: f32) -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::percentile(percent)),
            ..Default::default()
        }
    }

    /// Preset: GESD (rigorous, for large stacks >50).
    pub fn gesd() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::gesd()),
            ..Default::default()
        }
    }

    // ========== Frame-Type Presets ==========

    /// Preset for bias frames: Winsorized σ=3.0, no normalization.
    pub fn bias() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::winsorized(3.0)),
            normalization: Normalization::None,
            ..Default::default()
        }
    }

    /// Preset for dark frames: Winsorized σ=3.0, no normalization.
    pub fn dark() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::winsorized(3.0)),
            normalization: Normalization::None,
            ..Default::default()
        }
    }

    /// Preset for flat frames: σ-clip σ=2.5, multiplicative normalization.
    pub fn flat() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::sigma_clip(2.5)),
            normalization: Normalization::Multiplicative,
            ..Default::default()
        }
    }

    /// Preset for light frames: σ-clip σ=2.5, global normalization.
    pub fn light() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::sigma_clip(2.5)),
            normalization: Normalization::Global,
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
        if let CombineMethod::Mean(rejection) = &self.method {
            match rejection {
                Rejection::None => {}
                Rejection::SigmaClip(c) => {
                    assert!(c.sigma_low > 0.0, "Sigma low must be positive");
                    assert!(c.sigma_high > 0.0, "Sigma high must be positive");
                    assert!(c.max_iterations > 0, "Iterations must be at least 1");
                }
                Rejection::Winsorized(c) => {
                    assert!(c.sigma_low > 0.0, "Sigma low must be positive");
                    assert!(c.sigma_high > 0.0, "Sigma high must be positive");
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
    use crate::stacking::rejection::{PercentileClipConfig, SigmaClipConfig};

    #[test]
    fn test_default_config() {
        let config = StackConfig::default();
        assert!(matches!(
            config.method,
            CombineMethod::Mean(Rejection::SigmaClip(..))
        ));
        assert!(config.weights.is_empty());
        assert_eq!(config.normalization, Normalization::None);
    }

    #[test]
    fn test_sigma_clipped_preset() {
        let config = StackConfig::sigma_clipped(2.0);
        assert!(matches!(
            config.method,
            CombineMethod::Mean(Rejection::SigmaClip(c))
                if (c.sigma_low - 2.0).abs() < f32::EPSILON && (c.sigma_high - 2.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn test_median_preset() {
        let config = StackConfig::median();
        assert_eq!(config.method, CombineMethod::Median);
    }

    #[test]
    fn test_weighted_preset() {
        let config = StackConfig::weighted(vec![1.0, 2.0, 3.0]);
        assert!(matches!(config.method, CombineMethod::Mean(..)));
        assert_eq!(config.weights.len(), 3);
    }

    #[test]
    fn test_struct_update_syntax() {
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::SigmaClip(SigmaClipConfig::new_asymmetric(
                2.0, 3.0, 5,
            ))),
            normalization: Normalization::Global,
            ..Default::default()
        };
        assert!(matches!(
            config.method,
            CombineMethod::Mean(Rejection::SigmaClip(..))
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
    #[should_panic(expected = "Sigma low must be positive")]
    fn test_validate_invalid_sigma() {
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::SigmaClip(SigmaClipConfig {
                sigma_low: -1.0,
                sigma_high: -1.0,
                max_iterations: 3,
            })),
            ..Default::default()
        };
        config.validate();
    }

    #[test]
    fn test_bias_preset() {
        let config = StackConfig::bias();
        assert!(matches!(
            config.method,
            CombineMethod::Mean(Rejection::Winsorized(c))
                if (c.sigma_low - 3.0).abs() < f32::EPSILON
        ));
        assert_eq!(config.normalization, Normalization::None);
    }

    #[test]
    fn test_dark_preset() {
        let config = StackConfig::dark();
        assert!(matches!(
            config.method,
            CombineMethod::Mean(Rejection::Winsorized(c))
                if (c.sigma_low - 3.0).abs() < f32::EPSILON
        ));
        assert_eq!(config.normalization, Normalization::None);
    }

    #[test]
    fn test_flat_preset() {
        let config = StackConfig::flat();
        assert!(matches!(
            config.method,
            CombineMethod::Mean(Rejection::SigmaClip(c))
                if (c.sigma_low - 2.5).abs() < f32::EPSILON
        ));
        assert_eq!(config.normalization, Normalization::Multiplicative);
    }

    #[test]
    fn test_light_preset() {
        let config = StackConfig::light();
        assert!(matches!(
            config.method,
            CombineMethod::Mean(Rejection::SigmaClip(c))
                if (c.sigma_low - 2.5).abs() < f32::EPSILON
        ));
        assert_eq!(config.normalization, Normalization::Global);
    }

    #[test]
    #[should_panic(expected = "Low percentile must be 0-50")]
    fn test_validate_invalid_percentile() {
        let config = StackConfig {
            method: CombineMethod::Mean(Rejection::Percentile(PercentileClipConfig {
                low_percentile: 60.0,
                high_percentile: 10.0,
            })),
            ..Default::default()
        };
        config.validate();
    }
}
