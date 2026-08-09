//! AVX2+FMA cubic-spline segment interpolation.

use std::arch::x86_64::*;

use crate::stacking::star_detection::background::simd::{SegmentRamp, SplineSegment};

/// Evaluate cubic spline for 8 values using AVX2+FMA.
#[target_feature(enable = "avx2,fma")]
pub(super) unsafe fn interpolate_segment_cubic_avx2(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg: SplineSegment,
    noise: SplineSegment,
    ramp: SegmentRamp,
) {
    let len = bg_out.len();

    unsafe {
        let bg_f0_v = _mm256_set1_ps(bg.f0);
        let bg_f1_v = _mm256_set1_ps(bg.f1);
        let bg_a_v = _mm256_set1_ps(bg.a);
        let bg_b_v = _mm256_set1_ps(bg.b);
        let noise_f0_v = _mm256_set1_ps(noise.f0);
        let noise_f1_v = _mm256_set1_ps(noise.f1);
        let noise_a_v = _mm256_set1_ps(noise.a);
        let noise_b_v = _mm256_set1_ps(noise.b);
        let one = _mm256_set1_ps(1.0);
        let two = _mm256_set1_ps(2.0);
        let zero = _mm256_setzero_ps();
        let step8 = _mm256_set1_ps(ramp.step * 8.0);

        let mut t_v = _mm256_set_ps(
            ramp.start + 7.0 * ramp.step,
            ramp.start + 6.0 * ramp.step,
            ramp.start + 5.0 * ramp.step,
            ramp.start + 4.0 * ramp.step,
            ramp.start + 3.0 * ramp.step,
            ramp.start + 2.0 * ramp.step,
            ramp.start + ramp.step,
            ramp.start,
        );

        let mut i = 0;
        while i + 8 <= len {
            let t = _mm256_min_ps(_mm256_max_ps(t_v, zero), one);
            let ct = _mm256_sub_ps(one, t);

            // cubic = (2-t)*a + (1+t)*b
            let two_minus_t = _mm256_sub_ps(two, t);
            let one_plus_t = _mm256_add_ps(one, t);
            let cubic = _mm256_fmadd_ps(one_plus_t, bg_b_v, _mm256_mul_ps(two_minus_t, bg_a_v));
            // result = ct*f0 + t*f1 - t*ct*cubic
            let t_ct = _mm256_mul_ps(t, ct);
            let linear = _mm256_fmadd_ps(t, bg_f1_v, _mm256_mul_ps(ct, bg_f0_v));
            let result = _mm256_fnmadd_ps(t_ct, cubic, linear);
            _mm256_storeu_ps(bg_out.as_mut_ptr().add(i), result);

            // Same for noise
            let n_cubic =
                _mm256_fmadd_ps(one_plus_t, noise_b_v, _mm256_mul_ps(two_minus_t, noise_a_v));
            let n_linear = _mm256_fmadd_ps(t, noise_f1_v, _mm256_mul_ps(ct, noise_f0_v));
            let n_result = _mm256_fnmadd_ps(t_ct, n_cubic, n_linear);
            _mm256_storeu_ps(noise_out.as_mut_ptr().add(i), n_result);

            t_v = _mm256_add_ps(t_v, step8);
            i += 8;
        }

        // SSE remainder (4 at a time)
        if i + 4 <= len {
            let bg_f0_4 = _mm_set1_ps(bg.f0);
            let bg_f1_4 = _mm_set1_ps(bg.f1);
            let bg_a_4 = _mm_set1_ps(bg.a);
            let bg_b_4 = _mm_set1_ps(bg.b);
            let noise_f0_4 = _mm_set1_ps(noise.f0);
            let noise_f1_4 = _mm_set1_ps(noise.f1);
            let noise_a_4 = _mm_set1_ps(noise.a);
            let noise_b_4 = _mm_set1_ps(noise.b);
            let one4 = _mm_set1_ps(1.0);
            let two4 = _mm_set1_ps(2.0);
            let zero4 = _mm_setzero_ps();

            let cur = ramp.start + i as f32 * ramp.step;
            let t4 = _mm_min_ps(
                _mm_max_ps(
                    _mm_set_ps(
                        cur + 3.0 * ramp.step,
                        cur + 2.0 * ramp.step,
                        cur + ramp.step,
                        cur,
                    ),
                    zero4,
                ),
                one4,
            );
            let ct4 = _mm_sub_ps(one4, t4);
            let two_minus_t4 = _mm_sub_ps(two4, t4);
            let one_plus_t4 = _mm_add_ps(one4, t4);

            let cubic4 = _mm_fmadd_ps(one_plus_t4, bg_b_4, _mm_mul_ps(two_minus_t4, bg_a_4));
            let t_ct4 = _mm_mul_ps(t4, ct4);
            let lin4 = _mm_fmadd_ps(t4, bg_f1_4, _mm_mul_ps(ct4, bg_f0_4));
            let r4 = _mm_fnmadd_ps(t_ct4, cubic4, lin4);
            _mm_storeu_ps(bg_out.as_mut_ptr().add(i), r4);

            let nc4 = _mm_fmadd_ps(one_plus_t4, noise_b_4, _mm_mul_ps(two_minus_t4, noise_a_4));
            let nlin4 = _mm_fmadd_ps(t4, noise_f1_4, _mm_mul_ps(ct4, noise_f0_4));
            let nr4 = _mm_fnmadd_ps(t_ct4, nc4, nlin4);
            _mm_storeu_ps(noise_out.as_mut_ptr().add(i), nr4);
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
