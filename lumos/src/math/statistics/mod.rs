//! Statistical functions: median, MAD, sigma-clipped statistics.

use serde::{Deserialize, Serialize};

use crate::math::statistics::float::Float;
use crate::math::sum::mean_f32;

pub(crate) mod float;

/// A distribution's location and spread — what one pass over the values measures.
///
/// The spread is always the raw MAD, in the data's own units; [`Self::sigma`] rescales on
/// demand. Carrying the MAD rather than an already-scaled sigma is what keeps "median and MAD"
/// and "median and sigma" one type instead of two, and applies the 1.4826 factor exactly once,
/// where the caller needs Gaussian units.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct MedianMad {
    pub(crate) median: f32,
    pub(crate) mad: f32,
}

impl MedianMad {
    /// Measure both together, in one pass over `data`.
    ///
    /// More efficient than computing separately since the median is needed for the MAD.
    /// Mutates the input buffer.
    pub(crate) fn of_mut(data: &mut [f32]) -> Self {
        debug_assert!(!data.is_empty());

        let median = median_mut(data);
        abs_deviation_inplace(data, median);
        let mad = median_mut(data);

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
pub(crate) fn abs_deviation_inplace<F: Float>(values: &mut [F], median: F) {
    for v in values.iter_mut() {
        *v = (*v - median).abs();
    }
}

/// MAD (Median Absolute Deviation) to standard deviation conversion factor.
///
/// For a normal distribution σ ≈ 1.4826 × MAD; the factor is `1 / Φ⁻¹(3/4)`, where Φ⁻¹ is the
/// inverse normal CDF. Carried to the eight significant digits it has always had rather than the
/// full double-precision 1.482602218505602 — the two agree to within an `f32` ulp, so extending it
/// would move nothing single-precision, only the SIP clip threshold.
///
/// `f64` is the canonical form, with [`MAD_TO_SIGMA_F32`] cast from it so the two precisions cannot
/// round apart. The double-precision users used to reach the constant through `f64::from` on an
/// `f32`, spending an `f32` round-trip inside an `f64` computation for nothing.
pub(crate) const MAD_TO_SIGMA: f64 = 1.4826022;

/// [`MAD_TO_SIGMA`] in the precision the `f32` paths multiply in.
///
/// The nearest `f32` to the `f64` constant is the value those paths always used, so making the
/// `f64` one canonical left every single-precision result bit-identical.
const MAD_TO_SIGMA_F32: f32 = MAD_TO_SIGMA as f32;

/// χ²(0.99) for k = 2 degrees of freedom: the squared Mahalanobis radius enclosing 99% of an
/// isotropic 2-D Gaussian, so `r² > CHI2_99_2DOF · σ²` is the 1%-tail outlier test for a position
/// residual.
///
/// Exact in closed form — the k = 2 CDF is `1 − exp(−x/2)`, so the p-quantile is `−2·ln(1 − p)`,
/// here `−2·ln(0.01)`.
///
/// Lives here rather than beside either caller because registration gates 2-D residuals twice, at
/// the same confidence: MAGSAC's outlier boundary uses it squared, match recovery uses its square
/// root as a radius. Two literals drifted apart once already (9.21 against a rounded 3.03).
pub(crate) const CHI2_99_2DOF: f64 = 9.210_340_371_976_182;

/// Convert MAD to standard deviation (assuming normal distribution).
#[inline]
pub(crate) fn mad_to_sigma(mad: f32) -> f32 {
    mad * MAD_TO_SIGMA_F32
}

/// The median of `data`, in place.
///
/// Mutates the input buffer (partial sort via quickselect), and ranks NaN rather than tripping
/// over it — see [`Float::total_cmp`].
#[inline]
pub(crate) fn median_mut<F: Float>(data: &mut [F]) -> F {
    debug_assert!(!data.is_empty());

    let len = data.len();
    let mid = len / 2;

    if len & 1 == 1 {
        let (_, median, _) = data.select_nth_unstable_by(mid, F::total_cmp);
        *median
    } else {
        let (left_part, right_median, _) = data.select_nth_unstable_by(mid, F::total_cmp);
        let right = *right_median;
        let left = left_part.iter().copied().reduce(F::max).unwrap();
        (left + right) * F::HALF
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
    let median = median_mut(scratch);
    abs_deviation_inplace(scratch, median);
    MAD_TO_SIGMA * median_mut(scratch)
}

/// Fast approximate median: one partition under [`Float::fast_cmp`], no NaN handling.
///
/// Returns the upper-middle element for even-length arrays (no averaging). That convention is
/// what lets a caller that sorted and indexed `[len / 2]` switch to this and get the same value
/// out: a full sort's element at that rank is exactly what one selection returns.
///
/// `data` must contain no NaN: comparing one orders it `Equal` against everything, which is not
/// a total order, and `select_nth_unstable_by` is then free to return any element. Not unsound —
/// just a meaningless median, which is why this is checked rather than left to the caller's
/// comment. Every decoded frame satisfies it (the FITS reader rejects non-finite pixels and RAW
/// decodes from integers), so the check is debug-only and the hot paths pay nothing in release.
#[inline]
pub(crate) fn median_fast<F: Float>(data: &mut [F]) -> F {
    debug_assert!(!data.is_empty());
    debug_assert!(
        !data.iter().any(|value| value.is_nan()),
        "median_fast requires NaN-free data; use median_mut for data that may hold NaN"
    );

    let mid = data.len() / 2;
    let (_, median, _) = data.select_nth_unstable_by(mid, F::fast_cmp);
    *median
}

/// Replace `scratch` with `|value - median|` for each of `values`, leaving it exactly as long.
///
/// One pass: the subtraction rides on the copy rather than following it, and the buffer's
/// previous contents are never written before being overwritten.
///
/// Twin of [`abs_deviation_inplace`] — see there for why the pair does not collapse into one.
#[inline]
fn fill_abs_deviations<F: Float>(values: &[F], median: F, scratch: &mut Vec<F>) {
    scratch.clear();
    scratch.extend(values.iter().map(|&value| (value - median).abs()));
}

/// MAD of `values` about `median`, through [`median_fast`].
///
/// For rejection hot paths whose data is guaranteed NaN-free — see [`median_fast`] for what a NaN
/// would cost and why the check is debug-only. Checked here as well as there so a violation names
/// the caller's data rather than the derived deviations.
#[inline]
pub(crate) fn mad_fast<F: Float>(values: &[F], median: F, scratch: &mut Vec<F>) -> F {
    debug_assert!(
        !median.is_nan() && !values.iter().any(|value| value.is_nan()),
        "mad_fast requires a NaN-free median and values; use mad_with_scratch otherwise"
    );
    if values.is_empty() {
        return F::ZERO;
    }
    fill_abs_deviations(values, median, scratch);
    median_fast(scratch)
}

/// MAD of `values` about `median`, through [`median_mut`].
///
/// `MAD = median(|x_i - median(x)|)`. The NaN-tolerant twin of [`mad_fast`]: same shape, but the
/// deviations are ranked under a total order, so data that may hold NaN still measures.
#[inline]
pub(crate) fn mad_with_scratch<F: Float>(values: &[F], median: F, scratch: &mut Vec<F>) -> F {
    if values.is_empty() {
        return F::ZERO;
    }
    fill_abs_deviations(values, median, scratch);
    median_mut(scratch)
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
    pub(crate) median: f32,
    /// MAD-based sigma of the survivors.
    pub(crate) sigma: f32,
    /// Mean of the survivors. With the median it exposes the residual skew of the clipped
    /// distribution — what SExtractor's Pearson mode estimator corrects for.
    pub(crate) mean: f32,
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
    /// `values` must be NaN-free — the clip iteration medians with [`median_fast`] — and is
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
    let median = median_fast(active);

    deviations[..*len].copy_from_slice(active);
    abs_deviation_inplace(&mut deviations[..*len], median);

    let mad = median_fast(&mut deviations[..*len]);
    let sigma = mad_to_sigma(mad);

    // Degenerate against the data's own magnitude, not against a fixed number: one `f32` step at
    // `median` is the smallest spread the samples can even represent, so a σ below it is genuinely
    // unmeasurable — whatever span the decoder divided by. A bare `sigma < f32::EPSILON` instead
    // declares any frame whose whole noise range sits under 1.2e-7 to be flat, which is what a
    // 32-bit integer FITS becomes once it is normalized, and hands back a zero σ that collapses
    // every threshold built from it.
    if sigma <= median.abs() * f32::EPSILON {
        return ClipResult::Converged(MedianMad { median, mad: 0.0 });
    }

    // Clip values outside threshold, computing deviations on-the-fly.
    // (The deviations buffer was scrambled by median_fast above,
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

    let median = median_mut(values);
    deviations[..values.len()].copy_from_slice(values);
    abs_deviation_inplace(&mut deviations[..values.len()], median);
    let mad = median_mut(&mut deviations[..values.len()]);

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
