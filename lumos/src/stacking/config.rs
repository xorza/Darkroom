//! Unified stacking configuration.
//!
//! This module provides a single `StackConfig` type that encapsulates all stacking
//! parameters: combination method, pixel rejection, normalization, and memory settings.

// Allow dead_code until external callers adopt the new API

use crate::stacking::CacheConfig;

/// Method for combining pixel values across frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CombineMethod {
    /// Simple average (no implicit rejection, fastest).
    #[default]
    Mean,
    /// Median value (implicit outlier rejection).
    Median,
    /// Weighted mean using per-frame weights.
    WeightedMean,
}

/// Pixel rejection algorithm applied before combining.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rejection {
    /// No rejection.
    None,
    /// Iterative sigma clipping from median.
    SigmaClip {
        /// Number of standard deviations for clipping threshold.
        sigma: f32,
        /// Maximum number of iterations.
        iterations: u32,
    },
    /// Sigma clipping with asymmetric thresholds.
    SigmaClipAsymmetric {
        /// Sigma threshold for low outliers.
        sigma_low: f32,
        /// Sigma threshold for high outliers.
        sigma_high: f32,
        /// Maximum number of iterations.
        iterations: u32,
    },
    /// Replace outliers with boundary values (better for small stacks).
    Winsorized {
        /// Number of standard deviations for clipping threshold.
        sigma: f32,
        /// Maximum number of iterations.
        iterations: u32,
    },
    /// Fit linear trend, reject deviants (good for gradients).
    LinearFit {
        /// Sigma threshold for low outliers.
        sigma_low: f32,
        /// Sigma threshold for high outliers.
        sigma_high: f32,
        /// Maximum number of iterations.
        iterations: u32,
    },
    /// Clip lowest/highest percentiles.
    Percentile {
        /// Percentile to clip from low end (0-50).
        low: f32,
        /// Percentile to clip from high end (0-50).
        high: f32,
    },
    /// Generalized ESD test (best for large stacks >50 frames).
    Gesd {
        /// Significance level (typically 0.05).
        alpha: f32,
        /// Maximum outliers to detect. None = 25% of data.
        max_outliers: Option<usize>,
    },
}

impl Default for Rejection {
    fn default() -> Self {
        Self::SigmaClip {
            sigma: 2.5,
            iterations: 3,
        }
    }
}

impl Rejection {
    /// Create sigma clipping with default iterations.
    pub fn sigma_clip(sigma: f32) -> Self {
        Self::SigmaClip {
            sigma,
            iterations: 3,
        }
    }

    /// Create asymmetric sigma clipping.
    pub fn sigma_clip_asymmetric(sigma_low: f32, sigma_high: f32) -> Self {
        Self::SigmaClipAsymmetric {
            sigma_low,
            sigma_high,
            iterations: 3,
        }
    }

    /// Create winsorized sigma clipping.
    pub fn winsorized(sigma: f32) -> Self {
        Self::Winsorized {
            sigma,
            iterations: 3,
        }
    }

    /// Create linear fit clipping with symmetric thresholds.
    pub fn linear_fit(sigma: f32) -> Self {
        Self::LinearFit {
            sigma_low: sigma,
            sigma_high: sigma,
            iterations: 3,
        }
    }

    /// Create percentile clipping with symmetric bounds.
    pub fn percentile(percent: f32) -> Self {
        Self::Percentile {
            low: percent,
            high: percent,
        }
    }

    /// Create GESD with default alpha.
    pub fn gesd() -> Self {
        Self::Gesd {
            alpha: 0.05,
            max_outliers: None,
        }
    }
}

/// Frame normalization method applied before stacking.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Normalization {
    /// No normalization.
    #[default]
    None,
    /// Match global median and scale.
    Global,
    /// Tile-based local normalization (PixInsight-style).
    Local {
        /// Tile size in pixels (default: 128).
        tile_size: usize,
    },
}

