//! Scalar (non-SIMD) implementations of sum operations.

/// Sum f32 values using scalar operations.
#[inline]
pub fn sum_f32(values: &[f32]) -> f32 {
    values.iter().sum()
}
