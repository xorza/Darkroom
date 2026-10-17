//! Statistical functions: median, MAD, sigma-clipped statistics.

use crate::math::statistics::float::{Float, OrderKeys};

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
const MAD_TO_SIGMA: f64 = 1.482_602_218_505_602;

/// `b_n` for n = 2 ..= 20, so that `b_n · 1.4826 · MAD` is unbiased for n Gaussian samples. The
/// MAD here takes the mean of the two middle values at an even count, for the median and for the
/// median of the deviations, and Croux & Rousseeuw's (1992) table does not fit that form at even n
/// (n = 6: 1.200 against 1.1895). Measured by `internals/reference/mad_consistency.py` from 10⁷
/// trials per count: the standard error is below 3·10⁻⁴ at n = 3 and falls with n.
const MAD_CONSISTENCY: [f64; 19] = [
    1.1955, 1.4869, 1.3604, 1.2170, 1.1895, 1.1378, 1.1274, 1.1011, 1.0958, 1.0799, 1.0765, 1.0661,
    1.0638, 1.0563, 1.0548, 1.0491, 1.0478, 1.0435, 1.0425,
];

/// χ²(0.99) for k = 2 degrees of freedom: the squared Mahalanobis radius enclosing 99% of an
/// isotropic 2-D Gaussian, so `r² > CHI2_99_2DOF · σ²` is the 1%-tail outlier test for a position
/// residual.
///
/// Exact in closed form — the k = 2 CDF is `1 − exp(−x/2)`, so the p-quantile is `−2·ln(1 − p)`,
/// here `−2·ln(0.01)`.
///
/// Lives here rather than beside either caller because registration gates 2-D residuals twice, at
/// the same confidence: the RANSAC scorer's outlier boundary uses it squared, match recovery uses
/// its square root as a radius, and one constant keeps the two gates at one confidence.
pub(crate) const CHI2_99_2DOF: f64 = 9.210_340_371_976_182;

/// The σ of `count` normal samples from their MAD: `1.4826 · b_n · MAD`, multiplied in `f64` and
/// rounded once, unbiased at every count.
#[inline]
pub(crate) fn sigma_of_mad<F: Float>(mad: F, count: usize) -> F {
    mad.scaled(MAD_TO_SIGMA * mad_consistency(count))
}

/// The small-sample factor `b_n` for `count` samples. One sample's MAD is 0 whatever scales it.
/// Above the table, Croux & Rousseeuw's `n / (n − 0.8)`, which the same simulation puts within
/// 0.1% of the measured factor from n = 20 on.
fn mad_consistency(count: usize) -> f64 {
    debug_assert!(count > 0);
    match count {
        1 => 1.0,
        2..=20 => MAD_CONSISTENCY[count - 2],
        _ => {
            let n = count as f64;
            n / (n - 0.8)
        }
    }
}

/// The median of `data`, in place.
///
/// Mutates the input buffer (partial sort via quickselect), and ranks NaN rather than tripping
/// over it: the selection runs on [`OrderKeys`], in `total_cmp`'s order.
#[inline]
pub(crate) fn median_mut<F: Float>(data: &mut [F]) -> F {
    debug_assert!(!data.is_empty());
    let mut keys = OrderKeys::of(data);
    let median = median_of_keys::<F>(keys.keys());
    keys.into_values();
    median
}

/// The median of `magnitudes`, in place: values with no sign bit set, as `|x − c|` makes them,
/// whose bits order as `total_cmp` does already, so they select with nothing mapped either way.
#[inline]
pub(super) fn median_of_magnitudes<F: Float>(magnitudes: &mut [F]) -> F {
    debug_assert!(!magnitudes.is_empty());
    let keys = F::as_keys(magnitudes);
    debug_assert!(
        keys.iter().all(|&key| key >= F::Key::default()),
        "a magnitude with its sign bit set"
    );
    median_of_keys::<F>(keys)
}

/// The median of the values `keys` stand for: the middle one of an odd count, the mean of the two
/// middle ones of an even count.
#[inline]
fn median_of_keys<F: Float>(keys: &mut [F::Key]) -> F {
    median_of_partitioned_keys::<F>(keys, 0)
}

/// [`median_of_keys`] of keys none of which before `boundary` is above one after it: the upper
/// middle lies on one side, and only that side is searched. Leaves the keys partitioned at the
/// upper middle, `len / 2`, as a selection over all of them does.
#[inline]
fn median_of_partitioned_keys<F: Float>(keys: &mut [F::Key], boundary: usize) -> F {
    let len = keys.len();
    let middle = len / 2;
    let (start, end) = if middle < boundary {
        (0, boundary)
    } else {
        (boundary, len)
    };
    keys[start..end].select_nth_unstable(middle - start);
    let upper = F::from_key(keys[middle]);
    if len % 2 == 1 {
        return upper;
    }
    // A fold from the first key, not `Iterator::max`, which compiles to a scalar loop: the fold
    // vectorizes, ten times as fast on arm64 and twice on x86.
    let lower_part = &keys[..middle];
    let lower = F::from_key(lower_part.iter().copied().fold(lower_part[0], Ord::max));
    lower.midpoint(upper)
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
    debug_assert!(!values.is_empty());
    fill_abs_deviations(values, median, scratch);
    median_of_magnitudes(scratch)
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

#[cfg(feature = "internals")]
pub(crate) mod bench;

#[cfg(test)]
pub(crate) mod internals {
    use crate::math::statistics::MAD_TO_SIGMA;
    use crate::math::statistics::float::Float;

    /// The σ of a normal distribution whose MAD is `mad`: `sigma_of_mad` as the count grows, for
    /// fixtures that state a population and no sample.
    pub(crate) fn mad_to_sigma<F: Float>(mad: F) -> F {
        mad.scaled(MAD_TO_SIGMA)
    }
}

#[cfg(test)]
mod tests;
