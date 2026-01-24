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
