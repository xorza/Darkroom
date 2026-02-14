//! Sum and accumulation operations with SIMD acceleration.

pub(super) mod scalar;

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

/// Compute weighted mean of values with corresponding weights using SIMD when available.
///
/// Uses Kahan compensated summation (SIMD) or Neumaier (scalar) for both numerator
/// and denominator. Returns 0.0 if the total weight is near zero.
pub fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    debug_assert_eq!(
        values.len(),
        weights.len(),
        "values and weights must have the same length"
    );

    if values.is_empty() {
        return 0.0;
    }

    #[cfg(target_arch = "aarch64")]
    {
        if values.len() >= 4 {
            return unsafe { neon::weighted_mean_f32(values, weights) };
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if values.len() >= 8 && common::cpu_features::has_avx2() {
            return unsafe { avx2::weighted_mean_f32(values, weights) };
        }
        if values.len() >= 4 && common::cpu_features::has_sse4_1() {
            return unsafe { sse::weighted_mean_f32(values, weights) };
        }
    }
    scalar::weighted_mean_f32(values, weights)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bench;
