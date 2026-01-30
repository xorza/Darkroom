//! Math utilities with SIMD acceleration.
//!
//! Provides optimized math operations with platform-specific SIMD for ARM NEON (aarch64) and x86 SSE4.

pub mod scalar;

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

#[cfg(feature = "bench")]
pub mod bench;

/// Sum f32 values using SIMD when available.
#[cfg(target_arch = "aarch64")]
pub fn sum_f32(values: &[f32]) -> f32 {
    if values.len() < 4 {
        return scalar::sum_f32(values);
    }
    unsafe { neon::sum_f32(values) }
}

/// Sum f32 values using SIMD when available.
#[cfg(target_arch = "x86_64")]
pub fn sum_f32(values: &[f32]) -> f32 {
    if values.len() < 4 || !crate::common::cpu_features::has_sse4_1() {
        return scalar::sum_f32(values);
    }
    unsafe { sse::sum_f32(values) }
}

/// Fallback for other architectures.
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub fn sum_f32(values: &[f32]) -> f32 {
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

/// Calculate sum of squared differences from mean using SIMD.
#[cfg(target_arch = "aarch64")]
pub fn sum_squared_diff(values: &[f32], mean: f32) -> f32 {
    if values.len() < 4 {
        return scalar::sum_squared_diff(values, mean);
    }
    unsafe { neon::sum_squared_diff(values, mean) }
}

/// Calculate sum of squared differences from mean using SIMD.
#[cfg(target_arch = "x86_64")]
pub fn sum_squared_diff(values: &[f32], mean: f32) -> f32 {
    if values.len() < 4 || !crate::common::cpu_features::has_sse4_1() {
        return scalar::sum_squared_diff(values, mean);
    }
    unsafe { sse::sum_squared_diff(values, mean) }
}

/// Fallback for other architectures.
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
pub fn sum_squared_diff(values: &[f32], mean: f32) -> f32 {
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

/// Chunk size for parallel operations to avoid false cache sharing.
/// 16KB worth of f32 values (4096 elements) ensures each chunk spans
/// multiple cache lines and reduces false sharing at chunk boundaries.
const PARALLEL_CHUNK_SIZE: usize = 4096;

/// Compute sum of f32 slice in parallel using rayon.
/// Each thread computes a partial sum, then results are combined.
pub fn parallel_sum_f32(values: &[f32]) -> f32 {
    use rayon::prelude::*;
    values
        .par_chunks(PARALLEL_CHUNK_SIZE)
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
}
