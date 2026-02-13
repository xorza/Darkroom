//! SSE SIMD implementations of sum operations (x86_64).

use std::arch::x86_64::*;

/// Sum f32 values using SSE4.1 SIMD with Kahan compensated summation.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub unsafe fn sum_f32(values: &[f32]) -> f32 {
    unsafe {
        let mut sum_vec = _mm_setzero_ps();
        let mut c_vec = _mm_setzero_ps(); // compensation

        let chunks = values.chunks_exact(4);
        let remainder = chunks.remainder();

        for chunk in chunks {
            let v = _mm_loadu_ps(chunk.as_ptr());
            let y = _mm_sub_ps(v, c_vec); // compensated value
            let t = _mm_add_ps(sum_vec, y);
            c_vec = _mm_sub_ps(_mm_sub_ps(t, sum_vec), y); // (t - sum) - y
            sum_vec = t;
        }

        // Reduce sum and compensation lanes separately to preserve precision.
        let mut s_arr = [0.0f32; 4];
        let mut c_arr = [0.0f32; 4];
        _mm_storeu_ps(s_arr.as_mut_ptr(), sum_vec);
        _mm_storeu_ps(c_arr.as_mut_ptr(), c_vec);

        let mut s = 0.0f32;
        let mut c = 0.0f32;
        for i in 0..4 {
            // Add lane sum with Neumaier
            let t = s + s_arr[i];
            if s.abs() >= s_arr[i].abs() {
                c += (s - t) + s_arr[i];
            } else {
                c += (s_arr[i] - t) + s;
            }
            s = t;
            // Add lane compensation (subtract since Kahan c tracks error to subtract)
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
