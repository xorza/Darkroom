//! Scalar (non-SIMD) implementations of sum operations.

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
