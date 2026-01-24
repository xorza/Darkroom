//! Scalar (non-SIMD) implementation of median calculation.

use crate::math;

/// Calculate the median of values using scalar operations.
#[inline]
#[cfg_attr(target_arch = "aarch64", allow(dead_code))]
pub(super) fn median_f32(values: &[f32]) -> f32 {
    math::median_f32(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_median_odd() {
        let values = vec![1.0, 3.0, 2.0, 5.0, 4.0];
        assert!((median_f32(&values) - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_median_even() {
        let values = vec![1.0, 2.0, 3.0, 4.0];
        assert!((median_f32(&values) - 2.5).abs() < f32::EPSILON);
    }
}
