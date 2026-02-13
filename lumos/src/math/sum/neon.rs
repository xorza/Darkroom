//! NEON SIMD implementations of sum operations (aarch64).

use std::arch::aarch64::*;

/// Sum f32 values using NEON SIMD with Kahan compensated summation.
///
/// # Safety
/// Caller must ensure NEON is available (always true on aarch64).
#[target_feature(enable = "neon")]
pub unsafe fn sum_f32(values: &[f32]) -> f32 {
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

        // Reduce sum and compensation lanes separately to preserve precision.
        let mut s_arr = [0.0f32; 4];
        let mut c_arr = [0.0f32; 4];
        vst1q_f32(s_arr.as_mut_ptr(), sum_vec);
        vst1q_f32(c_arr.as_mut_ptr(), c_vec);

        let mut s = 0.0f32;
        let mut c = 0.0f32;
        for i in 0..4 {
            let t = s + s_arr[i];
            if s.abs() >= s_arr[i].abs() {
                c += (s - t) + s_arr[i];
            } else {
                c += (s_arr[i] - t) + s;
            }
            s = t;
            let ci = -c_arr[i];
            let t = s + ci;
            if s.abs() >= ci.abs() {
                c += (s - t) + ci;
            } else {
                c += (ci - t) + s;
            }
            s = t;
        }

        // Neumaier for remainder elements
        for &v in remainder {
            let t = s + v;
            if s.abs() >= v.abs() {
                c += (s - t) + v;
            } else {
                c += (v - t) + s;
            }
            s = t;
        }

        s + c
    }
}

/// Kahan horizontal reduction of 4 sum lanes + 4 compensation lanes.
#[inline]
#[target_feature(enable = "neon")]
unsafe fn reduce_kahan_neon(sum_vec: float32x4_t, c_vec: float32x4_t) -> (f32, f32) {
    let mut s_arr = [0.0f32; 4];
    let mut c_arr = [0.0f32; 4];
    vst1q_f32(s_arr.as_mut_ptr(), sum_vec);
    vst1q_f32(c_arr.as_mut_ptr(), c_vec);

    let mut s = 0.0f32;
    let mut c = 0.0f32;
    for i in 0..4 {
        let t = s + s_arr[i];
        if s.abs() >= s_arr[i].abs() {
            c += (s - t) + s_arr[i];
        } else {
            c += (s_arr[i] - t) + s;
        }
        s = t;
        let ci = -c_arr[i];
        let t = s + ci;
        if s.abs() >= ci.abs() {
            c += (s - t) + ci;
        } else {
            c += (ci - t) + s;
        }
        s = t;
    }
    (s, c)
}

/// Weighted mean using NEON SIMD with Kahan compensated summation.
///
/// # Safety
/// Caller must ensure NEON is available (always true on aarch64).
#[target_feature(enable = "neon")]
pub unsafe fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
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
            let vw = v * w;
            let t = s_vw + vw;
            if s_vw.abs() >= vw.abs() {
                c_s_vw += (s_vw - t) + vw;
            } else {
                c_s_vw += (vw - t) + s_vw;
            }
            s_vw = t;

            let t = s_w + w;
            if s_w.abs() >= w.abs() {
                c_s_w += (s_w - t) + w;
            } else {
                c_s_w += (w - t) + s_w;
            }
            s_w = t;
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
