//! Statistical functions: median, MAD, sigma-clipped statistics.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::math::sum::mean_f32;

/// A distribution's location and spread — what one pass over the values measures.
///
/// The spread is always the raw MAD, in the data's own units; [`Self::sigma`] rescales on
/// demand. Carrying the MAD rather than an already-scaled sigma is what keeps "median and MAD"
/// and "median and sigma" one type instead of two, and applies the 1.4826 factor exactly once,
/// where the caller needs Gaussian units.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct MedianMad {
    pub median: f32,
    pub mad: f32,
}

impl MedianMad {
    /// Measure both together, in one pass over `data`.
    ///
    /// More efficient than computing separately since the median is needed for the MAD.
    /// Mutates the input buffer.
    pub(crate) fn of_mut(data: &mut [f32]) -> Self {
        debug_assert!(!data.is_empty());

        let median = median_f32_mut(data);
        abs_deviation_inplace(data, median);
        let mad = median_f32_mut(data);

        Self { median, mad }
    }

    /// The MAD rescaled to the standard deviation of an equivalent normal distribution.
    #[inline]
    pub(crate) fn sigma(self) -> f32 {
        mad_to_sigma(self.mad)
    }
}

/// Replace each value with `|value − median|`, in place.
///
/// Twin of [`fill_abs_deviations`], which computes the same transform into a separate buffer.
/// They stay separate because the two contracts are incompatible, not by oversight: this one
/// consumes its input, that one preserves it, and a single function doing both would need input
/// and output to alias.
#[inline]
fn abs_deviation_inplace(values: &mut [f32], median: f32) {
    for v in values.iter_mut() {
        *v = (*v - median).abs();
    }
}

/// MAD (Median Absolute Deviation) to standard deviation conversion factor.
///
/// For a normal distribution, σ ≈ 1.4826 × MAD.
/// This is the exact value: 1 / Φ⁻¹(3/4) where Φ⁻¹ is the inverse CDF.
pub(crate) const MAD_TO_SIGMA: f32 = 1.4826022;

/// Convert MAD to standard deviation (assuming normal distribution).
#[inline]
pub(crate) fn mad_to_sigma(mad: f32) -> f32 {
    mad * MAD_TO_SIGMA
}

/// Calculate the median of f32 values in-place.
///
/// Mutates the input buffer (partial sort via quickselect).
#[inline]
pub(crate) fn median_f32_mut(data: &mut [f32]) -> f32 {
    debug_assert!(!data.is_empty());

    let len = data.len();
    let mid = len / 2;

    if len & 1 == 1 {
        let (_, median, _) = data.select_nth_unstable_by(mid, f32::total_cmp);
        *median
    } else {
        let (left_part, right_median, _) = data.select_nth_unstable_by(mid, f32::total_cmp);
        let right = *right_median;
        let left = left_part.iter().copied().reduce(f32::max).unwrap();
        (left + right) * 0.5
    }
}

/// Calculate the median of f64 values in-place.
///
/// The `f64` counterpart to [`median_f32_mut`], and the crate's only one: the polynomial surface
/// fit in `background_extraction` works in `f64` throughout, so downcasting its residuals to run
/// the `f32` path would lose precision inside a fitting loop.
#[inline]
pub(crate) fn median_f64_mut(data: &mut [f64]) -> f64 {
    debug_assert!(!data.is_empty());

    let len = data.len();
    let mid = len / 2;

    if len & 1 == 1 {
        let (_, median, _) = data.select_nth_unstable_by(mid, f64::total_cmp);
        *median
    } else {
        let (left_part, right_median, _) = data.select_nth_unstable_by(mid, f64::total_cmp);
        let right = *right_median;
        let left = left_part.iter().copied().reduce(f64::max).unwrap();
        (left + right) * 0.5
    }
}

/// MAD-scaled robust sigma of `data`: `1.4826 · median|x − median(x)|`, or `0.0` when empty.
///
/// Leaves `data` intact — callers that go on to threshold against their own residuals need them
/// — by working entirely in `scratch`, which is overwritten in full. Hoist `scratch` out of a
/// refit loop and the whole iteration allocates nothing.
pub(crate) fn robust_sigma_f64(data: &[f64], scratch: &mut Vec<f64>) -> f64 {
    if data.is_empty() {
        return 0.0;
    }

    scratch.clear();
    scratch.extend_from_slice(data);
    let median = median_f64_mut(scratch);
    for value in scratch.iter_mut() {
        *value = (*value - median).abs();
    }
    f64::from(MAD_TO_SIGMA) * median_f64_mut(scratch)
}

