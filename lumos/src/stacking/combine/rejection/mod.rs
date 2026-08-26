//! Pixel rejection algorithms for stacking.
//!
//! This module contains various outlier rejection methods used during image stacking:
//! - Sigma clipping (Kappa-Sigma)
//! - Winsorized sigma clipping
//! - Linear fit clipping
//! - Percentile clipping
//! - Generalized Extreme Studentized Deviate (GESD)
//!
//! [`Rejection`] is the enum a caller picks from; each variant's config type carries that method's
//! whole implementation and lives in its own file beside this one. What stays here is the enum, and
//! the handful of helpers the methods share.

pub(crate) mod gesd_config;
pub(crate) mod linear_fit_clip_config;
pub(crate) mod percentile_clip_config;
pub(crate) mod scratch_buffers;
pub(crate) mod sigma_bounds;
pub(crate) mod sigma_clip_config;
pub(crate) mod winsorized_clip_config;

use crate::error::InvalidConfigField;
use crate::math::sum;
use crate::stacking::combine::cache::sample::CombinedSample;
use crate::stacking::combine::rejection::gesd_config::GesdConfig;
use crate::stacking::combine::rejection::linear_fit_clip_config::LinearFitClipConfig;
use crate::stacking::combine::rejection::percentile_clip_config::PercentileClipConfig;
use crate::stacking::combine::rejection::scratch_buffers::ScratchBuffers;
use crate::stacking::combine::rejection::sigma_bounds::SigmaBounds;
use crate::stacking::combine::rejection::sigma_clip_config::SigmaClipConfig;
use crate::stacking::combine::rejection::winsorized_clip_config::WinsorizedClipConfig;

/// An iteration cap must leave at least one pass to run. Shared by the two methods that iterate.
fn validate_max_iterations(max_iterations: u32) -> Result<(), InvalidConfigField> {
    InvalidConfigField::check(
        max_iterations >= 1,
        "max_iterations",
        "at least 1",
        f64::from(max_iterations),
    )
}

/// Open a rejection: pair every value with its frame index, and report the survivor count straight
/// back when there are too few values to reject from.
///
/// `min_samples` is the smallest count the method can act on at all, not a tuning knob or a fast
/// path — a sigma clip has no spread to measure below three values, a linear fit no line to fit
/// below four. `Some` means the caller is done and everything survives, indices already paired.
fn begin_rejection(
    values: &[f32],
    scratch: &mut ScratchBuffers,
    min_samples: usize,
) -> Option<usize> {
    debug_assert!(!values.is_empty());
    scratch.reset_indices(values.len());
    (values.len() < min_samples).then_some(values.len())
}

/// Keep predicate for asymmetric sigma rejection: `diff = value − reference`, kept when it lies
/// within `[−low, high]` (the low- and high-side thresholds are applied separately).
#[inline]
fn within_threshold(diff: f32, bounds: SigmaBounds) -> bool {
    if diff < 0.0 {
        -diff <= bounds.low
    } else {
        diff <= bounds.high
    }
}

/// Compact in place the first `count` values (with their co-`indices`) whose deviation from
/// `reference(i)` stays within the asymmetric band `[−low, high]`, returning the survivor count.
/// Survivors keep their relative order and stay paired with their indices. `reference` is the
/// per-element comparison point — a constant `center` for sigma/winsorized clipping, or the fitted
/// `a + b·i` for linear-fit clipping.
fn compact_within(
    values: &mut [f32],
    indices: &mut [usize],
    count: usize,
    bounds: SigmaBounds,
    reference: impl Fn(usize) -> f32,
) -> usize {
    let mut write = 0;
    for read in 0..count {
        if within_threshold(values[read] - reference(read), bounds) {
            values[write] = values[read];
            indices[write] = indices[read];
            write += 1;
        }
    }
    write
}

/// MAD (median absolute deviation from `center`) of an **ascending-sorted** slice, without a
/// scratch buffer or quickselect. The absolute deviations split into two ascending runs — the
/// elements below `center` read backwards, and those at/above `center` read forwards — so a
/// two-pointer merge yields them in global ascending order. Advancing to rank `len/2` reproduces
/// `median_fast` of the deviations exactly (the same upper-middle order statistic).
fn sorted_mad(sorted: &[f32], center: f32) -> f32 {
    let m = sorted.len();
    debug_assert!(m > 0);
    let split = sorted.partition_point(|&v| v < center);
    let mut l = split; // left run consumes sorted[l - 1] going down
    let mut r = split; // right run consumes sorted[r] going up
    let target = m / 2;
    let mut dev = 0.0f32;
    for _ in 0..=target {
        let left = (l > 0).then(|| center - sorted[l - 1]);
        let right = (r < m).then(|| sorted[r] - center);
        dev = match (left, right) {
            (Some(ld), Some(rd)) if ld <= rd => {
                l -= 1;
                ld
            }
            (Some(_), Some(rd)) => {
                r += 1;
                rd
            }
            (Some(ld), None) => {
                l -= 1;
                ld
            }
            (None, Some(rd)) => {
                r += 1;
                rd
            }
            (None, None) => break,
        };
    }
    dev
}