impl Normalization {
    /// Create local normalization with default tile size.
    pub fn local() -> Self {
        Self::Local { tile_size: 128 }
    }

    /// Create local normalization with fine tiles (64px).
    pub fn local_fine() -> Self {
        Self::Local { tile_size: 64 }
    }

    /// Create local normalization with coarse tiles (256px).
    pub fn local_coarse() -> Self {
        Self::Local { tile_size: 256 }
    }
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
/// // Custom configuration using struct update syntax
/// let config = StackConfig {
///     rejection: Rejection::SigmaClipAsymmetric {
///         sigma_low: 2.0,
///         sigma_high: 3.0,
///         iterations: 5,
///     },
///     normalization: Normalization::Local { tile_size: 128 },
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
            rejection: Rejection::SigmaClip {
                sigma,
                iterations: 3,
            },
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
            method: CombineMethod::WeightedMean,
            weights,
            ..Default::default()
        }
    }

    /// Preset: winsorized sigma clipping (better for small stacks).
    pub fn winsorized(sigma: f32) -> Self {
        Self {
            rejection: Rejection::Winsorized {
                sigma,
                iterations: 3,
            },
            ..Default::default()
        }
    }

    /// Preset: linear fit clipping (good for sky gradients).
    pub fn linear_fit(sigma: f32) -> Self {
        Self {
            rejection: Rejection::LinearFit {
                sigma_low: sigma,
                sigma_high: sigma,
                iterations: 3,
            },
            ..Default::default()
        }
    }

    /// Preset: percentile clipping (simple, for small stacks <10).
    pub fn percentile(percent: f32) -> Self {
        Self {
            rejection: Rejection::Percentile {
                low: percent,
                high: percent,
            },
            ..Default::default()
        }
    }

    /// Preset: GESD (rigorous, for large stacks >50).
    pub fn gesd() -> Self {
        Self {
            rejection: Rejection::Gesd {
                alpha: 0.05,
                max_outliers: None,
            },
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
        match self.rejection {
            Rejection::None => {}
            Rejection::SigmaClip { sigma, iterations } => {
                assert!(sigma > 0.0, "Sigma must be positive");
                assert!(iterations > 0, "Iterations must be at least 1");
            }
            Rejection::SigmaClipAsymmetric {
                sigma_low,
                sigma_high,
                iterations,
            } => {
                assert!(sigma_low > 0.0, "Sigma low must be positive");
                assert!(sigma_high > 0.0, "Sigma high must be positive");
                assert!(iterations > 0, "Iterations must be at least 1");
            }
            Rejection::Winsorized { sigma, iterations } => {
                assert!(sigma > 0.0, "Sigma must be positive");
                assert!(iterations > 0, "Iterations must be at least 1");
            }
            Rejection::LinearFit {
                sigma_low,
                sigma_high,
                iterations,
            } => {
                assert!(sigma_low > 0.0, "Sigma low must be positive");
                assert!(sigma_high > 0.0, "Sigma high must be positive");
                assert!(iterations > 0, "Iterations must be at least 1");
            }
            Rejection::Percentile { low, high } => {
                assert!((0.0..=50.0).contains(&low), "Low percentile must be 0-50");
                assert!((0.0..=50.0).contains(&high), "High percentile must be 0-50");
                assert!(low + high < 100.0, "Total clipping must be < 100%");
            }
            Rejection::Gesd { alpha, .. } => {
                assert!((0.0..1.0).contains(&alpha), "Alpha must be 0-1");
            }
        }

        if let Normalization::Local { tile_size } = self.normalization {
            assert!(tile_size > 0, "Tile size must be positive");
        }

        if self.method == CombineMethod::WeightedMean && !self.weights.is_empty() {
            assert!(
                self.weights.iter().all(|&w| w >= 0.0),
                "Weights must be non-negative"
            );
        }
    }

    /// Get normalized weights (sum to 1.0).
    pub fn normalized_weights(&self, frame_count: usize) -> Vec<f32> {
        if self.weights.is_empty() {
            vec![1.0 / frame_count as f32; frame_count]
        } else {
            let sum: f32 = self.weights.iter().sum();
            if sum > f32::EPSILON {
                self.weights.iter().map(|w| w / sum).collect()
            } else {
                vec![1.0 / frame_count as f32; frame_count]
            }
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
        assert!(matches!(config.rejection, Rejection::SigmaClip { .. }));
        assert!(config.weights.is_empty());
        assert_eq!(config.normalization, Normalization::None);
    }

    #[test]
    fn test_sigma_clipped_preset() {
        let config = StackConfig::sigma_clipped(2.0);
        assert_eq!(config.method, CombineMethod::Mean);
        assert!(matches!(
            config.rejection,
            Rejection::SigmaClip { sigma, .. } if (sigma - 2.0).abs() < f32::EPSILON
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
        assert_eq!(config.method, CombineMethod::WeightedMean);
        assert_eq!(config.weights.len(), 3);
    }

    #[test]
    fn test_struct_update_syntax() {
        let config = StackConfig {
            rejection: Rejection::SigmaClipAsymmetric {
                sigma_low: 2.0,
                sigma_high: 3.0,
                iterations: 5,
            },
            normalization: Normalization::Local { tile_size: 64 },
            ..Default::default()
        };
        assert!(matches!(
            config.rejection,
            Rejection::SigmaClipAsymmetric { .. }
        ));
        assert!(matches!(config.normalization, Normalization::Local { .. }));
    }

    #[test]
    fn test_normalized_weights_empty() {
        let config = StackConfig::default();
        let weights = config.normalized_weights(4);
        assert_eq!(weights.len(), 4);
        assert!((weights.iter().sum::<f32>() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_normalized_weights_provided() {
        let config = StackConfig::weighted(vec![1.0, 2.0, 3.0]);
        let weights = config.normalized_weights(3);
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
            rejection: Rejection::SigmaClip {
                sigma: -1.0,
                iterations: 3,
            },
            ..Default::default()
        };
        config.validate();
    }

    #[test]
    #[should_panic(expected = "Low percentile must be 0-50")]
    fn test_validate_invalid_percentile() {
        let config = StackConfig {
            rejection: Rejection::Percentile {
                low: 60.0,
                high: 10.0,
            },
            ..Default::default()
        };
        config.validate();
    }

    #[test]
    fn test_rejection_constructors() {
        let r = Rejection::sigma_clip(2.0);
        assert!(
            matches!(r, Rejection::SigmaClip { sigma, .. } if (sigma - 2.0).abs() < f32::EPSILON)
        );

        let r = Rejection::winsorized(3.0);
        assert!(
            matches!(r, Rejection::Winsorized { sigma, .. } if (sigma - 3.0).abs() < f32::EPSILON)
        );

        let r = Rejection::linear_fit(2.5);
        assert!(
            matches!(r, Rejection::LinearFit { sigma_low, sigma_high, .. }
            if (sigma_low - 2.5).abs() < f32::EPSILON && (sigma_high - 2.5).abs() < f32::EPSILON)
        );

        let r = Rejection::percentile(15.0);
        assert!(matches!(r, Rejection::Percentile { low, high }
            if (low - 15.0).abs() < f32::EPSILON && (high - 15.0).abs() < f32::EPSILON));

        let r = Rejection::gesd();
        assert!(matches!(r, Rejection::Gesd { alpha, max_outliers: None }
            if (alpha - 0.05).abs() < f32::EPSILON));
    }

    #[test]
    fn test_normalization_constructors() {
        assert!(matches!(
            Normalization::local(),
            Normalization::Local { tile_size: 128 }
        ));
        assert!(matches!(
            Normalization::local_fine(),
            Normalization::Local { tile_size: 64 }
        ));
        assert!(matches!(
            Normalization::local_coarse(),
            Normalization::Local { tile_size: 256 }
        ));
    }
}
