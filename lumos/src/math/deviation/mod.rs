//! Deviation computation with SIMD acceleration.

pub mod scalar;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod avx2;

#[cfg(target_arch = "x86_64")]
mod sse;

/// Compute absolute deviations from median in-place using SIMD when available.
///
/// Replaces each value with |value - median|.
#[inline]
pub fn abs_deviation_inplace(values: &mut [f32], median: f32) {
    #[cfg(target_arch = "aarch64")]
    {
        if values.len() >= 4 {
            unsafe { neon::abs_deviation_inplace(values, median) };
            return;
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if values.len() >= 8 && common::cpu_features::has_avx2() {
            unsafe { avx2::abs_deviation_inplace(values, median) };
            return;
        }
        if values.len() >= 4 && common::cpu_features::has_sse4_1() {
            unsafe { sse::abs_deviation_inplace(values, median) };
            return;
        }
    }
    scalar::abs_deviation_inplace(values, median);
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bench;