/// Fast approximate median using `partial_cmp` (single partition, no NaN handling).
///
/// Returns the upper-middle element for even-length arrays (no averaging).
/// Uses `partial_cmp` instead of `total_cmp` for ~30% faster comparisons.
///
/// `data` must contain no NaN: comparing one orders it `Equal` against everything, which is not
/// a total order, and `select_nth_unstable_by` is then free to return any element. Not unsound —
/// just a meaningless median, which is why this is checked rather than left to the caller's
/// comment. Every decoded frame satisfies it (the FITS reader rejects non-finite pixels and RAW
/// decodes from integers), so the check is debug-only and the hot paths pay nothing in release.
#[inline]
pub(crate) fn median_f32_fast(data: &mut [f32]) -> f32 {
    debug_assert!(!data.is_empty());
    debug_assert!(
        !data.iter().any(|value| value.is_nan()),
        "median_f32_fast requires NaN-free data; use median_f32_mut for data that may hold NaN"
    );

    let mid = data.len() / 2;
    let (_, median, _) =
        data.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    *median
}

/// Replace `scratch` with `|value - median|` for each of `values`, leaving it exactly as long.
///
/// One pass: the subtraction rides on the copy rather than following it, and the buffer's
/// previous contents are never written before being overwritten.
///
/// Twin of [`abs_deviation_inplace`] — see there for why the pair does not collapse into one.
#[inline]
fn fill_abs_deviations(values: &[f32], median: f32, scratch: &mut Vec<f32>) {
    scratch.clear();
    scratch.extend(values.iter().map(|&value| (value - median).abs()));
}

/// Compute MAD using fast median (no NaN handling, single partition).
///
/// For use in rejection hot paths where data is guaranteed NaN-free — see
/// [`median_f32_fast`] for what a NaN would cost and why the check is debug-only. Checked here
/// as well as there so a violation names the caller's data rather than the derived deviations.
#[inline]
pub(crate) fn mad_f32_fast(values: &[f32], median: f32, scratch: &mut Vec<f32>) -> f32 {
    debug_assert!(
        !median.is_nan() && !values.iter().any(|value| value.is_nan()),
        "mad_f32_fast requires a NaN-free median and values; use mad_f32_with_scratch otherwise"
    );
    if values.is_empty() {
        return 0.0;
    }
    fill_abs_deviations(values, median, scratch);
    median_f32_fast(scratch)
}

/// Compute MAD (Median Absolute Deviation) using a scratch buffer.
///
/// MAD = median(|x_i - median(x)|)
#[inline]
pub(crate) fn mad_f32_with_scratch(values: &[f32], median: f32, scratch: &mut Vec<f32>) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    fill_abs_deviations(values, median, scratch);
    median_f32_mut(scratch)
}

/// MAD floored at `floor_fraction * center`.
///
/// A near-degenerate distribution (values nearly identical) has MAD ≈ 0, which would
/// collapse any MAD-scaled rejection threshold to zero. Flooring at a fraction of the
/// center keeps a usable spread estimate. Callers pass the median as `center`.
#[inline]
pub(crate) fn mad_floored(mad: f32, center: f32, floor_fraction: f32) -> f32 {
    mad.max(center * floor_fraction)
}

/// Scratch space a sigma-clip pass borrows for its per-value deviations.
///
/// Exists so one clip implementation serves both buffer kinds: the tiled background walks
/// thousands of tiles and reuses one heap `Vec`, while the centroid measure loop runs per star
/// inside a rayon fold and must not allocate at all. Contents are never read before being
/// written, so an implementation only has to produce a slice of the right length.
pub(crate) trait DeviationScratch {
    /// Yield exactly `len` elements to write into.
    fn sized_to(&mut self, len: usize) -> &mut [f32];
}

impl DeviationScratch for Vec<f32> {
    fn sized_to(&mut self, len: usize) -> &mut [f32] {
        self.resize(len, 0.0);
        self
    }
}

impl<const N: usize> DeviationScratch for arrayvec::ArrayVec<f32, N> {
    /// Grows to `len` rather than starting from a zeroed `[f32; N]`: the caller sizes `N` to its
    /// worst case, so a plain array would memset the whole capacity on every call while only
    /// `len` of it is ever used. Panics when `len > N` — a fixed buffer cannot grow.
    fn sized_to(&mut self, len: usize) -> &mut [f32] {
        self.clear();
        self.extend(std::iter::repeat_n(0.0f32, len));
        self.as_mut_slice()
    }
}

/// Statistics of the sigma-clip survivors, from [`ClippedStats::sigma_clipped`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClippedStats {
    pub median: f32,
    /// MAD-based sigma of the survivors.
    pub sigma: f32,
    /// Mean of the survivors. With the median it exposes the residual skew of the clipped
    /// distribution — what SExtractor's Pearson mode estimator corrects for.
    pub mean: f32,
}

