//! SIMD-accelerated background estimation utilities.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

#[cfg(target_arch = "x86_64")]
use imaginarium::cpu_features;

/// Natural cubic spline coefficients for one channel over a segment between two tile centers.
#[derive(Debug, Clone, Copy)]
pub(super) struct SplineSegment {
    /// Value at the left tile center (t = 0).
    pub(super) f0: f32,
    /// Value at the right tile center (t = 1).
    pub(super) f1: f32,
    /// Correction term h²/6 · d2 at the left center.
    pub(super) a: f32,
    /// Correction term h²/6 · d2 at the right center.
    pub(super) b: f32,
}

impl SplineSegment {
    /// Evaluates f(t) = (1-t)*f0 + t*f1 - t*(1-t)*((2-t)*a + (1+t)*b).
    ///
    /// Same polynomial as `background_mesh::spline::cubic_spline_eval`, but takes the
    /// precomputed `a, b = h²/6·d2` instead of raw second derivatives — keep the two in sync.
    #[inline]
    fn eval(self, t: f32) -> f32 {
        let ct = 1.0 - t;
        let t_ct = t * ct;
        ct * self.f0 + t * self.f1 - t_ct * ((2.0 - t) * self.a + (1.0 + t) * self.b)
    }
}

/// The spline parameter ramp across a segment: t(i) = `start` + i · `step`.
#[derive(Debug, Clone, Copy)]
pub(super) struct SegmentRamp {
    /// Parameter at the first output pixel (0.0 at the left tile center).
    pub(super) start: f32,
    /// Parameter increment per pixel.
    pub(super) step: f32,
}

impl SegmentRamp {
    /// The clamped spline parameter at output pixel `i`.
    #[inline]
    fn t_at(self, i: usize) -> f32 {
        (self.start + i as f32 * self.step).clamp(0.0, 1.0)
    }
}

/// Natural cubic spline interpolation for a row segment using SIMD.
///
/// `bg_out` and `noise_out` are the output slices, which must have the same length.
pub(super) fn interpolate_segment_cubic_simd(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg: SplineSegment,
    noise: SplineSegment,
    ramp: SegmentRamp,
) {
    // Release assert, not debug: every SIMD backend below derives its store bound solely from
    // bg_out.len() and writes into noise_out using that same bound — a length mismatch would be
    // an out-of-bounds write into noise_out, not just a wrong value. O(1) check, not expensive.
    assert_eq!(bg_out.len(), noise_out.len());

    #[cfg(target_arch = "x86_64")]
    {
        if cpu_features::has_avx2_fma() {
            unsafe {
                interpolate_segment_cubic_avx2(bg_out, noise_out, bg, noise, ramp);
            }
            return;
        }
        if cpu_features::has_sse4_1() {
            unsafe {
                interpolate_segment_cubic_sse(bg_out, noise_out, bg, noise, ramp);
            }
            return;
        }
    }

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            interpolate_segment_cubic_neon(bg_out, noise_out, bg, noise, ramp);
        }
        return;
    }

    // Scalar fallback
    #[allow(unreachable_code)]
    interpolate_segment_cubic_scalar(bg_out, noise_out, bg, noise, ramp);
}

/// Scalar implementation of cubic spline segment interpolation.
#[inline]
fn interpolate_segment_cubic_scalar(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg: SplineSegment,
    noise: SplineSegment,
    ramp: SegmentRamp,
) {
    for (i, (bg_px, noise_px)) in bg_out.iter_mut().zip(noise_out.iter_mut()).enumerate() {
        let t = ramp.t_at(i);
        *bg_px = bg.eval(t);
        *noise_px = noise.eval(t);
    }
}

