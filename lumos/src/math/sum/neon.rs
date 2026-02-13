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
