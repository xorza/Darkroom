//! Statistical functions: median, MAD, sigma-clipped statistics.

use super::deviation::abs_deviation_inplace;

/// MAD (Median Absolute Deviation) to standard deviation conversion factor.
///
/// For a normal distribution, σ ≈ 1.4826 × MAD.
/// This is the exact value: 1 / Φ⁻¹(3/4) where Φ⁻¹ is the inverse CDF.
pub const MAD_TO_SIGMA: f32 = 1.4826022;

/// Convert MAD to standard deviation (assuming normal distribution).
#[inline]
pub fn mad_to_sigma(mad: f32) -> f32 {
    mad * MAD_TO_SIGMA
}

/// Calculate the median of f32 values in-place using quickselect (O(n) average).
/// Mutates the input buffer (partial sort).
#[inline]
pub fn median_f32_mut(data: &mut [f32]) -> f32 {
    debug_assert!(!data.is_empty());

    let len = data.len();
    let mid = len / 2;

    if len.is_multiple_of(2) {
        // For even length, need both middle elements
        let (_, right_median, _) =
            data.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
        let right = *right_median;
        // Left median is max of left partition
        let left = data[..mid]
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        (left + right) / 2.0
    } else {
        let (_, median, _) = data.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
        *median
    }
}

/// Compute MAD (Median Absolute Deviation) using a scratch buffer.
///
/// MAD = median(|x_i - median(x)|)
#[inline]
pub fn mad_f32_with_scratch(values: &[f32], median: f32, scratch: &mut Vec<f32>) -> f32 {
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
pub fn median_and_mad_f32_mut(data: &mut [f32]) -> (f32, f32) {
    debug_assert!(!data.is_empty());

    let median = median_f32_mut(data);

    // Compute MAD in-place by replacing values with absolute deviations
    for v in data.iter_mut() {
        *v = (*v - median).abs();
    }
    let mad = median_f32_mut(data);

    (median, mad)
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
/// Tuple of (median, sigma) after clipping
pub fn sigma_clipped_median_mad(
    values: &mut [f32],
    deviations: &mut Vec<f32>,
    kappa: f32,
    iterations: usize,
) -> (f32, f32) {
    if values.is_empty() {
        return (0.0, 0.0);
    }

    let mut len = values.len();

    // Ensure deviations buffer has enough capacity
    if deviations.capacity() < len {
        deviations.reserve(len - deviations.capacity());
    }

    for _ in 0..iterations {
        if len < 3 {
            break;
        }

        let active = &mut values[..len];

        // Compute median
        let median = median_f32_mut(active);

        // Compute deviations using SIMD-accelerated in-place transform
        // We need to preserve original values for clipping, so copy first
        deviations.clear();
        deviations.extend_from_slice(active);
        abs_deviation_inplace(deviations, median);

        let mad = median_f32_mut(deviations);
        let sigma = mad_to_sigma(mad);

        if sigma < f32::EPSILON {
            return (median, 0.0);
        }

        // Clip values outside threshold
        // Recompute deviations inline to avoid dependency on partially-sorted buffer
        let threshold = kappa * sigma;
        let mut write_idx = 0;
        for i in 0..len {
            if (values[i] - median).abs() <= threshold {
                values[write_idx] = values[i];
                write_idx += 1;
            }
        }

        if write_idx == len {
            // Converged - no values clipped, current stats are final
            return (median, sigma);
        }
        len = write_idx;
    }

    // Compute final statistics after all iterations
    let active = &mut values[..len];
    if active.is_empty() {
        return (0.0, 0.0);
    }

    let median = median_f32_mut(active);

    deviations.clear();
    deviations.extend_from_slice(active);
    abs_deviation_inplace(deviations, median);
    let mad = median_f32_mut(deviations);
    let sigma = mad_to_sigma(mad);

    (median, sigma)
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
/// Tuple of (median, sigma) after clipping
pub fn sigma_clipped_median_mad_arrayvec<const N: usize>(
    values: &mut [f32],
    deviations: &mut arrayvec::ArrayVec<f32, N>,
    kappa: f32,
    iterations: usize,
) -> (f32, f32) {
    if values.is_empty() {
        return (0.0, 0.0);
    }

    let mut len = values.len();

    for _ in 0..iterations {
        if len < 3 {
            break;
        }

        let active = &mut values[..len];

        // Compute median
        let median = median_f32_mut(active);

        // Compute deviations - reuse for both MAD and clipping
        deviations.clear();
        for v in active.iter() {
            deviations.push((v - median).abs());
        }
        let mad = median_f32_mut(deviations.as_mut_slice());
        let sigma = mad_to_sigma(mad);

        if sigma < f32::EPSILON {
            return (median, 0.0);
        }

        // Clip values outside threshold, using already-computed deviations
        let threshold = kappa * sigma;
        let mut write_idx = 0;
        for i in 0..len {
            if deviations[i] <= threshold {
                values[write_idx] = values[i];
                write_idx += 1;
            }
        }

        if write_idx == len {
            // Converged - no values clipped, current stats are final
            return (median, sigma);
        }

        len = write_idx;
    }

    // Compute final statistics after all iterations
    let active = &mut values[..len];
    if active.is_empty() {
        return (0.0, 0.0);
    }

    let median = median_f32_mut(active);

    deviations.clear();
    for v in active.iter() {
        deviations.push((v - median).abs());
    }
    let mad = median_f32_mut(deviations.as_mut_slice());
    let sigma = mad_to_sigma(mad);

    (median, sigma)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bench;
