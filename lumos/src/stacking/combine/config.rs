//! Unified stacking configuration.
//!
//! This module provides a single `StackConfig` type that encapsulates all stacking
//! parameters: combination method, pixel rejection, normalization, and memory settings.

use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::combine::error::StackConfigError;
use crate::stacking::combine::rejection::Rejection;

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

/// Default frames below which sigma-clip and linear-fit rejection are too unreliable to trust and
/// the combine falls back to the median. GESD has its own stricter floor; Winsorized/Percentile are
/// stable at smaller N.
const MIN_FRAMES_FOR_REJECTION: usize = 5;
const MIN_FRAMES_FOR_GESD: usize = 15;

/// Small-stack fallback policy for [`StackConfig`]. When a stack has fewer than `min_frames` frames
/// the configured [`StackConfig::method`]'s rejection statistics are unreliable, so the combine uses
/// `fallback` instead. This makes the fallback an explicit, inspectable part of the config rather
/// than a runtime transformation. `fallback` must be rejection-free (`Median` or `Mean(None)`) so it
/// never needs a fallback of its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmallN {
    /// Frames below which `fallback` replaces the configured method.
    pub min_frames: usize,
    /// The combine method used below `min_frames`.
    pub fallback: CombineMethod,
}

impl SmallN {
    /// No fallback — the method is reliable at any frame count (Winsorized, Percentile, Median,
    /// plain mean).
    pub fn none() -> Self {
        Self {
            min_frames: 0,
            fallback: CombineMethod::Median,
        }
    }

    /// Fall back to the median below `min_frames` frames.
    pub fn median_below(min_frames: usize) -> Self {
        Self {
            min_frames,
            fallback: CombineMethod::Median,
        }
    }

    /// The combine method to use for `frame_count` frames: `fallback` when there are too few for the
    /// configured `method`, else `method`. Warns on a real downgrade.
    pub(crate) fn resolve(&self, method: CombineMethod, frame_count: usize) -> CombineMethod {
        // A plain mean (no rejection) has nothing to fall back *from* — only a `Mean` with an actual
        // rejection method is downgraded. (An explicit `Median` is excluded by `!= self.fallback`.)
        // This keeps a method override inherited with a default `min_frames` from spuriously
        // turning a plain mean into a median at small N.
        let does_rejection = !matches!(method, CombineMethod::Mean(Rejection::None));
        if does_rejection && frame_count < self.min_frames && method != self.fallback {
            tracing::warn!(
                frame_count,
                min_frames = self.min_frames,
                "too few frames for the configured rejection; combining with the fallback instead",
            );
            return self.fallback;
        }
        method
    }
}

/// Frame weighting strategy.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Weighting {
    /// Equal weights for all frames (default).
    #[default]
    Equal,
    /// Automatic weighting by inverse background noise variance: w = 1/sigma^2.
    /// Frames with lower noise get higher weight. Uses per-frame MAD statistics
    /// that are already computed during normalization.
    Noise,
    /// Explicit per-frame weights provided by the user.
    Manual(Vec<f32>),
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
/// use lumos::{CombineMethod, Normalization, Rejection, StackConfig, stack};
///
/// // Simple sigma-clipped stacking (default)
/// let result = stack(&paths, StackConfig::default())?;
///
/// // Using presets
/// let result = stack(&paths, StackConfig::sigma_clipped(2.0))?;
/// let result = stack(&paths, StackConfig::median())?;
///
/// // Custom configuration
/// let config = StackConfig {
///     method: CombineMethod::Mean(Rejection::sigma_clip_asymmetric(2.0, 3.0)),
///     normalization: Normalization::Global,
///     ..Default::default()
/// };
/// let result = stack(&paths, config)?;
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct StackConfig {
    /// How to combine pixel values across frames.
    /// For `Mean`, includes the rejection algorithm.
    pub method: CombineMethod,
    /// Frame weighting strategy.
    pub weighting: Weighting,
    /// Frame normalization before stacking.
    pub normalization: Normalization,
    /// Combine method used when there are too few frames for `method`'s rejection (see [`SmallN`]).
    pub small_n: SmallN,
    /// Cache/memory behavior.
    pub cache: CacheConfig,
}

