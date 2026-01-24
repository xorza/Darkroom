//! SSE SIMD implementation for median stacking (x86_64).
//!
//! Note: Median calculation doesn't benefit much from SIMD since it requires sorting.
//! This module is a placeholder for future optimizations like vectorized partial sorts.

use super::scalar;

/// Calculate the median using SSE intrinsics.
/// Currently falls back to scalar implementation.
#[allow(dead_code)]
#[inline]
pub fn median_f32(values: &[f32]) -> f32 {
    // Median requires sorting which doesn't vectorize well.
    // Future: could use SIMD for partial sort networks on small arrays.
    scalar::median_f32(values)
}
