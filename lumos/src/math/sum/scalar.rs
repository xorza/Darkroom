//! Scalar (non-SIMD) implementations of sum operations.

/// Sum f32 values using scalar operations.
#[inline]
pub fn sum_f32(values: &[f32]) -> f32 {
    values.iter().sum()
}

/// Calculate sum of squared differences from mean using scalar operations.
#[inline]
pub fn sum_squared_diff(values: &[f32], mean: f32) -> f32 {
    values
        .iter()
        .map(|v| {
            let diff = v - mean;
            diff * diff
        })
        .sum()
}
