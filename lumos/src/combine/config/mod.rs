//! Stacking configuration: how the samples at a pixel combine ([`Combine`]), and the policy a
//! frame set's role asks for around it ([`StackConfig`]).

use crate::combine::error::StackConfigError;
use crate::combine::rejection::Rejection;
use crate::error::InvalidConfigField;
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
/// the combine falls back to the median. GESD has its own stricter floor. Winsorized clips once,
/// about an estimate of every sample, and trim measures no spread, so neither has one.
const MIN_FRAMES_FOR_REJECTION: usize = 5;
const MIN_FRAMES_FOR_GESD: usize = 15;

/// Small-stack fallback policy for [`Combine`]. When a stack has fewer than `min_frames` frames
/// the configured [`Combine::method`]'s rejection statistics are unreliable, so the combine
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
#[derive(Debug, Clone, PartialEq)]
pub enum Weighting {
    /// Equal weights for all frames.
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Normalization {
    /// No normalization.
    None,
    /// Match global median and scale across frames (additive + scaling).
    /// Best for light frames.
    Global,
    /// Scale by ratio of medians (no additive offset).
    /// Best for flat frames where exposure varies.
    Multiplicative,
}

/// How the samples at one pixel become one value: the method, with its rejection, and the method
/// it falls back to below the frame count that rejection needs.
///
/// The constructors are the method presets. They set the method alone; the normalization and the
/// weighting belong to the role of the frames, which [`StackConfig`]'s presets carry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Combine {
    /// How to combine pixel values across frames. For `Mean`, includes the rejection algorithm.
    pub method: CombineMethod,
    /// Combine method used when there are too few frames for `method`'s rejection (see [`SmallN`]).
    pub small_n: SmallN,
}

impl Combine {
    /// Sigma-clipped mean, with the median below the frames sigma statistics need.
    pub const fn sigma_clipped(sigma: f32) -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::sigma_clip(sigma)),
            small_n: SmallN::median_below(MIN_FRAMES_FOR_REJECTION),
        }
    }

    /// Median (implicit outlier rejection).
    pub const fn median() -> Self {
        Self {
            method: CombineMethod::Median,
            small_n: SmallN::none(),
        }
    }

    /// Plain mean without rejection.
    pub const fn mean() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::None),
            small_n: SmallN::none(),
        }
    }

    /// Winsorized sigma clipping, with no median fallback: it clips once, about a Huber estimate of
    /// every sample. On ten clean samples at k = 3 it rejects 0.12% under the frames' noise floor and
    /// 1.5% with none, against the Gaussian tail share of 0.27%.
    pub const fn winsorized(sigma: f32) -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::winsorized(sigma)),
            small_n: SmallN::none(),
        }
    }

    /// Linear fit clipping (good for sky gradients).
    pub const fn linear_fit(sigma: f32) -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::linear_fit(sigma)),
            small_n: SmallN::median_below(MIN_FRAMES_FOR_REJECTION),
        }
    }

    /// A trimmed mean, dropping `percent` of the samples from each end. A trim measures no spread,
    /// so a small stack needs no median fallback.
    pub const fn trim(percent: f32) -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::trim(percent)),
            small_n: SmallN::none(),
        }
    }

    /// Validation-constrained automatic GESD with a median fallback below its supported sample
    /// size.
    pub fn gesd() -> Self {
        Self {
            method: CombineMethod::Mean(Rejection::gesd()),
            small_n: SmallN::median_below(MIN_FRAMES_FOR_GESD),
        }
    }
}

