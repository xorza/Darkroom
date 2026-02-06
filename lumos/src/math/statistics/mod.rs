//! Statistical functions: median, MAD, sigma-clipped statistics.

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
pub const MAD_TO_SIGMA: f32 = 1.4826022;

/// Convert MAD to standard deviation (assuming normal distribution).
#[inline]
pub fn mad_to_sigma(mad: f32) -> f32 {
    mad * MAD_TO_SIGMA
}

/// Calculate the median of f32 values in-place.
///
/// Mutates the input buffer (partial sort via quickselect).
#[inline]
pub fn median_f32_mut(data: &mut [f32]) -> f32 {
    debug_assert!(!data.is_empty());

    let len = data.len();
    let mid = len / 2;

    if len & 1 == 1 {
        let (_, median, _) = data.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
        *median
    } else {
        let (left_part, right_median, _) =
            data.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
        let right = *right_median;
        let left = left_part.iter().copied().reduce(f32::max).unwrap();
        (left + right) * 0.5
    }
}

/// Fast approximate median.
///
/// For even-length arrays, returns the upper-middle element
/// instead of averaging — sufficient for iterative sigma clipping.
#[inline]
fn median_f32_approx(data: &mut [f32]) -> f32 {
    debug_assert!(!data.is_empty());

    let mid = data.len() / 2;
    let (_, median, _) = data.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
    *median
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
    abs_deviation_inplace(data, median);
    let mad = median_f32_mut(data);

    (median, mad)
}

/// Core sigma-clipping iteration logic shared between Vec and ArrayVec versions.
///
/// Returns (median, sigma, converged) where converged indicates no clipping occurred.
#[inline]
fn sigma_clip_iteration(
    values: &mut [f32],
    len: &mut usize,
    deviations: &mut [f32],
    kappa: f32,
) -> Option<(f32, f32)> {
    if *len < 3 {
        return None;
    }

    let active = &mut values[..*len];

    // Compute approximate median (faster - skips averaging for even lengths)
    let median = median_f32_approx(active);

    // Compute deviations using SIMD - copy values first, then transform
    deviations[..*len].copy_from_slice(active);
    abs_deviation_inplace(&mut deviations[..*len], median);

    let mad = median_f32_approx(&mut deviations[..*len]);
    let sigma = mad_to_sigma(mad);

    if sigma < f32::EPSILON {
        return Some((median, 0.0));
    }

    // Clip values outside threshold using computed deviations
    let threshold = kappa * sigma;
    let mut write_idx = 0;
    for i in 0..*len {
        if deviations[i] <= threshold {
            values[write_idx] = values[i];
            write_idx += 1;
        }
    }

    if write_idx == *len {
        // Converged - no values clipped
        return Some((median, sigma));
    }

    *len = write_idx;
    None
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
    deviations.resize(len, 0.0);

    for _ in 0..iterations {
        if let Some(result) = sigma_clip_iteration(values, &mut len, deviations, kappa) {
            return result;
        }
    }

    compute_final_stats(&mut values[..len], &mut deviations[..len])
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

    // Ensure deviations buffer is sized correctly
    deviations.clear();
    deviations.extend(std::iter::repeat_n(0.0f32, len.min(N)));

    for _ in 0..iterations {
        if let Some(result) =
            sigma_clip_iteration(values, &mut len, deviations.as_mut_slice(), kappa)
        {
            return result;
        }
    }

    compute_final_stats(&mut values[..len], &mut deviations.as_mut_slice()[..len])
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bench;
