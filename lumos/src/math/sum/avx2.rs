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

/// Kahan horizontal reduction of 8 sum lanes + 8 compensation lanes into (total, total_c).
#[inline]
#[target_feature(enable = "avx2")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn reduce_kahan_256(sum_vec: __m256, c_vec: __m256) -> (f32, f32) {
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
    (s, c)
}

/// Weighted mean using AVX2 SIMD with Kahan compensated summation.
///
/// # Safety
/// Caller must ensure AVX2 is available.
#[target_feature(enable = "avx2")]
pub unsafe fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    unsafe {
        let mut sum_vw = _mm256_setzero_ps();
        let mut c_vw = _mm256_setzero_ps();
        let mut sum_w = _mm256_setzero_ps();
        let mut c_w = _mm256_setzero_ps();

        let v_chunks = values.chunks_exact(8);
        let v_rem = v_chunks.remainder();
        let mut w_ptr = weights.as_ptr();

        for v_chunk in v_chunks {
            let v = _mm256_loadu_ps(v_chunk.as_ptr());
            let w = _mm256_loadu_ps(w_ptr);
            w_ptr = w_ptr.add(8);

            let vw = _mm256_mul_ps(v, w);

            // Kahan for v*w
            let y = _mm256_sub_ps(vw, c_vw);
            let t = _mm256_add_ps(sum_vw, y);
            c_vw = _mm256_sub_ps(_mm256_sub_ps(t, sum_vw), y);
            sum_vw = t;

            // Kahan for w
            let y = _mm256_sub_ps(w, c_w);
            let t = _mm256_add_ps(sum_w, y);
            c_w = _mm256_sub_ps(_mm256_sub_ps(t, sum_w), y);
            sum_w = t;
        }

        let (mut s_vw, mut c_s_vw) = reduce_kahan_256(sum_vw, c_vw);
        let (mut s_w, mut c_s_w) = reduce_kahan_256(sum_w, c_w);

        // Neumaier for remainder
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
