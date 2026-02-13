//! Scalar (non-SIMD) implementations of sum operations.

/// Neumaier compensated addition.
#[inline]
fn neumaier_add(sum: &mut f32, c: &mut f32, v: f32) {
    let t = *sum + v;
    if sum.abs() >= v.abs() {
        *c += (*sum - t) + v;
    } else {
        *c += (v - t) + *sum;
    }
    *sum = t;
}

/// Sum f32 values using Neumaier compensated summation.
///
/// Achieves O(n·ε²) error — essentially independent of array length,
/// vs O(n·ε) for naive summation.
#[inline]
pub fn sum_f32(values: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    let mut c = 0.0f32;
    for &v in values {
        neumaier_add(&mut sum, &mut c, v);
    }
    sum + c
}

/// Weighted mean using Neumaier compensated summation (scalar).
#[inline]
pub fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    let mut c_sum = 0.0f32;
    let mut wsum = 0.0f32;
    let mut c_wsum = 0.0f32;

    for (&v, &w) in values.iter().zip(weights.iter()) {
        neumaier_add(&mut sum, &mut c_sum, v * w);
        neumaier_add(&mut wsum, &mut c_wsum, w);
    }

    let total = sum + c_sum;
    let total_w = wsum + c_wsum;

    if total_w > f32::EPSILON {
        total / total_w
    } else {
        0.0
    }
}
