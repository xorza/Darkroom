//! CPU dispatch for median stacking.

use super::scalar;

/// Calculate median of values, dispatching to best available implementation.
#[inline]
pub fn median_f32(values: &[f32]) -> f32 {
    // Median doesn't benefit from SIMD (requires sorting), use scalar
    scalar::median_f32(values)
}
