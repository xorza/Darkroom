//! AVX2 SIMD implementations of sum operations (x86_64).

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// Horizontal sum of a 256-bit f32 vector (8 elements).
#[inline]
#[target_feature(enable = "avx2")]
unsafe fn horizontal_sum_256(v: __m256) -> f32 {
    // Extract high 128 bits and add to low 128 bits
    let high = _mm256_extractf128_ps(v, 1);
    let low = _mm256_castps256_ps128(v);
    let sum128 = _mm_add_ps(low, high);

    // Standard 128-bit horizontal sum
    let shuf = _mm_movehdup_ps(sum128);
    let sums = _mm_add_ps(sum128, shuf);
    let shuf = _mm_movehl_ps(sums, sums);
    let sums = _mm_add_ss(sums, shuf);
    _mm_cvtss_f32(sums)
}

/// Sum f32 values using AVX2 SIMD.
///
/// # Safety
/// Caller must ensure AVX2 is available.
#[target_feature(enable = "avx2")]
pub unsafe fn sum_f32(values: &[f32]) -> f32 {
    unsafe {
        let mut sum_vec = _mm256_setzero_ps();
        let chunks = values.chunks_exact(8);
        let remainder = chunks.remainder();

        for chunk in chunks {
            let v = _mm256_loadu_ps(chunk.as_ptr());
            sum_vec = _mm256_add_ps(sum_vec, v);
        }

        horizontal_sum_256(sum_vec) + remainder.iter().sum::<f32>()
    }
}
