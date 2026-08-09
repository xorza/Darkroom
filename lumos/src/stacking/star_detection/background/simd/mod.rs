//! SIMD-accelerated background estimation utilities.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE on x86_64
//! - NEON on aarch64
//! - Scalar fallback on other platforms

use crate::simd::dispatch;

#[cfg(target_arch = "x86_64")]
mod avx2;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod sse41;

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

    dispatch! {
        x86: avx2_fma => avx2::interpolate_segment_cubic_avx2(bg_out, noise_out, bg, noise, ramp),
        x86: sse4_1 => sse41::interpolate_segment_cubic_sse(bg_out, noise_out, bg, noise, ramp),
        aarch64 => neon::interpolate_segment_cubic_neon(bg_out, noise_out, bg, noise, ramp),
        scalar => interpolate_segment_cubic_scalar(bg_out, noise_out, bg, noise, ramp),
    }
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

#[cfg(test)]
mod tests;