impl ClippedStats {
    const ZERO: ClippedStats = ClippedStats {
        median: 0.0,
        sigma: 0.0,
        mean: 0.0,
    };

    /// Widen a clip pass's estimate with the mean of the same survivors.
    fn from_median_mad(location: MedianMad, mean: f32) -> Self {
        Self {
            median: location.median,
            sigma: location.sigma(),
            mean,
        }
    }

    /// Sigma-clipped median, MAD-sigma and mean of `values`, rejecting outliers beyond
    /// `kappa × sigma` from the median over `iterations` passes.
    ///
    /// `values` must be NaN-free — the clip iteration medians with [`median_f32_fast`] — and is
    /// reordered in place.
    ///
    /// `deviations` is borrowed scratch, overwritten in full; only its length matters. Sizing it
    /// is [`DeviationScratch`]'s job, so a heap caller can reuse one `Vec` across calls and a
    /// stamp-sized caller can keep its scratch on the stack, without this having to know which.
    pub(crate) fn sigma_clipped(
        values: &mut [f32],
        deviations: &mut impl DeviationScratch,
        kappa: f32,
        iterations: usize,
    ) -> Self {
        if values.is_empty() {
            return ClippedStats::ZERO;
        }

        sigma_clipped_core(values, deviations.sized_to(values.len()), kappa, iterations)
    }
}

/// Result of a single sigma-clipping iteration.
enum ClipResult {
    /// Converged: no values were clipped (or sigma ≈ 0). Final stats.
    Converged(MedianMad),
    /// Values were clipped; continue iterating.
    Clipped,
    /// Too few values remain (< 3) to compute meaningful statistics.
    TooFew,
}

/// Core sigma-clipping iteration logic shared between Vec and ArrayVec versions.
#[inline]
fn sigma_clip_iteration(
    values: &mut [f32],
    len: &mut usize,
    deviations: &mut [f32],
    kappa: f32,
) -> ClipResult {
    if *len < 3 {
        return ClipResult::TooFew;
    }

    let active = &mut values[..*len];

    // Compute approximate median (fast — partial_cmp, single partition)
    let median = median_f32_fast(active);

    deviations[..*len].copy_from_slice(active);
    abs_deviation_inplace(&mut deviations[..*len], median);

    let mad = median_f32_fast(&mut deviations[..*len]);
    let sigma = mad_to_sigma(mad);

    if sigma < f32::EPSILON {
        return ClipResult::Converged(MedianMad { median, mad: 0.0 });
    }

    // Clip values outside threshold, computing deviations on-the-fly.
    // (The deviations buffer was scrambled by median_f32_fast above,
    // so we recompute each deviation inline instead of a separate pass.)
    let threshold = kappa * sigma;
    let mut write_idx = 0;
    for i in 0..*len {
        if (values[i] - median).abs() <= threshold {
            values[write_idx] = values[i];
            write_idx += 1;
        }
    }

    if write_idx == *len {
        // Converged - no values clipped
        return ClipResult::Converged(MedianMad { median, mad });
    }

    *len = write_idx;
    ClipResult::Clipped
}

/// Compute final statistics from remaining values.
#[inline]
fn compute_final_stats(values: &mut [f32], deviations: &mut [f32]) -> MedianMad {
    if values.is_empty() {
        return MedianMad {
            median: 0.0,
            mad: 0.0,
        };
    }

    let median = median_f32_mut(values);
    deviations[..values.len()].copy_from_slice(values);
    abs_deviation_inplace(&mut deviations[..values.len()], median);
    let mad = median_f32_mut(&mut deviations[..values.len()]);

    MedianMad { median, mad }
}

/// Iteratively clip `values`, then measure what survived.
///
/// `deviations` must already be `values.len()` long; the two public entry points differ only in
/// how they get it that way, so everything after the sizing lives here.
fn sigma_clipped_core(
    values: &mut [f32],
    deviations: &mut [f32],
    kappa: f32,
    iterations: usize,
) -> ClippedStats {
    let mut len = values.len();

    let mut converged = None;
    for _ in 0..iterations {
        match sigma_clip_iteration(values, &mut len, deviations, kappa) {
            ClipResult::Converged(location) => {
                converged = Some(location);
                break;
            }
            ClipResult::TooFew => break,
            ClipResult::Clipped => {}
        }
    }

    // Every exit path leaves the survivors in `values[..len]` (len ≥ 1: at entry values is
    // non-empty and a clip pass always keeps at least the values at the median).
    let location = converged
        .unwrap_or_else(|| compute_final_stats(&mut values[..len], &mut deviations[..len]));
    ClippedStats::from_median_mad(location, mean_f32(&values[..len]))
}

/// Compute sigma-clipped median and MAD-based sigma.
#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "internals"))]
mod bench;
