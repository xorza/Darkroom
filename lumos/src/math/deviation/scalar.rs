//! Scalar (non-SIMD) implementations of deviation operations.

/// Compute absolute deviations from median in-place.
///
/// Replaces each value with |value - median|.
#[inline]
pub fn abs_deviation_inplace(values: &mut [f32], median: f32) {
    for v in values.iter_mut() {
        *v = (*v - median).abs();
    }
}
