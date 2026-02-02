//! AVX2 SIMD implementations of deviation operations (x86_64).

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// Compute absolute deviations from median in-place using AVX2.
///
/// Replaces each value with |value - median|.
///
/// # Safety
/// Caller must ensure AVX2 is available.
#[target_feature(enable = "avx2")]
pub unsafe fn abs_deviation_inplace(values: &mut [f32], median: f32) {
    unsafe {
        let median_vec = _mm256_set1_ps(median);
        // Sign bit mask for absolute value via andnot
        let sign_mask = _mm256_set1_ps(-0.0f32);

        let len = values.len();
        let ptr = values.as_mut_ptr();

        // Process 16 elements at a time (2 AVX2 vectors)
        let chunks16 = len / 16;
        for i in 0..chunks16 {
            let base = ptr.add(i * 16);

            let v0 = _mm256_loadu_ps(base);
            let v1 = _mm256_loadu_ps(base.add(8));

            let diff0 = _mm256_sub_ps(v0, median_vec);
            let diff1 = _mm256_sub_ps(v1, median_vec);

            let abs0 = _mm256_andnot_ps(sign_mask, diff0);
            let abs1 = _mm256_andnot_ps(sign_mask, diff1);

            _mm256_storeu_ps(base, abs0);
            _mm256_storeu_ps(base.add(8), abs1);
        }

        // Process remaining 8-element chunk
        let processed = chunks16 * 16;
        if len - processed >= 8 {
            let base = ptr.add(processed);
            let v = _mm256_loadu_ps(base);
            let diff = _mm256_sub_ps(v, median_vec);
            let abs = _mm256_andnot_ps(sign_mask, diff);
            _mm256_storeu_ps(base, abs);
        }

        // Scalar remainder (0-7 elements)
        let simd_processed = (len / 8) * 8;
        for i in simd_processed..len {
            *values.get_unchecked_mut(i) = (*values.get_unchecked(i) - median).abs();
        }
    }
}