impl Default for StackConfig {
    fn default() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::default()),
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            // Default method is σ-clip, so the default fallback is the library σ-floor.
            small_n: SmallN::median_below(MIN_FRAMES_FOR_REJECTION),
            cache: CacheConfig::default(),
        }
    }
}

impl StackConfig {
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
            small_n: SmallN::none(),
            ..Default::default()
        }
    }

    /// Preset: simple mean without rejection (fastest, for bias frames).
    pub fn mean() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::None),
            small_n: SmallN::none(),
            ..Default::default()
        }
    }

    /// Preset: weighted mean with explicit weights.
    pub fn weighted(weights: Vec<f32>) -> Self {
        Self {
            weighting: Weighting::Manual(weights),
            ..Default::default()
        }
    }

    /// Preset: winsorized sigma clipping (better for small stacks).
    pub fn winsorized(sigma: f32) -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::winsorized(sigma)),
            // Winsorized is stable at small N — no median fallback.
            small_n: SmallN::none(),
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
            // Percentile clips a fixed fraction — stable at small N, no median fallback.
            small_n: SmallN::none(),
            ..Default::default()
        }
    }

    /// Preset: validation-constrained automatic GESD with a median fallback below its supported
    /// sample size.
    pub fn gesd() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::gesd()),
            small_n: SmallN::median_below(MIN_FRAMES_FOR_GESD),
            ..Default::default()
        }
    }

    /// Preset for bias frames: Winsorized σ=3.0, no normalization.
    pub fn bias() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::winsorized(3.0)),
            normalization: Normalization::None,
            small_n: SmallN::none(),
            ..Default::default()
        }
    }

    /// Preset for dark frames: Winsorized σ=3.0, no normalization.
    pub fn dark() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::winsorized(3.0)),
            normalization: Normalization::None,
            small_n: SmallN::none(),
            ..Default::default()
        }
    }

    /// Preset for flat frames: σ-clip σ=3.0, multiplicative normalization.
    pub fn flat() -> Self {
        // σ=3.0 matches the dark/bias preset and ccdproc's `combine` default (3σ low/high); flats are
        // smooth, so a permissive cut just trims clear outliers (dust shadows move between flats).
        Self {
            method: CombineMethod::Mean(Rejection::sigma_clip(3.0)),
            normalization: Normalization::Multiplicative,
            // Stricter quality floor than the library graphault: a master flat from < 8 frames uses the
            // median, since σ-clip statistics on so few smooth flats aren't worth the noise.
            small_n: SmallN::median_below(8),
            ..Default::default()
        }
    }

    /// Preset for light frames: σ-clip σ=2.5, global normalization, noise weighting.
    pub fn light() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::sigma_clip(2.5)),
            weighting: Weighting::Noise,
            normalization: Normalization::Global,
            ..Default::default()
        }
    }

    /// Validate configuration parameters.
    pub fn validate(&self) -> Result<(), StackConfigError> {
        if let CombineMethod::Mean(rejection) = &self.method {
            match rejection {
                Rejection::None => {}
                Rejection::SigmaClip(c) => {
                    if !c.sigma_low.is_finite() || c.sigma_low <= 0.0 {
                        return Err(StackConfigError::InvalidSigmaLow { value: c.sigma_low });
                    }
                    if !c.sigma_high.is_finite() || c.sigma_high <= 0.0 {
                        return Err(StackConfigError::InvalidSigmaHigh {
                            value: c.sigma_high,
                        });
                    }
                    if c.max_iterations == 0 {
                        return Err(StackConfigError::ZeroMaxIterations);
                    }
                }
                Rejection::Winsorized(c) => {
                    if !c.sigma_low.is_finite() || c.sigma_low <= 0.0 {
                        return Err(StackConfigError::InvalidSigmaLow { value: c.sigma_low });
                    }
                    if !c.sigma_high.is_finite() || c.sigma_high <= 0.0 {
                        return Err(StackConfigError::InvalidSigmaHigh {
                            value: c.sigma_high,
                        });
                    }
                }
                Rejection::LinearFit(c) => {
                    if !c.sigma_low.is_finite() || c.sigma_low <= 0.0 {
                        return Err(StackConfigError::InvalidSigmaLow { value: c.sigma_low });
                    }
                    if !c.sigma_high.is_finite() || c.sigma_high <= 0.0 {
                        return Err(StackConfigError::InvalidSigmaHigh {
                            value: c.sigma_high,
                        });
                    }
                    if c.max_iterations == 0 {
                        return Err(StackConfigError::ZeroMaxIterations);
                    }
                }
                Rejection::Percentile(c) => {
                    if !c.low_percentile.is_finite() || !(0.0..=50.0).contains(&c.low_percentile) {
                        return Err(StackConfigError::InvalidLowPercentile {
                            value: c.low_percentile,
                        });
                    }
                    if !c.high_percentile.is_finite() || !(0.0..=50.0).contains(&c.high_percentile)
                    {
                        return Err(StackConfigError::InvalidHighPercentile {
                            value: c.high_percentile,
                        });
                    }
                    let total = c.low_percentile + c.high_percentile;
                    if total >= 100.0 {
                        return Err(StackConfigError::InvalidTotalPercentile { total });
                    }
                }
                Rejection::Gesd(c) => {
                    if !c.alpha.is_finite() || !(0.0..1.0).contains(&c.alpha) {
                        return Err(StackConfigError::InvalidGesdAlpha { value: c.alpha });
                    }
                }
            }
        }

        if matches!(
            self.small_n.fallback,
            CombineMethod::Mean(rejection) if rejection != Rejection::None
        ) {
            return Err(StackConfigError::RejectingSmallNFallback);
        }

        if let Weighting::Manual(weights) = &self.weighting {
            if let Some((index, &value)) = weights
                .iter()
                .enumerate()
                .find(|(_, value)| !value.is_finite() || **value < 0.0)
            {
                return Err(StackConfigError::InvalidManualWeight { index, value });
            }
            let sum: f32 = weights.iter().sum();
            if !sum.is_finite() || sum <= 0.0 {
                return Err(StackConfigError::InvalidManualWeightSum);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::stacking::combine::config::*;
    use crate::stacking::combine::rejection::{
        GesdConfig, LinearFitClipConfig, PercentileClipConfig, SigmaClipConfig,
        WinsorizedClipConfig,
    };

    #[test]
    fn small_n_resolve_downgrades_below_min_frames() {
        let sigma = CombineMethod::Mean(Rejection::sigma_clip(2.5));
        let floor5 = SmallN::median_below(5);
        // Below the floor → fallback (median); at/above → the configured method.
        assert_eq!(floor5.resolve(sigma, 4), CombineMethod::Median);
        assert_eq!(floor5.resolve(sigma, 5), sigma);
        assert_eq!(floor5.resolve(sigma, 50), sigma);
        // `none()` never downgrades, even at N=2.
        let win = CombineMethod::Mean(Rejection::winsorized(3.0));
        assert_eq!(SmallN::none().resolve(win, 2), win);
        // A method that already equals the fallback is returned unchanged (no spurious downgrade).
        assert_eq!(
            floor5.resolve(CombineMethod::Median, 1),
            CombineMethod::Median
        );
        // The flat preset's stricter floor of 8 is honoured.
        assert_eq!(
            StackConfig::flat().small_n.resolve(sigma, 7),
            CombineMethod::Median
        );
        assert_eq!(StackConfig::flat().small_n.resolve(sigma, 8), sigma);
    }

    #[test]
    fn test_default_config() {
        let config = StackConfig::default();
        assert!(matches!(
            config.method,
            CombineMethod::Mean(Rejection::SigmaClip(..))
        ));
        assert_eq!(config.weighting, Weighting::Equal);
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
        assert!(matches!(config.weighting, Weighting::Manual(ref w) if w.len() == 3));
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
    fn test_validate_valid_config() {
        let config = StackConfig::sigma_clipped(2.5);
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn test_validate_invalid_config_returns_exact_errors() {
        let cases = [
            (
                StackConfig::sigma_clipped(-1.0),
                StackConfigError::InvalidSigmaLow { value: -1.0 },
            ),
            (
                StackConfig {
                    method: CombineMethod::Mean(Rejection::sigma_clip_asymmetric(
                        2.0,
                        f32::INFINITY,
                    )),
                    ..Default::default()
                },
                StackConfigError::InvalidSigmaHigh {
                    value: f32::INFINITY,
                },
            ),
            (
                StackConfig {
                    method: CombineMethod::Mean(Rejection::SigmaClip(SigmaClipConfig::new(2.0, 0))),
                    ..Default::default()
                },
                StackConfigError::ZeroMaxIterations,
            ),
            (
                StackConfig {
                    method: CombineMethod::Mean(Rejection::Winsorized(WinsorizedClipConfig::new(
                        0.0,
                    ))),
                    ..Default::default()
                },
                StackConfigError::InvalidSigmaLow { value: 0.0 },
            ),
            (
                StackConfig {
                    method: CombineMethod::Mean(Rejection::LinearFit(LinearFitClipConfig::new(
                        2.0, 0.0, 3,
                    ))),
                    ..Default::default()
                },
                StackConfigError::InvalidSigmaHigh { value: 0.0 },
            ),
            (
                StackConfig::percentile(60.0),
                StackConfigError::InvalidLowPercentile { value: 60.0 },
            ),
            (
                StackConfig {
                    method: CombineMethod::Mean(Rejection::Percentile(PercentileClipConfig::new(
                        10.0, 60.0,
                    ))),
                    ..Default::default()
                },
                StackConfigError::InvalidHighPercentile { value: 60.0 },
            ),
            (
                StackConfig {
                    method: CombineMethod::Mean(Rejection::Percentile(PercentileClipConfig::new(
                        50.0, 50.0,
                    ))),
                    ..Default::default()
                },
                StackConfigError::InvalidTotalPercentile { total: 100.0 },
            ),
            (
                StackConfig {
                    method: CombineMethod::Mean(Rejection::Gesd(GesdConfig::new(1.0, None))),
                    ..Default::default()
                },
                StackConfigError::InvalidGesdAlpha { value: 1.0 },
            ),
            (
                StackConfig::weighted(vec![1.0, -0.5]),
                StackConfigError::InvalidManualWeight {
                    index: 1,
                    value: -0.5,
                },
            ),
            (
                StackConfig::weighted(vec![0.0, 0.0]),
                StackConfigError::InvalidManualWeightSum,
            ),
            (
                StackConfig {
                    small_n: SmallN {
                        min_frames: 5,
                        fallback: CombineMethod::Mean(Rejection::sigma_clip(2.0)),
                    },
                    ..Default::default()
                },
                StackConfigError::RejectingSmallNFallback,
            ),
        ];

        for (config, expected) in cases {
            assert_eq!(config.validate(), Err(expected));
        }
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
                if (c.sigma_low - 3.0).abs() < f32::EPSILON
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
        assert_eq!(config.weighting, Weighting::Noise);
        assert_eq!(config.normalization, Normalization::Global);
    }

    #[test]
    fn test_gesd_preset_uses_supported_sample_floor() {
        let config = StackConfig::gesd();
        assert!(matches!(
            config.method,
            CombineMethod::Mean(Rejection::Gesd(..))
        ));
        assert_eq!(config.small_n, SmallN::median_below(15));
    }
}
