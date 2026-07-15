//! NEON SIMD implementations of sum operations (aarch64).

use std::arch::aarch64::*;

use crate::math::sum::scalar::neumaier_add;

/// Sum f32 values using NEON SIMD with Kahan compensated summation.
///
/// # Safety
/// Caller must ensure NEON is available (always true on aarch64).
#[target_feature(enable = "neon")]
pub(crate) unsafe fn sum_f32(values: &[f32]) -> f32 {
    unsafe {
        let mut sum_vec = vdupq_n_f32(0.0);
        let mut c_vec = vdupq_n_f32(0.0);

        let chunks = values.chunks_exact(4);
        let remainder = chunks.remainder();

        for chunk in chunks {
            let v = vld1q_f32(chunk.as_ptr());
            let y = vsubq_f32(v, c_vec);
            let t = vaddq_f32(sum_vec, y);
            c_vec = vsubq_f32(vsubq_f32(t, sum_vec), y);
            sum_vec = t;
        }

        let (mut s, mut c) = reduce_kahan_neon(sum_vec, c_vec);

        for &v in remainder {
            neumaier_add(&mut s, &mut c, v);
        }

        s + c
    }
}

/// Kahan horizontal reduction of 4 sum lanes + 4 compensation lanes.
#[inline]
#[target_feature(enable = "neon")]
unsafe fn reduce_kahan_neon(sum_vec: float32x4_t, c_vec: float32x4_t) -> (f32, f32) {
    unsafe {
        let mut s_arr = [0.0f32; 4];
        let mut c_arr = [0.0f32; 4];
        vst1q_f32(s_arr.as_mut_ptr(), sum_vec);
        vst1q_f32(c_arr.as_mut_ptr(), c_vec);

        let mut s = 0.0f32;
        let mut c = 0.0f32;
        for i in 0..4 {
            neumaier_add(&mut s, &mut c, s_arr[i]);
            neumaier_add(&mut s, &mut c, -c_arr[i]);
        }
        (s, c)
    }
}

/// Weighted mean using NEON SIMD with Kahan compensated summation.
///
/// # Safety
/// Caller must ensure NEON is available (always true on aarch64).
#[target_feature(enable = "neon")]
pub(crate) unsafe fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    unsafe {
        let mut sum_vw = vdupq_n_f32(0.0);
        let mut c_vw = vdupq_n_f32(0.0);
        let mut sum_w = vdupq_n_f32(0.0);
        let mut c_w = vdupq_n_f32(0.0);

        let v_chunks = values.chunks_exact(4);
        let v_rem = v_chunks.remainder();
        let mut w_ptr = weights.as_ptr();

        for v_chunk in v_chunks {
            let v = vld1q_f32(v_chunk.as_ptr());
            let w = vld1q_f32(w_ptr);
            w_ptr = w_ptr.add(4);

            let vw = vmulq_f32(v, w);

            let y = vsubq_f32(vw, c_vw);
            let t = vaddq_f32(sum_vw, y);
            c_vw = vsubq_f32(vsubq_f32(t, sum_vw), y);
            sum_vw = t;

            let y = vsubq_f32(w, c_w);
            let t = vaddq_f32(sum_w, y);
            c_w = vsubq_f32(vsubq_f32(t, sum_w), y);
            sum_w = t;
        }

        let (mut s_vw, mut c_s_vw) = reduce_kahan_neon(sum_vw, c_vw);
        let (mut s_w, mut c_s_w) = reduce_kahan_neon(sum_w, c_w);

        let w_rem = &weights[values.len() - v_rem.len()..];
        for (&v, &w) in v_rem.iter().zip(w_rem.iter()) {
            neumaier_add(&mut s_vw, &mut c_s_vw, v * w);
            neumaier_add(&mut s_w, &mut c_s_w, w);
        }

        let total = s_vw + c_s_vw;
        let total_w = s_w + c_s_w;

        if total_w > f32::EPSILON {
            total / total_w
        } else {
            0.0
        }
    }
}
