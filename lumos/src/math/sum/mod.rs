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

/// Calculate sum of squared differences from mean using SIMD when available.
pub fn sum_squared_diff(values: &[f32], mean: f32) -> f32 {
    #[cfg(target_arch = "aarch64")]
    {
        if values.len() >= 4 {
            return unsafe { neon::sum_squared_diff(values, mean) };
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if values.len() >= 8 && common::cpu_features::has_avx2() {
            return unsafe { avx2::sum_squared_diff(values, mean) };
        }
        if values.len() >= 4 && common::cpu_features::has_sse4_1() {
            return unsafe { sse::sum_squared_diff(values, mean) };
        }
    }
    scalar::sum_squared_diff(values, mean)
}

/// Calculate the mean of f32 values using SIMD-accelerated sum.
pub fn mean_f32(values: &[f32]) -> f32 {
    debug_assert!(!values.is_empty());
    sum_f32(values) / values.len() as f32
}

/// Compute weighted mean of values with corresponding weights.
pub fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }

    let mut sum = 0.0f32;
    let mut weight_sum = 0.0f32;

    for (&v, &w) in values.iter().zip(weights.iter()) {
        sum += v * w;
        weight_sum += w;
    }

    if weight_sum > f32::EPSILON {
        sum / weight_sum
    } else {
        values.iter().sum::<f32>() / values.len() as f32
    }
}

/// Compute weighted mean from `(value, weight)` pairs.
pub fn weighted_mean_pairs_f32(pairs: &[(f32, f32)]) -> f32 {
    if pairs.is_empty() {
        return 0.0;
    }
    let mut sum = 0.0f32;
    let mut weight_sum = 0.0f32;
    for &(v, w) in pairs {
        sum += v * w;
        weight_sum += w;
    }
    if weight_sum > f32::EPSILON {
        sum / weight_sum
    } else {
        pairs.iter().map(|(v, _)| v).sum::<f32>() / pairs.len() as f32
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bench;
