//! Unified stacking configuration.
//!
//! This module provides a single `StackConfig` type that encapsulates all stacking
//! parameters: combination method, pixel rejection, normalization, and memory settings.

use crate::combine::error::StackConfigError;
use crate::combine::rejection::Rejection;
use crate::error::InvalidConfigField;
use crate::ingest::ingest_config::IngestConfig;
use crate::stack_product::quality_planes::QualityPlanes;

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
/// the combine falls back to the median. GESD has its own stricter floor; Winsorized and Trim are
/// stable at smaller N.
const MIN_FRAMES_FOR_REJECTION: usize = 5;
const MIN_FRAMES_FOR_GESD: usize = 15;

/// Small-stack fallback policy for [`StackConfig`]. When a stack has fewer than `min_frames` frames
/// the configured [`StackConfig::method`]'s rejection statistics are unreliable, so the combine
/// uses `fallback` instead. This makes the fallback an explicit, inspectable part of the config
/// rather than a runtime transformation. `fallback` must be rejection-free (`Median` or
/// `Mean(None)`) so it never needs a fallback of its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SmallN {
    /// Frames below which `fallback` replaces the configured method.
    pub min_frames: usize,
    /// The combine method used below `min_frames`.
    pub fallback: CombineMethod,
}

impl SmallN {
    /// No fallback — the method is reliable at any frame count (Winsorized, Trim, Median, plain
    /// mean).
    pub const fn none() -> Self {
        Self {
            min_frames: 0,
            fallback: CombineMethod::Median,
        }
    }

    /// Fall back to the median below `min_frames` frames.
    pub const fn median_below(min_frames: usize) -> Self {
        Self {
            min_frames,
            fallback: CombineMethod::Median,
        }
    }

    /// The combine method to use for `frame_count` frames: `fallback` when there are too few for
    /// the configured `method`, else `method`. Warns on a real downgrade.
    pub(crate) fn resolve(&self, method: CombineMethod, frame_count: usize) -> CombineMethod {
        // A plain mean (no rejection) has nothing to fall back *from* — only a `Mean` with an
        // actual rejection method is downgraded. (An explicit `Median` is excluded by `!=
        // self.fallback`.) This keeps a method override inherited with a default `min_frames` from
        // spuriously turning a plain mean into a median at small N.
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
    /// Each frame's inverse noise variance per channel, `w = 1/(gain·σ)²`: σ is the white noise
    /// measured on the frame (by the multiresolution estimator, or per colour on a mosaic) and
    /// `gain` its normalization. Not normalized, so the weight plane is an inverse variance. A
    /// frame with no measured noise is an error.
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
/// ```no_run
/// use common::CancelToken;
/// use lumos::{CombineMethod, Normalization, ProgressCallback, Rejection, StackConfig, stack};
///
/// let paths = ["frame1.fits", "frame2.fits", "frame3.fits"];
///
/// // Simple sigma-clipped stacking (default)
/// let result = stack(
///     &paths,
///     &StackConfig::default(),
///     ProgressCallback::default(),
///     CancelToken::never(),
/// )?;
///
/// // Using presets
/// let median = StackConfig::median();
///
/// // Custom configuration
/// let config = StackConfig {
///     method: CombineMethod::Mean(Rejection::sigma_clip_asymmetric(2.0, 3.0)),
///     normalization: Normalization::Global,
///     ..Default::default()
/// };
/// let result = stack(
///     &paths,
///     &config,
///     ProgressCallback::default(),
///     CancelToken::never(),
/// )?;
/// # Ok::<(), lumos::StackError>(())
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
    pub ingest: IngestConfig,
    /// Which ancillary per-pixel planes the combine should produce. Defaults to coverage, weight
    /// and variance — they are what makes the stacked master measurable — but each is a full
    /// image-sized allocation, so a caller that discards them should say so.
    pub quality: QualityPlanes,
    /// The fewest samples a pixel keeps when the combine leaves samples out: flagged ones today
    /// (saturated, repaired, cosmic ray, defect, flat floor). A flagged sample is left out only
    /// while this many unflagged samples remain at its pixel; otherwise every sample stays.
    /// PixInsight keeps 3, Siril 4. At least 1.
    pub min_survivors: usize,
}

/// [`StackConfig::min_survivors`] by default, as PixInsight's `ImageIntegration`.
pub(crate) const DEFAULT_MIN_SURVIVORS: usize = 3;

impl Default for StackConfig {
    fn default() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::default()),
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            // Default method is σ-clip, so the default fallback is the library σ-floor.
            small_n: SmallN::median_below(MIN_FRAMES_FOR_REJECTION),
            ingest: IngestConfig::default(),
            quality: QualityPlanes::STANDARD,
            min_survivors: DEFAULT_MIN_SURVIVORS,
        }
    }
}

impl StackConfig {
    /// This configuration for the frames left once the input frames at `dropped` (ascending)
    /// are gone: manual weights, given one per input frame, follow their frames; every other
    /// setting applies unchanged.
    pub(crate) fn for_survivors(&self, dropped: &[usize]) -> Self {
        let mut config = self.clone();
        if let Weighting::Manual(weights) = &mut config.weighting {
            let mut index = 0;
            weights.retain(|_| {
                let kept = dropped.binary_search(&index).is_err();
                index += 1;
                kept
            });
        }
        config
    }

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

    /// Preset: a trimmed mean, dropping `percent` of the samples from each end.
    pub fn trim(percent: f32) -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::trim(percent)),
            // A trim measures no spread, so a small stack needs no median fallback.
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

    /// Preset for bias and dark frames: Winsorized σ=3.0, no normalization. One preset, because
    /// both measure a level that every frame shares, with nothing between frames to normalize.
    pub fn bias_or_dark() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::winsorized(3.0)),
            normalization: Normalization::None,
            small_n: SmallN::none(),
            ..Default::default()
        }
    }

    /// Preset for flat frames: σ-clip σ=3.0, multiplicative normalization.
    pub fn flat() -> Self {
        // σ=3.0 matches the bias-or-dark preset and ccdproc's `combine` default (3σ low/high); flats
        // are smooth, so a permissive cut just trims clear outliers (dust shadows move between
        // flats).
        Self {
            method: CombineMethod::Mean(Rejection::sigma_clip(3.0)),
            normalization: Normalization::Multiplicative,
            // Stricter than the default floor: a master flat from fewer than 8 frames uses the
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
        InvalidConfigField::check(
            self.min_survivors >= 1,
            "min_survivors",
            "at least 1",
            self.min_survivors as f64,
        )?;
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
