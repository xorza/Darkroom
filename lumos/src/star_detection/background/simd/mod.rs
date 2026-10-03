//! SIMD-accelerated background estimation utilities.
//!
//! This module provides runtime dispatch to the best available SIMD implementation:
//! - AVX2/SSE on `x86_64`
//! - NEON on aarch64
//! - Scalar fallback on other platforms

use crate::background_mesh::spline::spline_segment::SplineSegment;
use crate::simd::dispatch;

#[cfg(target_arch = "x86_64")]
mod avx2;

#[cfg(target_arch = "aarch64")]
mod neon;

#[cfg(target_arch = "x86_64")]
mod sse41;

/// The spline parameter ramp across a segment: t(i) = `start` + i · `step`.
#[derive(Debug, Clone, Copy)]
pub(super) struct SegmentRamp {
    /// Parameter at the first output pixel (0.0 at the left tile center).
    pub(super) start: f32,
    /// Parameter increment per pixel.
    pub(super) step: f32,
}

impl SegmentRamp {
    /// The spline parameter at output pixel `i`: outside [0, 1] past the segment's knots, where
    /// the end segments extrapolate.
    #[inline]
    fn t_at(self, i: usize) -> f32 {
        self.start + i as f32 * self.step
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
