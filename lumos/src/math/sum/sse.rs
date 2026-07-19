//! SSE SIMD weighted mean (x86_64).

use std::arch::x86_64::*;

use crate::math::sum::scalar::neumaier_add;

/// Kahan horizontal reduction of 4 sum lanes + 4 compensation lanes.
#[inline]
#[target_feature(enable = "sse4.1")]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn reduce_kahan_128(sum_vec: __m128, c_vec: __m128) -> (f32, f32) {
    let mut s_arr = [0.0f32; 4];
    let mut c_arr = [0.0f32; 4];
    _mm_storeu_ps(s_arr.as_mut_ptr(), sum_vec);
    _mm_storeu_ps(c_arr.as_mut_ptr(), c_vec);

    let mut s = 0.0f32;
    let mut c = 0.0f32;
    for i in 0..4 {
        neumaier_add(&mut s, &mut c, s_arr[i]);
        neumaier_add(&mut s, &mut c, -c_arr[i]);
    }
    (s, c)
}

/// Weighted mean using SSE4.1 SIMD with Kahan compensated summation.
///
/// # Safety
/// Caller must ensure SSE4.1 is available.
#[target_feature(enable = "sse4.1")]
pub(crate) unsafe fn weighted_mean_f32(values: &[f32], weights: &[f32]) -> f32 {
    unsafe {
        let mut sum_vw = _mm_setzero_ps();
        let mut c_vw = _mm_setzero_ps();
        let mut sum_w = _mm_setzero_ps();
        let mut c_w = _mm_setzero_ps();

        let v_chunks = values.chunks_exact(4);
        let v_rem = v_chunks.remainder();
        let mut w_ptr = weights.as_ptr();

        for v_chunk in v_chunks {
            let v = _mm_loadu_ps(v_chunk.as_ptr());
            let w = _mm_loadu_ps(w_ptr);
            w_ptr = w_ptr.add(4);

            let vw = _mm_mul_ps(v, w);

            let y = _mm_sub_ps(vw, c_vw);
            let t = _mm_add_ps(sum_vw, y);
            c_vw = _mm_sub_ps(_mm_sub_ps(t, sum_vw), y);
            sum_vw = t;

            let y = _mm_sub_ps(w, c_w);
            let t = _mm_add_ps(sum_w, y);
            c_w = _mm_sub_ps(_mm_sub_ps(t, sum_w), y);
            sum_w = t;
        }

        let (mut s_vw, mut c_s_vw) = reduce_kahan_128(sum_vw, c_vw);
        let (mut s_w, mut c_s_w) = reduce_kahan_128(sum_w, c_w);

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
