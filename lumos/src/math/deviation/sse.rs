//! SSE SIMD implementations of deviation operations (x86_64).

use std::arch::x86_64::*;

/// Compute absolute deviations from median in-place using SSE.
///
/// Replaces each value with |value - median|.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub unsafe fn abs_deviation_inplace(values: &mut [f32], median: f32) {
    unsafe {
        let median_vec = _mm_set1_ps(median);
        // Sign bit mask for absolute value
        let sign_mask = _mm_set1_ps(-0.0f32);

        let len = values.len();
        let ptr = values.as_mut_ptr();

        // Process 8 elements at a time for better throughput
        let chunks8 = len / 8;
        for i in 0..chunks8 {
            let base = ptr.add(i * 8);

            let v0 = _mm_loadu_ps(base);
            let v1 = _mm_loadu_ps(base.add(4));

            let diff0 = _mm_sub_ps(v0, median_vec);
            let diff1 = _mm_sub_ps(v1, median_vec);

            let abs0 = _mm_andnot_ps(sign_mask, diff0);
            let abs1 = _mm_andnot_ps(sign_mask, diff1);

            _mm_storeu_ps(base, abs0);
            _mm_storeu_ps(base.add(4), abs1);
        }

        // Process remaining 4-element chunk
        let processed = chunks8 * 8;
        if len - processed >= 4 {
            let base = ptr.add(processed);
            let v = _mm_loadu_ps(base);
            let diff = _mm_sub_ps(v, median_vec);
            let abs = _mm_andnot_ps(sign_mask, diff);
            _mm_storeu_ps(base, abs);
        }

        // Scalar remainder
        let simd_processed = (len / 4) * 4;
        for i in simd_processed..len {
            *values.get_unchecked_mut(i) = (*values.get_unchecked(i) - median).abs();
        }
    }
}