/// Pixel rejection algorithm applied before combining.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rejection {
    /// No rejection.
    None,
    /// Iterative sigma clipping from median (symmetric or asymmetric).
    SigmaClip(SigmaClipConfig),
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
        Self::SigmaClip(SigmaClipConfig::new_asymmetric(sigma_low, sigma_high, 3))
    }

    /// Create winsorized sigma clipping.
    pub fn winsorized(sigma: f32) -> Self {
        Self::Winsorized(WinsorizedClipConfig::new(sigma))
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
        Self::Gesd(GesdConfig::default())
    }

    /// Validate the held configuration, if any.
    pub(super) fn validate(&self) -> Result<(), InvalidConfigField> {
        match self {
            Self::None => Ok(()),
            Self::SigmaClip(config) => config.validate(),
            Self::Winsorized(config) => config.validate(),
            Self::LinearFit(config) => config.validate(),
            Self::Percentile(config) => config.validate(),
            Self::Gesd(config) => config.validate(),
        }
    }

    /// Partition values by rejection algorithm, returning the number of survivors.
    ///
    /// After return, `values[..remaining]` holds the surviving values and `scratch.indices`
    /// their original frame indices (kept paired). `None` does no work and returns `values.len()`.
    fn reject(&self, values: &mut [f32], scratch: &mut ScratchBuffers) -> usize {
        match self {
            Rejection::None => values.len(),
            Rejection::SigmaClip(c) => c.reject(values, scratch),
            Rejection::Winsorized(c) => c.reject(values, scratch),
            Rejection::LinearFit(c) => c.reject(values, scratch),
            Rejection::Percentile(c) => c.reject(values, scratch),
            Rejection::Gesd(c) => c.reject(values, scratch),
        }
    }

    /// Reject outliers, then reduce the survivors to their weighted mean.
    ///
    /// The one reduction entry point: `values` and `weights` are the samples actually reaching
    /// this pixel, and the returned sample always carries its survivor count. `measure_quality`
    /// asks for the survivors' effective weight as well — skipped when no output plane will read
    /// it, since it is a second pass over the frames at every pixel.
    ///
    /// Rejection reorders `values`, so weights are re-paired through `scratch.indices` rather
    /// than by position.
    ///
    /// Only the mean is weighted; [`Self::reject`] decides survivors from the values alone. That
    /// is deliberate, and matches what ImageIntegration, Siril and DSS all do: rejection asks
    /// which samples disagree with their neighbours, which is a question about the normalized
    /// values, not about how much each frame is trusted. It is also what keeps GESD available —
    /// its critical values come from the t-distribution for `n` iid observations, and there is no
    /// weighted form of those tables, so a weighted GESD would be a different test rather than the
    /// same one with weights.
    pub(crate) fn combine_mean(
        &self,
        values: &mut [f32],
        weights: &[f32],
        scratch: &mut ScratchBuffers,
        measure_quality: bool,
    ) -> CombinedSample {
        debug_assert_eq!(values.len(), weights.len());
        if let Rejection::None = self {
            let value = sum::weighted_mean_f32(values, weights);
            return if measure_quality {
                CombinedSample::from_all(value, weights)
            } else {
                CombinedSample::value_only(value, values.len())
            };
        }

        let remaining = self.reject(values, scratch);
        let survivors = &scratch.indices[..remaining];
        let value = if remaining > 0 {
            weighted_mean_indexed(
                &values[..remaining],
                weights,
                survivors,
                &mut scratch.estimate_values,
            )
        } else {
            0.0
        };
        if measure_quality {
            CombinedSample::from_survivors(value, weights, remaining, survivors.iter().copied())
        } else {
            CombinedSample::value_only(value, remaining)
        }
    }
}

/// Weighted mean of rejection-reordered `values`: gathers each survivor's weight
/// via `indices[i] → weights[indices[i]]` into `scratch` so values and weights
/// align, then delegates to the precision-preserving [`sum::weighted_mean_f32`],
/// matching the unrejected branch. Returns `0.0` when the total weight is ~0.
///
/// `scratch` is a reused buffer (its prior contents are overwritten) so the
/// per-pixel combine path allocates nothing.
///
/// Preconditions: `indices.len() == values.len()`, all `indices[i] < weights.len()`.
fn weighted_mean_indexed(
    values: &[f32],
    weights: &[f32],
    indices: &[usize],
    scratch: &mut Vec<f32>,
) -> f32 {
    debug_assert_eq!(values.len(), indices.len());

    scratch.clear();
    scratch.extend(indices.iter().map(|&idx| weights[idx]));
    sum::weighted_mean_f32(values, scratch.as_slice())
}

#[cfg(test)]
mod tests;
