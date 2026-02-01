//! Scalar (non-SIMD) implementations of math operations.

/// Sum f32 values using scalar operations.
#[inline]
pub fn sum_f32(values: &[f32]) -> f32 {
    values.iter().sum()
}

/// Calculate sum of squared differences from mean using scalar operations.
#[inline]
pub fn sum_squared_diff(values: &[f32], mean: f32) -> f32 {
    values.iter().map(|v| (v - mean).powi(2)).sum()
}

/// Accumulate src into dst (dst[i] += src[i]).
#[inline]
pub fn accumulate(dst: &mut [f32], src: &[f32]) {
    debug_assert_eq!(dst.len(), src.len());
    for (d, &s) in dst.iter_mut().zip(src.iter()) {
        *d += s;
    }
}

/// Scale values in-place (data[i] *= scale).
#[inline]
pub fn scale(data: &mut [f32], scale: f32) {
    for d in data.iter_mut() {
        *d *= scale;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // sum_f32 tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_sum_empty() {
        assert_eq!(sum_f32(&[]), 0.0);
    }

    #[test]
    fn test_sum_single() {
        assert!((sum_f32(&[42.0]) - 42.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_multiple() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert!((sum_f32(&values) - 15.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_negative() {
        let values = [-1.0, -2.0, 3.0];
        assert!((sum_f32(&values) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_large_array() {
        let values: Vec<f32> = (1..=1000).map(|x| x as f32).collect();
        let expected = 1000.0 * 1001.0 / 2.0; // n(n+1)/2
        assert!((sum_f32(&values) - expected).abs() < 1.0);
    }

    #[test]
    fn test_sum_all_same() {
        let values = [3.5; 100];
        assert!((sum_f32(&values) - 350.0).abs() < 0.01);
    }

    #[test]
    fn test_sum_alternating_signs() {
        let values: Vec<f32> = (0..100)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        assert!((sum_f32(&values) - 0.0).abs() < f32::EPSILON);
    }

    // -------------------------------------------------------------------------
    // sum_squared_diff tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_sum_squared_diff_empty() {
        assert_eq!(sum_squared_diff(&[], 0.0), 0.0);
    }

    #[test]
    fn test_sum_squared_diff_single_at_mean() {
        assert!((sum_squared_diff(&[5.0], 5.0) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_squared_diff_single_off_mean() {
        // (3 - 5)^2 = 4
        assert!((sum_squared_diff(&[3.0], 5.0) - 4.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_squared_diff_symmetric() {
        // Values symmetric around mean: deviations are equal
        let values = [1.0, 3.0, 5.0, 7.0, 9.0];
        let mean = 5.0;
        // (1-5)^2 + (3-5)^2 + (5-5)^2 + (7-5)^2 + (9-5)^2 = 16 + 4 + 0 + 4 + 16 = 40
        assert!((sum_squared_diff(&values, mean) - 40.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_squared_diff_negative_values() {
        let values = [-2.0, 0.0, 2.0];
        let mean = 0.0;
        // (-2)^2 + 0^2 + 2^2 = 4 + 0 + 4 = 8
        assert!((sum_squared_diff(&values, mean) - 8.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_squared_diff_all_same() {
        let values = [7.0; 50];
        assert!((sum_squared_diff(&values, 7.0) - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_sum_squared_diff_large_array() {
        let values: Vec<f32> = (0..1000).map(|x| x as f32).collect();
        let mean = 499.5; // mean of 0..999
        let result = sum_squared_diff(&values, mean);
        // Variance formula: sum of (x - mean)^2
        assert!(result > 0.0);
        assert!(result < 100_000_000.0); // Sanity check
    }

    // -------------------------------------------------------------------------
    // accumulate tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_accumulate_empty() {
        let mut dst: Vec<f32> = vec![];
        let src: Vec<f32> = vec![];
        accumulate(&mut dst, &src);
        assert!(dst.is_empty());
    }

    #[test]
    fn test_accumulate_single() {
        let mut dst = [10.0];
        let src = [5.0];
        accumulate(&mut dst, &src);
        assert!((dst[0] - 15.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_accumulate_multiple() {
        let mut dst = [1.0, 2.0, 3.0];
        let src = [0.5, 0.5, 0.5];
        accumulate(&mut dst, &src);
        assert_eq!(dst, [1.5, 2.5, 3.5]);
    }

    #[test]
    fn test_accumulate_negative() {
        let mut dst = [10.0, 20.0];
        let src = [-5.0, -10.0];
        accumulate(&mut dst, &src);
        assert_eq!(dst, [5.0, 10.0]);
    }

    #[test]
    fn test_accumulate_zeros() {
        let mut dst = [1.0, 2.0, 3.0];
        let src = [0.0, 0.0, 0.0];
        accumulate(&mut dst, &src);
        assert_eq!(dst, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_accumulate_large_array() {
        let mut dst: Vec<f32> = vec![1.0; 1000];
        let src: Vec<f32> = vec![0.001; 1000];
        accumulate(&mut dst, &src);
        for &v in &dst {
            assert!((v - 1.001).abs() < 1e-6);
        }
    }

    // -------------------------------------------------------------------------
    // scale tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_scale_empty() {
        let mut data: Vec<f32> = vec![];
        scale(&mut data, 2.0);
        assert!(data.is_empty());
    }

    #[test]
    fn test_scale_single() {
        let mut data = [5.0];
        scale(&mut data, 3.0);
        assert!((data[0] - 15.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_scale_multiple() {
        let mut data = [1.0, 2.0, 3.0, 4.0];
        scale(&mut data, 2.0);
        assert_eq!(data, [2.0, 4.0, 6.0, 8.0]);
    }

    #[test]
    fn test_scale_by_zero() {
        let mut data = [1.0, 2.0, 3.0];
        scale(&mut data, 0.0);
        assert_eq!(data, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_scale_by_one() {
        let mut data = [1.0, 2.0, 3.0];
        scale(&mut data, 1.0);
        assert_eq!(data, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_scale_by_negative() {
        let mut data = [1.0, -2.0, 3.0];
        scale(&mut data, -1.0);
        assert_eq!(data, [-1.0, 2.0, -3.0]);
    }

    #[test]
    fn test_scale_fractional() {
        let mut data = [10.0, 20.0, 30.0];
        scale(&mut data, 0.5);
        assert_eq!(data, [5.0, 10.0, 15.0]);
    }

    #[test]
    fn test_scale_large_array() {
        let mut data: Vec<f32> = (1..=1000).map(|x| x as f32).collect();
        scale(&mut data, 0.1);
        for (i, &v) in data.iter().enumerate() {
            let expected = (i + 1) as f32 * 0.1;
            assert!((v - expected).abs() < 1e-5);
        }
    }
}
