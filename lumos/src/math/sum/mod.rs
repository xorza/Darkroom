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

/// Accumulate src into dst (dst[i] += src[i]).
#[inline]
pub fn accumulate(dst: &mut [f32], src: &[f32]) {
    scalar::accumulate(dst, src)
}

/// Scale values in-place (data[i] *= scale).
#[inline]
pub fn scale(data: &mut [f32], scale_val: f32) {
    scalar::scale(data, scale_val)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bench;