/// Evaluate cubic spline for 8 values using AVX2+FMA.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn interpolate_segment_cubic_avx2(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg: SplineSegment,
    noise: SplineSegment,
    ramp: SegmentRamp,
) {
    use std::arch::x86_64::*;

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

/// Evaluate cubic spline for 4 values using SSE4.1.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn interpolate_segment_cubic_sse(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg: SplineSegment,
    noise: SplineSegment,
    ramp: SegmentRamp,
) {
    use std::arch::x86_64::*;

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

/// Evaluate cubic spline for 4 values using NEON.
#[cfg(target_arch = "aarch64")]
unsafe fn interpolate_segment_cubic_neon(
    bg_out: &mut [f32],
    noise_out: &mut [f32],
    bg: SplineSegment,
    noise: SplineSegment,
    ramp: SegmentRamp,
) {
    use std::arch::aarch64::*;

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

#[cfg(test)]
mod tests {
    use crate::stacking::star_detection::background::simd::*;

    #[test]
    #[should_panic(expected = "assertion")]
    fn test_cubic_segment_simd_mismatched_lengths_panics() {
        // Every SIMD backend derives its store bound solely from bg_out.len() and writes
        // into noise_out with that same bound — a mismatch must be rejected even in release
        // builds, not just debug, since it would otherwise be an out-of-bounds write.
        let mut bg = vec![0.0f32; 8];
        let mut noise = vec![0.0f32; 4];
        interpolate_segment_cubic_simd(
            &mut bg,
            &mut noise,
            SplineSegment {
                f0: 100.0,
                f1: 200.0,
                a: -5.0,
                b: 3.0,
            },
            SplineSegment {
                f0: 5.0,
                f1: 10.0,
                a: -0.5,
                b: 0.3,
            },
            SegmentRamp {
                start: 0.0,
                step: 0.1,
            },
        );
    }

    #[test]
    fn test_cubic_segment_simd_matches_scalar() {
        // Non-trivial spline parameters: a = h²/6 * d2_left, b = h²/6 * d2_right
        let bg = SplineSegment {
            f0: 100.0,
            f1: 200.0,
            a: -5.0,
            b: 3.0,
        };
        let noise = SplineSegment {
            f0: 5.0,
            f1: 10.0,
            a: -0.5,
            b: 0.3,
        };

        // Test various segment lengths including SIMD boundary cases
        for len in [1, 3, 4, 7, 8, 15, 16, 31, 64, 100] {
            let mut bg_simd = vec![0.0f32; len];
            let mut noise_simd = vec![0.0f32; len];
            let mut bg_scalar = vec![0.0f32; len];
            let mut noise_scalar = vec![0.0f32; len];

            let ramp = SegmentRamp {
                start: 0.1,
                step: 0.8 / len as f32,
            };

            interpolate_segment_cubic_simd(&mut bg_simd, &mut noise_simd, bg, noise, ramp);
            interpolate_segment_cubic_scalar(&mut bg_scalar, &mut noise_scalar, bg, noise, ramp);

            for i in 0..len {
                assert!(
                    (bg_simd[i] - bg_scalar[i]).abs() < 1e-4,
                    "len={}, i={}: bg mismatch {} vs {}",
                    len,
                    i,
                    bg_simd[i],
                    bg_scalar[i]
                );
                assert!(
                    (noise_simd[i] - noise_scalar[i]).abs() < 1e-4,
                    "len={}, i={}: noise mismatch {} vs {}",
                    len,
                    i,
                    noise_simd[i],
                    noise_scalar[i]
                );
            }
        }
    }

    #[test]
    fn test_cubic_segment_simd_endpoints() {
        // At t=0 result should be f0, at t=1 result should be f1
        // (regardless of a, b coefficients, since t*(1-t) = 0 at both endpoints)
        let mut bg = vec![0.0f32; 2];
        let mut noise = vec![0.0f32; 2];

        // t=0 for first pixel, t=1 for second pixel
        interpolate_segment_cubic_simd(
            &mut bg,
            &mut noise,
            SplineSegment {
                f0: 100.0,
                f1: 200.0,
                a: -10.0,
                b: 7.0,
            },
            SplineSegment {
                f0: 5.0,
                f1: 15.0,
                a: -1.0,
                b: 0.5,
            },
            SegmentRamp {
                start: 0.0,
                step: 1.0,
            },
        );

        // f(0) = 1*100 + 0*200 + 0*1*(1*a + 0*b) = 100
        assert!(
            (bg[0] - 100.0).abs() < 1e-4,
            "t=0: bg should be f0=100, got {}",
            bg[0]
        );
        // f(1) = 0*100 + 1*200 + 1*0*(0*a + 1*b) = 200
        assert!(
            (bg[1] - 200.0).abs() < 1e-4,
            "t=1: bg should be f1=200, got {}",
            bg[1]
        );
        assert!(
            (noise[0] - 5.0).abs() < 1e-4,
            "t=0: noise should be f0=5, got {}",
            noise[0]
        );
        assert!(
            (noise[1] - 15.0).abs() < 1e-4,
            "t=1: noise should be f1=15, got {}",
            noise[1]
        );
    }

    #[test]
    fn test_cubic_segment_simd_midpoint() {
        // At t=0.5, using f(t) = ct*f0 + t*f1 - t*ct*((2-t)*a + (1+t)*b):
        //   = 0.5*f0 + 0.5*f1 - 0.5*0.5*(1.5*a + 1.5*b)
        //   = (f0+f1)/2 - 0.375*(a+b)
        let mut bg = vec![0.0f32; 1];
        let mut noise = vec![0.0f32; 1];

        // Expected: (100+200)/2 - 0.375*(-8+16) = 150 - 3 = 147
        interpolate_segment_cubic_simd(
            &mut bg,
            &mut noise,
            SplineSegment {
                f0: 100.0,
                f1: 200.0,
                a: -8.0,
                b: 16.0,
            },
            SplineSegment {
                f0: 0.0,
                f1: 0.0,
                a: 0.0,
                b: 0.0,
            },
            SegmentRamp {
                start: 0.5,
                step: 1.0,
            },
        );

        assert!(
            (bg[0] - 147.0).abs() < 1e-4,
            "Midpoint: expected 147, got {}",
            bg[0]
        );
    }

    #[test]
    fn test_cubic_segment_simd_linear_when_no_correction() {
        // With a=0, b=0, cubic spline reduces to linear interpolation
        let mut bg = vec![0.0f32; 50];
        let mut noise = vec![0.0f32; 50];

        let f0 = 100.0;
        let f1 = 200.0;
        let ramp = SegmentRamp {
            start: 0.0,
            step: 1.0 / 49.0,
        };

        interpolate_segment_cubic_simd(
            &mut bg,
            &mut noise,
            SplineSegment {
                f0,
                f1,
                a: 0.0,
                b: 0.0,
            },
            SplineSegment {
                f0: 5.0,
                f1: 10.0,
                a: 0.0,
                b: 0.0,
            },
            ramp,
        );

        for (i, &b) in bg.iter().enumerate() {
            let t = (i as f32 * ramp.step).clamp(0.0, 1.0);
            let expected = (1.0 - t) * f0 + t * f1;
            assert!(
                (b - expected).abs() < 1e-3,
                "i={}: expected linear {}, got {}",
                i,
                expected,
                b
            );
        }
    }

    #[test]
    fn test_cubic_segment_simd_clamping() {
        // t values outside [0,1] should be clamped
        let mut bg = vec![0.0f32; 10];
        let mut noise = vec![0.0f32; 10];

        // tx_start = -0.5, step = 0.2 → t goes from -0.5 to 1.3
        interpolate_segment_cubic_simd(
            &mut bg,
            &mut noise,
            SplineSegment {
                f0: 100.0,
                f1: 200.0,
                a: -5.0,
                b: 3.0,
            },
            SplineSegment {
                f0: 5.0,
                f1: 10.0,
                a: -0.5,
                b: 0.3,
            },
            SegmentRamp {
                start: -0.5,
                step: 0.2,
            },
        );

        // First element: t = -0.5 clamped to 0 → bg = f0 = 100
        assert!(
            (bg[0] - 100.0).abs() < 1e-4,
            "t<0 clamped: expected f0=100, got {}",
            bg[0]
        );

        // Last element: t = -0.5 + 9*0.2 = 1.3 clamped to 1 → bg = f1 = 200
        assert!(
            (bg[9] - 200.0).abs() < 1e-4,
            "t>1 clamped: expected f1=200, got {}",
            bg[9]
        );
    }
}
