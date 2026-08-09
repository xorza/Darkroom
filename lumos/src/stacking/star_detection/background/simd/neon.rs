//! NEON cubic-spline segment interpolation.

use std::arch::aarch64::*;

use crate::stacking::star_detection::background::simd::{SegmentRamp, SplineSegment};

/// Evaluate cubic spline for 4 values using NEON.
pub(super) unsafe fn interpolate_segment_cubic_neon(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg: SplineSegment,
    noise: SplineSegment,
    ramp: SegmentRamp,
) {
    let len = bg_out.len();

    unsafe {
        let bg_f0_v = vdupq_n_f32(bg.f0);
        let bg_f1_v = vdupq_n_f32(bg.f1);
        let bg_a_v = vdupq_n_f32(bg.a);
        let bg_b_v = vdupq_n_f32(bg.b);
        let noise_f0_v = vdupq_n_f32(noise.f0);
        let noise_f1_v = vdupq_n_f32(noise.f1);
        let noise_a_v = vdupq_n_f32(noise.a);
        let noise_b_v = vdupq_n_f32(noise.b);
        let one = vdupq_n_f32(1.0);
        let two = vdupq_n_f32(2.0);
        let zero = vdupq_n_f32(0.0);
        let step4 = vdupq_n_f32(ramp.step * 4.0);

        let offsets: [f32; 4] = [0.0, ramp.step, 2.0 * ramp.step, 3.0 * ramp.step];
        let mut t_v = vaddq_f32(vdupq_n_f32(ramp.start), vld1q_f32(offsets.as_ptr()));

        let mut i = 0;
        while i + 4 <= len {
            let t = vminq_f32(vmaxq_f32(t_v, zero), one);
            let ct = vsubq_f32(one, t);

            // cubic = (2-t)*a + (1+t)*b
            let two_minus_t = vsubq_f32(two, t);
            let one_plus_t = vaddq_f32(one, t);
            let cubic = vfmaq_f32(vmulq_f32(two_minus_t, bg_a_v), one_plus_t, bg_b_v);
            let t_ct = vmulq_f32(t, ct);
            // result = ct*f0 + t*f1 - t*ct*cubic
            let linear = vfmaq_f32(vmulq_f32(ct, bg_f0_v), t, bg_f1_v);
            let result = vsubq_f32(linear, vmulq_f32(t_ct, cubic));
            vst1q_f32(bg_out.as_mut_ptr().add(i), result);

            let n_cubic = vfmaq_f32(vmulq_f32(two_minus_t, noise_a_v), one_plus_t, noise_b_v);
            let n_linear = vfmaq_f32(vmulq_f32(ct, noise_f0_v), t, noise_f1_v);
            let n_result = vsubq_f32(n_linear, vmulq_f32(t_ct, n_cubic));
            vst1q_f32(noise_out.as_mut_ptr().add(i), n_result);

            t_v = vaddq_f32(t_v, step4);
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
