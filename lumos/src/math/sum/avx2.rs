//! AVX2 SIMD implementations of sum operations (x86_64).

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// Sum f32 values using AVX2 SIMD with Kahan compensated summation.
///
/// # Safety
/// Caller must ensure AVX2 is available.
#[target_feature(enable = "avx2")]
pub unsafe fn sum_f32(values: &[f32]) -> f32 {
    unsafe {
        let mut sum_vec = _mm256_setzero_ps();
        let mut c_vec = _mm256_setzero_ps();

        let chunks = values.chunks_exact(8);
        let remainder = chunks.remainder();

        for chunk in chunks {
            let v = _mm256_loadu_ps(chunk.as_ptr());
            let y = _mm256_sub_ps(v, c_vec);
            let t = _mm256_add_ps(sum_vec, y);
            c_vec = _mm256_sub_ps(_mm256_sub_ps(t, sum_vec), y);
            sum_vec = t;
        }

        // Reduce sum and compensation lanes separately to preserve precision.
        let mut s_arr = [0.0f32; 8];
        let mut c_arr = [0.0f32; 8];
        _mm256_storeu_ps(s_arr.as_mut_ptr(), sum_vec);
        _mm256_storeu_ps(c_arr.as_mut_ptr(), c_vec);

        let mut s = 0.0f32;
        let mut c = 0.0f32;
        for i in 0..8 {
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
