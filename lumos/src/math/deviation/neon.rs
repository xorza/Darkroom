//! NEON SIMD implementations of deviation operations (aarch64).

use std::arch::aarch64::*;

/// Compute absolute deviations from median in-place using NEON.
///
/// Replaces each value with |value - median|.
///
/// # Safety
/// Caller must ensure NEON is available (always true on aarch64).
#[target_feature(enable = "neon")]
pub unsafe fn abs_deviation_inplace(values: &mut [f32], median: f32) {
    let median_vec = vdupq_n_f32(median);

    let len = values.len();
    let ptr = values.as_mut_ptr();

    // Process 8 elements at a time for better throughput
    let chunks8 = len / 8;
    for i in 0..chunks8 {
        let base = ptr.add(i * 8);

        let v0 = vld1q_f32(base);
        let v1 = vld1q_f32(base.add(4));

        let diff0 = vsubq_f32(v0, median_vec);
        let diff1 = vsubq_f32(v1, median_vec);

        let abs0 = vabsq_f32(diff0);
        let abs1 = vabsq_f32(diff1);

        vst1q_f32(base, abs0);
        vst1q_f32(base.add(4), abs1);
    }

    // Process remaining 4-element chunk
    let processed = chunks8 * 8;
    if len - processed >= 4 {
        let base = ptr.add(processed);
        let v = vld1q_f32(base);
        let diff = vsubq_f32(v, median_vec);
        let abs = vabsq_f32(diff);
        vst1q_f32(base, abs);
    }

    // Scalar remainder
    let simd_processed = (len / 4) * 4;
    for i in simd_processed..len {
        *values.get_unchecked_mut(i) = (*values.get_unchecked(i) - median).abs();
    }
}
