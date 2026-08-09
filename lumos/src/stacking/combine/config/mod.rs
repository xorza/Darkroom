//! Unified stacking configuration.
//!
//! This module provides a single `StackConfig` type that encapsulates all stacking
//! parameters: combination method, pixel rejection, normalization, and memory settings.

use crate::stacking::combine::cache_config::CacheConfig;
use crate::stacking::combine::error::StackConfigError;
use crate::stacking::combine::rejection::Rejection;
use crate::stacking::stack_product::quality_planes::QualityPlanes;

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
    /// Which ancillary per-pixel planes the combine should produce. Defaults to all of them —
    /// they are what makes the stacked master measurable — but each is a full image-sized
    /// allocation, so a caller that discards them should say so.
    pub quality: QualityPlanes,
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
            quality: QualityPlanes::ALL,
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
            rejection.validate()?;
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
mod tests;
