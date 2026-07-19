//! Statistical functions: median, MAD, sigma-clipped statistics.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::math::sum::mean_f32;

/// Per-channel robust statistics (median and MAD).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub(crate) struct ChannelStats {
    pub median: f32,
    pub mad: f32,
}

/// Compute absolute deviations from median in-place.
///
/// Replaces each value with |value - median|.
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

/// Fast approximate median using `partial_cmp` (single partition, no NaN handling).
///
/// Returns the upper-middle element for even-length arrays (no averaging).
/// Uses `partial_cmp` instead of `total_cmp` for ~30% faster comparisons.
/// Only safe for data guaranteed to contain no NaN values.
#[inline]
pub(crate) fn median_f32_fast(data: &mut [f32]) -> f32 {
    debug_assert!(!data.is_empty());

    let mid = data.len() / 2;
    let (_, median, _) =
        data.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    *median
}

/// Compute MAD using fast median (no NaN handling, single partition).
///
/// For use in rejection hot paths where data is guaranteed NaN-free.
/// Writes deviations directly into `scratch[..values.len()]` (must have sufficient length).
#[inline]
pub(crate) fn mad_f32_fast(values: &[f32], median: f32, scratch: &mut Vec<f32>) -> f32 {
    let n = values.len();
    if n == 0 {
        return 0.0;
    }
    scratch.resize(n, 0.0);
    for i in 0..n {
        scratch[i] = (values[i] - median).abs();
    }
    median_f32_fast(&mut scratch[..n])
}

/// Compute MAD (Median Absolute Deviation) using a scratch buffer.
///
/// MAD = median(|x_i - median(x)|)
#[inline]
pub(crate) fn mad_f32_with_scratch(values: &[f32], median: f32, scratch: &mut Vec<f32>) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    scratch.clear();
    scratch.extend(values.iter().map(|&v| (v - median).abs()));
    median_f32_mut(scratch)
}

/// Compute median and MAD (Median Absolute Deviation) together.
///
/// More efficient than computing separately since median is needed for MAD.
/// Mutates the input buffer.
pub(crate) fn median_and_mad_f32_mut(data: &mut [f32]) -> (f32, f32) {
    debug_assert!(!data.is_empty());

    let median = median_f32_mut(data);

    // Compute MAD in-place by replacing values with absolute deviations
    abs_deviation_inplace(data, median);
    let mad = median_f32_mut(data);

    (median, mad)
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

/// Statistics of the sigma-clip survivors, from [`sigma_clipped_median_mad`].
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
}

/// Result of a single sigma-clipping iteration.
enum ClipResult {
    /// Converged: no values were clipped (or sigma ≈ 0). Final stats.
    Converged(f32, f32),
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
        return ClipResult::Converged(median, 0.0);
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
        return ClipResult::Converged(median, sigma);
    }

    *len = write_idx;
    ClipResult::Clipped
}

/// Compute final statistics from remaining values.
#[inline]
fn compute_final_stats(values: &mut [f32], deviations: &mut [f32]) -> (f32, f32) {
    if values.is_empty() {
        return (0.0, 0.0);
    }

    let median = median_f32_mut(values);
    deviations[..values.len()].copy_from_slice(values);
    abs_deviation_inplace(&mut deviations[..values.len()], median);
    let mad = median_f32_mut(&mut deviations[..values.len()]);
    let sigma = mad_to_sigma(mad);

    (median, sigma)
}

/// Compute sigma-clipped median and MAD-based sigma.
///
/// Iteratively rejects outliers beyond `kappa × sigma` from the median.
/// Uses scratch buffer `deviations` for efficiency when called repeatedly.
///
/// # Arguments
/// * `values` - Mutable slice of values (will be reordered)
/// * `deviations` - Scratch buffer for deviations (reused between calls)
/// * `kappa` - Number of sigma for clipping threshold
/// * `iterations` - Number of clipping iterations
///
/// # Returns
/// [`ClippedStats`] (median, MAD-sigma, mean) of the clip survivors.
pub(crate) fn sigma_clipped_median_mad(
    values: &mut [f32],
    deviations: &mut Vec<f32>,
    kappa: f32,
    iterations: usize,
) -> ClippedStats {
    if values.is_empty() {
        return ClippedStats::ZERO;
    }

    let mut len = values.len();

    // Ensure deviations buffer has enough capacity
    deviations.resize(len, 0.0);

    let mut converged = None;
    for _ in 0..iterations {
        match sigma_clip_iteration(values, &mut len, deviations, kappa) {
            ClipResult::Converged(median, sigma) => {
                converged = Some((median, sigma));
                break;
            }
            ClipResult::TooFew => break,
            ClipResult::Clipped => {}
        }
    }

    // Every exit path leaves the survivors in `values[..len]` (len ≥ 1: at entry values is
    // non-empty and a clip pass always keeps at least the values at the median).
    let (median, sigma) = converged
        .unwrap_or_else(|| compute_final_stats(&mut values[..len], &mut deviations[..len]));
    ClippedStats {
        median,
        sigma,
        mean: mean_f32(&values[..len]),
    }
}

/// Sigma-clipped median and MAD computation using ArrayVec for zero heap allocation.
///
/// This is identical to `sigma_clipped_median_mad` but works with ArrayVec
/// instead of Vec for the deviations buffer, enabling stack allocation.
///
/// # Arguments
/// * `values` - Mutable slice of values (will be reordered)
/// * `deviations` - Pre-sized ArrayVec scratch buffer for deviations
/// * `kappa` - Number of sigma for clipping threshold
/// * `iterations` - Number of clipping iterations
///
/// # Returns
/// [`ClippedStats`] (median, MAD-sigma, mean) of the clip survivors.
pub(crate) fn sigma_clipped_median_mad_arrayvec<const N: usize>(
    values: &mut [f32],
    deviations: &mut arrayvec::ArrayVec<f32, N>,
    kappa: f32,
    iterations: usize,
) -> ClippedStats {
    if values.is_empty() {
        return ClippedStats::ZERO;
    }
    assert!(
        values.len() <= N,
        "sigma_clipped_median_mad_arrayvec: values.len()={} exceeds deviations capacity N={N}",
        values.len()
    );

    let mut len = values.len();

    // Ensure deviations buffer is sized correctly (len ≤ N, asserted above).
    deviations.clear();
    deviations.extend(std::iter::repeat_n(0.0f32, len));

    let mut converged = None;
    for _ in 0..iterations {
        match sigma_clip_iteration(values, &mut len, deviations.as_mut_slice(), kappa) {
            ClipResult::Converged(median, sigma) => {
                converged = Some((median, sigma));
                break;
            }
            ClipResult::TooFew => break,
            ClipResult::Clipped => {}
        }
    }

    let (median, sigma) = converged.unwrap_or_else(|| {
        compute_final_stats(&mut values[..len], &mut deviations.as_mut_slice()[..len])
    });
    ClippedStats {
        median,
        sigma,
        mean: mean_f32(&values[..len]),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bench;
