//! SSE4.1 cubic-spline segment interpolation.

use std::arch::x86_64::*;

use crate::stacking::star_detection::background::simd::{SegmentRamp, SplineSegment};

/// Evaluate cubic spline for 4 values using SSE4.1.
#[target_feature(enable = "sse4.1")]
pub(super) unsafe fn interpolate_segment_cubic_sse(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg: SplineSegment,
    noise: SplineSegment,
    ramp: SegmentRamp,
) {
    let len = bg_out.len();

    unsafe {
        let bg_f0_v = _mm_set1_ps(bg.f0);
        let bg_f1_v = _mm_set1_ps(bg.f1);
        let bg_a_v = _mm_set1_ps(bg.a);
        let bg_b_v = _mm_set1_ps(bg.b);
        let noise_f0_v = _mm_set1_ps(noise.f0);
        let noise_f1_v = _mm_set1_ps(noise.f1);
        let noise_a_v = _mm_set1_ps(noise.a);
        let noise_b_v = _mm_set1_ps(noise.b);
        let one = _mm_set1_ps(1.0);
        let two = _mm_set1_ps(2.0);
        let zero = _mm_setzero_ps();
        let step4 = _mm_set1_ps(ramp.step * 4.0);

        let mut t_v = _mm_set_ps(
            ramp.start + 3.0 * ramp.step,
            ramp.start + 2.0 * ramp.step,
            ramp.start + ramp.step,
            ramp.start,
        );

        let mut i = 0;
        while i + 4 <= len {
            let t = _mm_min_ps(_mm_max_ps(t_v, zero), one);
            let ct = _mm_sub_ps(one, t);

            // cubic = (2-t)*a + (1+t)*b (no FMA on SSE4.1)
            let two_minus_t = _mm_sub_ps(two, t);
            let one_plus_t = _mm_add_ps(one, t);
            let cubic = _mm_add_ps(
                _mm_mul_ps(two_minus_t, bg_a_v),
                _mm_mul_ps(one_plus_t, bg_b_v),
            );
            let t_ct = _mm_mul_ps(t, ct);
            // result = ct*f0 + t*f1 - t*ct*cubic
            let linear = _mm_add_ps(_mm_mul_ps(ct, bg_f0_v), _mm_mul_ps(t, bg_f1_v));
            let result = _mm_sub_ps(linear, _mm_mul_ps(t_ct, cubic));
            _mm_storeu_ps(bg_out.as_mut_ptr().add(i), result);

            let n_cubic = _mm_add_ps(
                _mm_mul_ps(two_minus_t, noise_a_v),
                _mm_mul_ps(one_plus_t, noise_b_v),
            );
            let n_linear = _mm_add_ps(_mm_mul_ps(ct, noise_f0_v), _mm_mul_ps(t, noise_f1_v));
            let n_result = _mm_sub_ps(n_linear, _mm_mul_ps(t_ct, n_cubic));
            _mm_storeu_ps(noise_out.as_mut_ptr().add(i), n_result);

            t_v = _mm_add_ps(t_v, step4);
            i += 4;
        }

        // Scalar remainder
        while i < len {
            let t = ramp.t_at(i);
            bg_out[i] = bg.eval(t);
            noise_out[i] = noise.eval(t);
            i += 1;
        }
    }
}
