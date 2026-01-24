//! CPU dispatch for median stacking with SIMD support.

#[cfg(not(target_arch = "aarch64"))]
use super::scalar;

/// Calculate median of values, dispatching to best available implementation.
///
/// Note: This function clones the input because the SIMD implementations
/// use in-place sorting networks for efficiency.
#[inline]
pub(super) fn median_f32(values: &[f32]) -> f32 {
    // Clone to allow mutation - sorting networks work in-place
    let mut buf = values.to_vec();
    median_f32_mut(&mut buf)
}

/// Calculate median with mutable buffer (avoids allocation if caller can provide one).
#[inline]
fn median_f32_mut(values: &mut [f32]) -> f32 {
    #[cfg(target_arch = "aarch64")]
    {
        // NEON is always available on aarch64
        unsafe { super::neon::median_f32(values) }
    }

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("sse2") {
            unsafe { super::sse::median_f32(values) }
        } else {
            scalar::median_f32(values)
        }
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        scalar::median_f32(values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_median_odd() {
        let values = vec![5.0, 1.0, 3.0, 2.0, 4.0];
        let result = median_f32(&values);
        assert!((result - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_even() {
        let values = vec![4.0, 1.0, 3.0, 2.0];
        let result = median_f32(&values);
        assert!((result - 2.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_single() {
        let values = vec![42.0];
        let result = median_f32(&values);
        assert!((result - 42.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_two() {
        let values = vec![1.0, 3.0];
        let result = median_f32(&values);
        assert!((result - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_large() {
        let values: Vec<f32> = (1..=100).map(|x| x as f32).rev().collect();
        let result = median_f32(&values);
        assert!((result - 50.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_simd_vs_scalar_consistency() {
        use super::super::scalar;

        // Test various sizes to exercise different code paths
        for size in [3, 4, 5, 6, 7, 8, 10, 15, 16, 20, 50] {
            let values: Vec<f32> = (1..=size).map(|x| x as f32).rev().collect();

            let simd_result = median_f32(&values);
            let scalar_result = scalar::median_f32(&values);

            assert!(
                (simd_result - scalar_result).abs() < f32::EPSILON,
                "Mismatch for size {}: simd={}, scalar={}",
                size,
                simd_result,
                scalar_result
            );
        }
    }
}
