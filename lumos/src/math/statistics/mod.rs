//! Statistical functions: median, MAD, sigma-clipped statistics.

use crate::math::statistics::float::Float;

pub(crate) mod clipped_stats;
pub(crate) mod float;
pub(crate) mod median_mad;
pub(crate) mod radix_median;
pub(crate) mod spread;
pub(crate) mod subsample;

/// Replace each value with `|value − median|`, in place.
///
/// Twin of [`fill_abs_deviations`], which computes the same transform into a separate buffer.
/// They stay separate because the two contracts are incompatible, not by oversight: this one
/// consumes its input, that one preserves it, and a single function doing both would need input
/// and output to alias.
#[inline]
fn abs_deviation_inplace<F: Float>(values: &mut [F], median: F) {
    for v in values.iter_mut() {
        *v = (*v - median).abs();
    }
}

/// MAD (Median Absolute Deviation) to standard deviation conversion factor.
///
/// For a normal distribution σ ≈ 1.4826 × MAD; the factor is `1 / Φ⁻¹(3/4)`, where Φ⁻¹ is the
/// inverse normal CDF, carried to full double precision. A test holds it to the normal CDF.
///
/// `f64` is the canonical form, with [`MAD_TO_SIGMA_F32`] cast from it so the two precisions cannot
/// round apart. Reaching the `f64` users through `f64::from` on the `f32` constant instead would
/// spend an `f32` round-trip inside an `f64` computation for nothing.
const MAD_TO_SIGMA: f64 = 1.482_602_218_505_602;

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
/// the same confidence: the RANSAC scorer's outlier boundary uses it squared, match recovery uses
/// its square root as a radius. Two literals drifted apart once already (9.21 against a rounded 3.03).
pub(crate) const CHI2_99_2DOF: f64 = 9.210_340_371_976_182;

/// Convert MAD to standard deviation (assuming normal distribution).
#[inline]
pub(crate) const fn mad_to_sigma(mad: f32) -> f32 {
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
    MAD_TO_SIGMA * mad_with_scratch(data, median, scratch)
}

/// Median under [`Float::fast_cmp`], with no NaN handling: one partition, plus a scan of the lower
/// half for an even count, whose median is the mean of the two middle elements.
///
/// `data` must contain no NaN: comparing one orders it `Equal` against everything, which is not
/// a total order, and `select_nth_unstable_by` is then free to return any element. Not unsound —
/// just a meaningless median, which is why this is checked rather than left to the caller's
/// comment. Every decoded frame satisfies it (the FITS reader rejects non-finite pixels and RAW
/// decodes from integers), so the check is debug-only and the hot paths pay nothing in release.
#[inline]
fn median_fast<F: Float>(data: &mut [F]) -> F {
    debug_assert!(!data.is_empty());
    debug_assert!(
        !data.iter().any(|value| value.is_nan()),
        "median_fast requires NaN-free data; use median_mut for data that may hold NaN"
    );

    let len = data.len();
    let (lower, median, _) = data.select_nth_unstable_by(len / 2, F::fast_cmp);
    if len % 2 == 1 {
        return *median;
    }
    let below = lower.iter().copied().reduce(F::max).unwrap();
    (below + *median) * F::HALF
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

/// MAD of `values` about `median`, through [`median_mut`].
///
/// `MAD = median(|x_i - median(x)|)`, the deviations ranked under a total order, so data that may
/// hold NaN still measures.
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
pub(crate) const fn mad_floored(mad: f32, center: f32, floor_fraction: f32) -> f32 {
    mad.max(center * floor_fraction)
}

#[cfg(feature = "bench")]
pub(crate) mod bench;

#[cfg(test)]
mod tests;
