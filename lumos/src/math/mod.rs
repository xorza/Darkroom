//! Math utilities with SIMD acceleration.
//!
//! Provides optimized math operations with platform-specific SIMD for ARM NEON (aarch64) and x86 SSE4.

mod bbox;
pub mod scalar;
mod vec2;

pub use bbox::Aabb;
pub use vec2::Vec2us;

/// FWHM to Gaussian sigma conversion factor.
///
/// For a Gaussian distribution, FWHM = 2√(2ln2) × σ ≈ 2.3548 × σ.
/// This is the exact value: 2 * sqrt(2 * ln(2)).
pub const FWHM_TO_SIGMA: f32 = 2.354_82;

/// MAD (Median Absolute Deviation) to standard deviation conversion factor.
///
/// For a normal distribution, σ ≈ 1.4826 × MAD.
/// This is the exact value: 1 / Φ⁻¹(3/4) where Φ⁻¹ is the inverse CDF.
pub const MAD_TO_SIGMA: f32 = 1.4826022;

/// Convert FWHM to Gaussian sigma.
#[inline]
pub fn fwhm_to_sigma(fwhm: f32) -> f32 {
    fwhm / FWHM_TO_SIGMA
}

/// Convert Gaussian sigma to FWHM.
#[inline]
pub fn sigma_to_fwhm(sigma: f32) -> f32 {
    sigma * FWHM_TO_SIGMA
}

/// Convert MAD to standard deviation (assuming normal distribution).
#[inline]
pub fn mad_to_sigma(mad: f32) -> f32 {
    mad * MAD_TO_SIGMA
}

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod sse;