/// Configuration for stacking a set of frames: how each pixel combines, and the normalization,
/// weighting and output planes the set's role asks for.
///
/// There is no default: the policy depends on what the frames are, so every configuration starts
/// from a role preset — [`Self::light`], [`Self::flat`] or [`Self::bias_or_dark`] — and changes
/// what it needs.
///
/// # Examples
///
/// ```no_run
/// use common::CancelToken;
/// use lumos::{Combine, IngestConfig, ProgressCallback, Rejection, StackConfig, stack};
///
/// let paths = ["frame1.fits", "frame2.fits", "frame3.fits"];
/// let ingest = IngestConfig::default();
///
/// // Light frames, as the preset combines them.
/// let result = stack(
///     &paths,
///     &StackConfig::light(),
///     &ingest,
///     ProgressCallback::default(),
///     CancelToken::never(),
/// )?;
///
/// // Light frames by another method, normalized and weighted as lights still.
/// let config = StackConfig {
///     combine: Combine::winsorized(3.0),
///     ..StackConfig::light()
/// };
/// let result = stack(&paths, &config, &ingest, ProgressCallback::default(), CancelToken::never())?;
/// # Ok::<(), lumos::StackError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct StackConfig {
    /// How the samples at a pixel combine.
    pub combine: Combine,
    /// Frame weighting strategy.
    pub weighting: Weighting,
    /// Frame normalization before stacking.
    pub normalization: Normalization,
    /// Which ancillary per-pixel planes the combine should produce. Defaults to coverage, weight
    /// and variance — they are what makes the stacked master measurable — but each is a full
    /// image-sized allocation, so a caller that discards them should say so.
    pub quality: QualityPlanes,
    /// The fewest samples a pixel keeps when the combine leaves samples out. A flagged sample
    /// (saturated, repaired, cosmic ray, defect, flat floor) is left out only while this many
    /// unflagged samples remain at its pixel; otherwise every sample stays. Rejection never leaves
    /// fewer: a pixel with no more samples than this is not rejected from, and a pass that would
    /// keep fewer keeps this many nearest its centre instead. PixInsight keeps 3, Siril 4. At
    /// least 1.
    pub min_survivors: usize,
}

/// [`StackConfig::min_survivors`] by default, as PixInsight's `ImageIntegration`.
pub(crate) const DEFAULT_MIN_SURVIVORS: usize = 3;

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

    /// Preset for light frames: σ-clip σ=2.5, global normalization, noise weighting. Frames of one
    /// field differ in sky level and transparency, so lights are put on one scale and weighted by
    /// their noise, as Siril, PixInsight and DSS do by default.
    pub const fn light() -> Self {
        Self {
            combine: Combine::sigma_clipped(2.5),
            weighting: Weighting::Noise,
            normalization: Normalization::Global,
            quality: QualityPlanes::STANDARD,
            min_survivors: DEFAULT_MIN_SURVIVORS,
        }
    }

    /// Preset for bias and dark frames: Winsorized σ=3.0, no normalization, equal weights. One
    /// preset, because both measure a level that every frame shares, with nothing between frames
    /// to normalize.
    pub const fn bias_or_dark() -> Self {
        Self {
            combine: Combine::winsorized(3.0),
            weighting: Weighting::Equal,
            normalization: Normalization::None,
            quality: QualityPlanes::STANDARD,
            min_survivors: DEFAULT_MIN_SURVIVORS,
        }
    }

    /// Preset for flat frames: σ-clip σ=3.0, multiplicative normalization, equal weights.
    pub const fn flat() -> Self {
        // σ=3.0 matches the bias-or-dark preset and ccdproc's `combine` default (3σ low/high); flats
        // are smooth, so a permissive cut just trims clear outliers (dust shadows move between
        // flats).
        Self {
            combine: Combine {
                method: CombineMethod::Mean(Rejection::sigma_clip(3.0)),
                // Stricter than the default floor: a master flat from fewer than 8 frames uses the
                // median, since σ-clip statistics on so few smooth flats aren't worth the noise.
                small_n: SmallN::median_below(8),
            },
            weighting: Weighting::Equal,
            normalization: Normalization::Multiplicative,
            quality: QualityPlanes::STANDARD,
            min_survivors: DEFAULT_MIN_SURVIVORS,
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
        if let CombineMethod::Mean(rejection) = &self.combine.method {
            rejection.validate()?;
        }

        if matches!(
            self.combine.small_n.fallback,
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
