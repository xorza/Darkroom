//! Scalar (non-SIMD) implementations of sum operations.

/// Sum f32 values using Neumaier compensated summation.
///
/// Achieves O(n·ε²) error — essentially independent of array length,
/// vs O(n·ε) for naive summation.
#[inline]
pub fn sum_f32(values: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    let mut c = 0.0f32;
    for &v in values {
        let t = sum + v;
        if sum.abs() >= v.abs() {
            c += (sum - t) + v;
        } else {
            c += (v - t) + sum;
        }
        sum = t;
    }
    sum + c
}