/// Sum f32 values using SIMD when available.
pub fn sum_f32(values: &[f32]) -> f32 {
    #[cfg(target_arch = "aarch64")]
    {
        if values.len() >= 4 {
            return unsafe { neon::sum_f32(values) };
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if values.len() >= 4 && common::cpu_features::has_sse4_1() {
            return unsafe { sse::sum_f32(values) };
        }
    }
    scalar::sum_f32(values)
}

/// Calculate the mean of f32 values using SIMD-accelerated sum.
pub fn mean_f32(values: &[f32]) -> f32 {
    debug_assert!(!values.is_empty());
    sum_f32(values) / values.len() as f32
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
///
/// Uses the provided scratch buffer to avoid allocation.
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

    for _ in 0..iterations {
        if len < 3 {
            break;
        }

        let active = &mut values[..len];

        // Compute median
        let median = median_f32_mut(active);

        // Compute MAD using deviations buffer
        deviations.clear();
        deviations.extend(active.iter().map(|v| (v - median).abs()));
        let mad = median_f32_mut(deviations);
        let sigma = mad_to_sigma(mad);

        if sigma < f32::EPSILON {
            return (median, 0.0);
        }

        // Clip values outside threshold
        let threshold = kappa * sigma;
        let mut write_idx = 0;
        for i in 0..len {
            if (values[i] - median).abs() <= threshold {
                values[write_idx] = values[i];
                write_idx += 1;
            }
        }

        if write_idx == len {
            break;
        }
        len = write_idx;
    }

    // Final statistics
    let active = &mut values[..len];
    if active.is_empty() {
        return (0.0, 0.0);
    }

    let median = median_f32_mut(active);

    deviations.clear();
    deviations.extend(active.iter().map(|v| (v - median).abs()));
    let mad = median_f32_mut(deviations);
    let sigma = mad_to_sigma(mad);

    (median, sigma)
}

/// Calculate sum of squared differences from mean using SIMD when available.
pub fn sum_squared_diff(values: &[f32], mean: f32) -> f32 {
    #[cfg(target_arch = "aarch64")]
    {
        if values.len() >= 4 {
            return unsafe { neon::sum_squared_diff(values, mean) };
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if values.len() >= 4 && common::cpu_features::has_sse4_1() {
            return unsafe { sse::sum_squared_diff(values, mean) };
        }
    }
    scalar::sum_squared_diff(values, mean)
}

/// Accumulate src into dst (dst[i] += src[i]).
/// Uses scalar implementation (compiler auto-vectorizes effectively).
#[inline]
pub fn accumulate(dst: &mut [f32], src: &[f32]) {
    scalar::accumulate(dst, src)
}

/// Scale values in-place (data[i] *= scale).
/// Uses scalar implementation (compiler auto-vectorizes effectively).
#[inline]
pub fn scale(data: &mut [f32], scale_val: f32) {
    scalar::scale(data, scale_val)
}

/// Compute sum of f32 slice in parallel using rayon.
/// Each thread computes a partial sum, then results are combined.
pub fn parallel_sum_f32(values: &[f32]) -> f32 {
    use rayon::prelude::*;
    values
        .par_chunks(rayon::current_num_threads())
        .map(|chunk| chunk.iter().sum::<f32>())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fwhm_sigma_conversion_roundtrip() {
        let fwhm = 4.5;
        let sigma = fwhm_to_sigma(fwhm);
        let fwhm_back = sigma_to_fwhm(sigma);
        assert!((fwhm - fwhm_back).abs() < 1e-6);
    }

    #[test]
    fn test_fwhm_to_sigma_known_value() {
        // For FWHM = 2.3548, sigma should be ~1.0
        let sigma = fwhm_to_sigma(FWHM_TO_SIGMA);
        assert!((sigma - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_mad_to_sigma_known_value() {
        // For MAD = 1.0, sigma should be ~1.4826
        let sigma = mad_to_sigma(1.0);
        assert!((sigma - MAD_TO_SIGMA).abs() < 1e-6);
    }

    #[test]
    fn test_median_odd() {
        let mut values = [1.0f32, 3.0, 2.0, 5.0, 4.0];
        assert!((median_f32_mut(&mut values) - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_even() {
        let mut values = [1.0f32, 2.0, 3.0, 4.0];
        assert!((median_f32_mut(&mut values) - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_f32() {
        let values: Vec<f32> = (1..=16).map(|x| x as f32).collect();
        let expected: f32 = values.iter().sum();
        let result = sum_f32(&values);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_f32_remainder() {
        let values: Vec<f32> = (1..=13).map(|x| x as f32).collect();
        let expected: f32 = values.iter().sum();
        let result = sum_f32(&values);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_f32_small() {
        let values = vec![1.0f32, 2.0, 3.0];
        let expected: f32 = values.iter().sum();
        let result = sum_f32(&values);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_squared_diff() {
        let values: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let mean_val: f32 = 4.5;
        let expected: f32 = values.iter().map(|v| (v - mean_val).powi(2)).sum();
        let result = sum_squared_diff(&values, mean_val);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_squared_diff_remainder() {
        let values: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0];
        let mean_val: f32 = 4.0;
        let expected: f32 = values.iter().map(|v| (v - mean_val).powi(2)).sum();
        let result = sum_squared_diff(&values, mean_val);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_squared_diff_small() {
        let values: Vec<f32> = vec![1.0, 2.0, 3.0];
        let mean_val: f32 = 2.0;
        let expected: f32 = values.iter().map(|v| (v - mean_val).powi(2)).sum();
        let result = sum_squared_diff(&values, mean_val);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_mean_f32() {
        let values: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let expected = 4.5;
        let result = mean_f32(&values);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_mean_f32_single() {
        let values = vec![42.0f32];
        assert!((mean_f32(&values) - 42.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_f32_single() {
        let values = vec![42.0f32];
        assert!((sum_f32(&values) - 42.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_f32_empty() {
        let values: Vec<f32> = vec![];
        assert!((sum_f32(&values) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_f32_single() {
        let mut values = [42.0f32];
        assert!((median_f32_mut(&mut values) - 42.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_and_mad_odd() {
        // [2.0, 3.0, 4.0] -> median = 3.0
        // deviations: [1.0, 0.0, 1.0] -> MAD = 1.0
        let mut values = [2.0f32, 4.0, 3.0];
        let (median, mad) = median_and_mad_f32_mut(&mut values);
        assert!((median - 3.0).abs() < 1e-6);
        assert!((mad - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_median_and_mad_uniform() {
        // All same values -> MAD = 0
        let mut values = [3.5f32, 3.5, 3.5, 3.5, 3.5];
        let (median, mad) = median_and_mad_f32_mut(&mut values);
        assert!((median - 3.5).abs() < 1e-6);
        assert!(mad.abs() < 1e-6);
    }

    #[test]
    fn test_mad_with_scratch() {
        let values = [2.0f32, 4.0, 3.0];
        let mut scratch = Vec::new();
        let mad = mad_f32_with_scratch(&values, 3.0, &mut scratch);
        assert!((mad - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_mad_with_scratch_empty() {
        let values: [f32; 0] = [];
        let mut scratch = Vec::new();
        let mad = mad_f32_with_scratch(&values, 0.0, &mut scratch);
        assert!(mad.abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_f32_negative() {
        let values: Vec<f32> = vec![-1.0, -2.0, -3.0, -4.0, 5.0, 6.0, 7.0, 8.0];
        let expected: f32 = values.iter().sum();
        let result = sum_f32(&values);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_sum_squared_diff_negative() {
        let values: Vec<f32> = vec![-4.0, -2.0, 0.0, 2.0, 4.0, 6.0, 8.0, 10.0];
        let mean_val: f32 = 3.0;
        let expected: f32 = values.iter().map(|v| (v - mean_val).powi(2)).sum();
        let result = sum_squared_diff(&values, mean_val);
        assert!((result - expected).abs() < 1e-4);
    }

    #[test]
    fn test_median_f32_negative() {
        let mut values = [-5.0f32, -3.0, -1.0, 2.0, 4.0];
        assert!((median_f32_mut(&mut values) - (-1.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn test_simd_vs_scalar_sum() {
        let values: Vec<f32> = (0..1000).map(|x| x as f32 * 0.1).collect();
        let scalar_result = scalar::sum_f32(&values);
        let simd_result = sum_f32(&values);
        assert!(
            (scalar_result - simd_result).abs() < 1e-2,
            "scalar={}, simd={}",
            scalar_result,
            simd_result
        );
    }

    #[test]
    fn test_simd_vs_scalar_sum_squared_diff() {
        let values: Vec<f32> = (0..1000).map(|x| x as f32 * 0.1).collect();
        let mean = values.iter().sum::<f32>() / values.len() as f32;
        let scalar_result = scalar::sum_squared_diff(&values, mean);
        let simd_result = sum_squared_diff(&values, mean);
        assert!(
            (scalar_result - simd_result).abs() < 1e-1,
            "scalar={}, simd={}",
            scalar_result,
            simd_result
        );
    }

    #[test]
    fn test_accumulate() {
        let mut dst: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let src: Vec<f32> = vec![0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5];
        accumulate(&mut dst, &src);
        assert_eq!(dst, vec![1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5, 8.5]);
    }

    #[test]
    fn test_accumulate_remainder() {
        let mut dst: Vec<f32> = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let src: Vec<f32> = vec![0.5, 0.5, 0.5, 0.5, 0.5];
        accumulate(&mut dst, &src);
        assert_eq!(dst, vec![1.5, 2.5, 3.5, 4.5, 5.5]);
    }

    #[test]
    fn test_accumulate_small() {
        let mut dst: Vec<f32> = vec![1.0, 2.0];
        let src: Vec<f32> = vec![0.5, 0.5];
        accumulate(&mut dst, &src);
        assert_eq!(dst, vec![1.5, 2.5]);
    }

    #[test]
    fn test_scale() {
        let mut data: Vec<f32> = vec![2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0];
        scale(&mut data, 0.5);
        assert_eq!(data, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]);
    }

    #[test]
    fn test_scale_remainder() {
        let mut data: Vec<f32> = vec![2.0, 4.0, 6.0, 8.0, 10.0];
        scale(&mut data, 0.5);
        assert_eq!(data, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn test_scale_small() {
        let mut data: Vec<f32> = vec![2.0, 4.0];
        scale(&mut data, 0.5);
        assert_eq!(data, vec![1.0, 2.0]);
    }

    #[test]
    fn test_simd_vs_scalar_accumulate() {
        let mut dst_scalar: Vec<f32> = (0..1000).map(|x| x as f32).collect();
        let mut dst_simd: Vec<f32> = dst_scalar.clone();
        let src: Vec<f32> = (0..1000).map(|x| x as f32 * 0.1).collect();

        scalar::accumulate(&mut dst_scalar, &src);
        accumulate(&mut dst_simd, &src);

        for (s, d) in dst_scalar.iter().zip(dst_simd.iter()) {
            assert!((s - d).abs() < 1e-4, "scalar={}, simd={}", s, d);
        }
    }

    #[test]
    fn test_simd_vs_scalar_scale() {
        let mut data_scalar: Vec<f32> = (0..1000).map(|x| x as f32).collect();
        let mut data_simd: Vec<f32> = data_scalar.clone();

        scalar::scale(&mut data_scalar, 0.123);
        scale(&mut data_simd, 0.123);

        for (s, d) in data_scalar.iter().zip(data_simd.iter()) {
            assert!((s - d).abs() < 1e-4, "scalar={}, simd={}", s, d);
        }
    }

    #[test]
    fn test_parallel_sum_f32() {
        let values: Vec<f32> = (1..=100).map(|x| x as f32).collect();
        let result = parallel_sum_f32(&values);
        let expected: f32 = values.iter().sum();
        assert!((result - expected).abs() < f32::EPSILON);
    }

    // --- sigma_clipped_median_mad tests ---

    #[test]
    fn test_sigma_clipped_empty_input() {
        let mut values: Vec<f32> = vec![];
        let mut deviations = Vec::new();
        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
        assert_eq!(median, 0.0);
        assert_eq!(sigma, 0.0);
    }

    #[test]
    fn test_sigma_clipped_single_value() {
        let mut values = vec![5.0];
        let mut deviations = Vec::new();
        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
        assert_eq!(median, 5.0);
        assert_eq!(sigma, 0.0);
    }

    #[test]
    fn test_sigma_clipped_two_values() {
        let mut values = vec![2.0, 4.0];
        let mut deviations = Vec::new();
        let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
        // Median of [2, 4] = 3
        assert!((median - 3.0).abs() < 0.01);
    }

    #[test]
    fn test_sigma_clipped_uniform_values() {
        let mut values = vec![5.0; 100];
        let mut deviations = Vec::new();
        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
        assert_eq!(median, 5.0);
        assert_eq!(sigma, 0.0);
    }

    #[test]
    fn test_sigma_clipped_no_outliers() {
        // Normal-ish distribution with no extreme outliers
        let mut values: Vec<f32> = (0..100).map(|i| 50.0 + (i as f32 - 50.0) * 0.1).collect();
        let mut deviations = Vec::new();
        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
        // Median should be close to 50
        assert!((median - 50.0).abs() < 1.0);
        // Sigma should be small and non-zero
        assert!(sigma > 0.0);
        assert!(sigma < 10.0);
    }

    #[test]
    fn test_sigma_clipped_rejects_outliers() {
        // 97 values around 10, plus 3 extreme outliers
        let mut values: Vec<f32> = vec![10.0; 97];
        values.extend([1000.0, 2000.0, 3000.0]);
        let original_len = values.len();
        let mut deviations = Vec::new();

        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

        // After clipping, median should be ~10 (outliers rejected)
        assert!((median - 10.0).abs() < 0.1);
        // Sigma should be very small (uniform after clipping)
        assert!(sigma < 1.0);
        // Original slice reordered but length unchanged
        assert_eq!(values.len(), original_len);
    }

    #[test]
    fn test_sigma_clipped_negative_values() {
        let mut values = vec![-10.0, -5.0, 0.0, 5.0, 10.0];
        let mut deviations = Vec::new();
        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);
        assert!((median - 0.0).abs() < 0.1);
        assert!(sigma > 0.0);
    }

    #[test]
    fn test_sigma_clipped_mixed_outliers() {
        // Values centered at 100 with outliers on both sides
        let mut values: Vec<f32> = vec![100.0; 90];
        values.extend([0.0, 1.0, 2.0, 198.0, 199.0, 200.0]); // outliers
        values.extend([99.0, 100.0, 101.0, 102.0]); // normal values
        let mut deviations = Vec::new();

        let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

        // After clipping, median should be close to 100
        assert!((median - 100.0).abs() < 2.0);
    }

    #[test]
    fn test_sigma_clipped_zero_iterations() {
        let mut values = vec![1.0, 2.0, 3.0, 1000.0];
        let mut deviations = Vec::new();
        let (median, _sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 0);
        // With 0 iterations, should just return median/MAD of all values
        // Median of [1, 2, 3, 1000] = 2.5
        assert!((median - 2.5).abs() < 0.1);
    }

    #[test]
    fn test_sigma_clipped_one_iteration() {
        // 10 normal values plus one extreme outlier
        let mut values: Vec<f32> = vec![10.0; 10];
        values.push(10000.0);
        let mut deviations = Vec::new();

        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 1);

        // One iteration should reject the extreme outlier
        assert!((median - 10.0).abs() < 0.1);
        assert!(sigma < 1.0);
    }

    #[test]
    fn test_sigma_clipped_kappa_affects_clipping() {
        // Same data, different kappa values
        let base_values: Vec<f32> = {
            let mut v = vec![50.0; 90];
            v.extend([20.0, 25.0, 75.0, 80.0]); // moderate outliers
            v.extend([0.0, 100.0]); // extreme outliers
            v
        };

        let mut values_strict = base_values.clone();
        let mut values_loose = base_values.clone();
        let mut deviations = Vec::new();

        // Strict kappa (1.5) should clip more aggressively
        let (median_strict, sigma_strict) =
            sigma_clipped_median_mad(&mut values_strict, &mut deviations, 1.5, 3);

        // Loose kappa (5.0) should clip less
        let (median_loose, sigma_loose) =
            sigma_clipped_median_mad(&mut values_loose, &mut deviations, 5.0, 3);

        // Both should converge to similar median
        assert!((median_strict - 50.0).abs() < 5.0);
        assert!((median_loose - 50.0).abs() < 5.0);
        // Strict clipping should give smaller sigma
        assert!(sigma_strict <= sigma_loose);
    }

    #[test]
    fn test_sigma_clipped_deviations_buffer_reused() {
        let mut values1 = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let mut values2 = vec![10.0, 20.0, 30.0];
        let mut deviations = Vec::new();

        sigma_clipped_median_mad(&mut values1, &mut deviations, 3.0, 2);
        let cap_after_first = deviations.capacity();

        sigma_clipped_median_mad(&mut values2, &mut deviations, 3.0, 2);

        // Buffer should be reused (capacity not reduced)
        assert!(deviations.capacity() >= cap_after_first.min(values2.len()));
    }

    #[test]
    fn test_sigma_clipped_large_dataset() {
        // 10000 values with some outliers
        let mut values: Vec<f32> = (0..10000).map(|i| 100.0 + (i % 10) as f32).collect();
        // Add 100 outliers
        for i in 0..100 {
            values[i * 100] = 1000.0;
        }
        let mut deviations = Vec::new();

        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

        // Should converge to values around 100-109
        assert!((100.0..=110.0).contains(&median));
        assert!(sigma > 0.0 && sigma < 20.0);
    }

    #[test]
    fn test_sigma_clipped_all_same_then_one_different() {
        // Edge case: many identical values plus one outlier
        let mut values: Vec<f32> = vec![42.0; 999];
        values.push(9999.0);
        let mut deviations = Vec::new();

        let (median, sigma) = sigma_clipped_median_mad(&mut values, &mut deviations, 3.0, 3);

        assert!((median - 42.0).abs() < 0.01);
        // After clipping outlier, sigma should be 0 (all identical)
        assert!(sigma < 0.01);
    }
}
