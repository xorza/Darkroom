//! Sum and accumulation operations with SIMD acceleration.

pub mod scalar;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod avx2;

#[cfg(target_arch = "x86_64")]
mod sse;

/// Sum f32 values using SIMD when available.
pub fn sum_f32(values: &[f32]) -> f32 {
    #[cfg(target_arch = "aarch64")]
    {
        if values.len() >= 4 {
            return unsafe { neon::sum_f32(values) };
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if values.len() >= 8 && common::cpu_features::has_avx2() {
            return unsafe { avx2::sum_f32(values) };
        }
        if values.len() >= 4 && common::cpu_features::has_sse4_1() {
            return unsafe { sse::sum_f32(values) };
        }
    }
    scalar::sum_f32(values)
}

/// Calculate the mean of f32 values using SIMD-accelerated sum.
pub fn mean_f32(values: &[f32]) -> f32 {
    debug_assert!(!values.is_empty());
    sum_f32(values) / values.len() as f32
}

/// Neumaier compensated addition: accumulates `v` into `sum` with compensation `c`.
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

/// Compute weighted mean of values with corresponding weights.
///
/// Uses Neumaier compensated summation for both numerator and denominator.
/// Returns 0.0 if the total weight is near zero.
pub fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    debug_assert_eq!(
        values.len(),
        weights.len(),
        "values and weights must have the same length"
    );

    if values.is_empty() {
        return 0.0;
    }

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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bench;
