//! Scalar (non-SIMD) implementations of sum operations.

/// Neumaier compensated addition. Single source of truth for the compensation step — the SIMD
/// backends call this for their lane-reduction and scalar-remainder tails.
#[inline]
pub(crate) fn neumaier_add(sum: &mut f32, c: &mut f32, v: f32) {
    let t = *sum + v;
    if sum.abs() >= v.abs() {
        *c += (*sum - t) + v;
    } else {
        *c += (v - t) + *sum;
    }
    *sum = t;
}

/// Sum f32 values using a wider accumulator.
#[inline]
pub(crate) fn sum_f32(values: &[f32]) -> f32 {
    values.iter().map(|&value| f64::from(value)).sum::<f64>() as f32
}

/// Weighted mean using wider products and accumulators.
#[inline]
pub(crate) fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    let mut sum = 0.0f64;
    let mut weight_sum = 0.0f64;

    for (&value, &weight) in values.iter().zip(weights) {
        sum += f64::from(value) * f64::from(weight);
        weight_sum += f64::from(weight);
    }

    if weight_sum > f64::from(f32::EPSILON) {
        (sum / weight_sum) as f32
    } else {
        0.0
    }
}
