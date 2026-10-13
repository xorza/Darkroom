//! Cubic-spline interpolation of a background row segment as a vector kernel.
//!
//! The lanes evaluate [`SplineSegment::eval`]'s own unfused expression at the parameter
//! `start + i·step`, rounded as the scalar reference rounds it, so every pixel is the scalar result
//! bit for bit, the last partial vector's too.

use crate::background_mesh::spline::spline_segment::SplineSegment;
use crate::simd::{F32_LANES, F32x8, Isa, Kernel};

/// The spline parameter ramp across a segment: t(i) = `start` + i · `step`.
#[derive(Debug, Clone, Copy)]
pub(super) struct SegmentRamp {
    /// Parameter at the first output pixel (0.0 at the left tile center).
    pub(super) start: f32,
    /// Parameter increment per pixel.
    pub(super) step: f32,
}

/// Natural cubic spline interpolation of one row segment, background and noise together, as a
/// kernel. `bg_out` and `noise_out` have the same length: the kernel walks them in lockstep, and a
/// shorter one would leave the other's tail unwritten.
#[derive(Debug)]
pub(super) struct InterpolateSegment<'a> {
    pub(super) bg_out: &'a mut [f32],
    pub(super) noise_out: &'a mut [f32],
    pub(super) bg: SplineSegment,
    pub(super) noise: SplineSegment,
    pub(super) ramp: SegmentRamp,
}

impl Kernel for InterpolateSegment<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        debug_assert_eq!(self.bg_out.len(), self.noise_out.len());
        let bg = Coefficients::splat(isa, self.bg);
        let noise = Coefficients::splat(isa, self.noise);
        let one = isa.splat_f32(1.0);
        let two = isa.splat_f32(2.0);
        let start = isa.splat_f32(self.ramp.start);
        let step = isa.splat_f32(self.ramp.step);
        let lanes = isa.load_f32(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);

        let chunks = self
            .bg_out
            .chunks_mut(F32_LANES)
            .zip(self.noise_out.chunks_mut(F32_LANES));
        for (chunk, (bg_out, noise_out)) in chunks.enumerate() {
            // `start + i·step` per lane: a parameter stepped by repeated addition would drift
            // from the scalar one along the segment. The index is exact in f32 for any row below
            // 2²⁴ pixels.
            let index = isa.splat_f32((chunk * F32_LANES) as f32) + lanes;
            let t = start + index * step;
            let t_ct = t * (one - t);
            let two_minus_t = two - t;
            let one_plus_t = one + t;
            bg.eval(t, t_ct, two_minus_t, one_plus_t)
                .store_partial(bg_out);
            noise
                .eval(t, t_ct, two_minus_t, one_plus_t)
                .store_partial(noise_out);
        }
    }
}

/// One [`SplineSegment`]'s coefficients, splat across the lanes.
#[derive(Debug, Clone, Copy)]
struct Coefficients<V> {
    f0: V,
    rise: V,
    a: V,
    b: V,
}

impl<V: F32x8> Coefficients<V> {
    #[inline(always)]
    fn splat<S: Isa<F32 = V>>(isa: S, segment: SplineSegment) -> Self {
        Self {
            f0: isa.splat_f32(segment.f0),
            rise: isa.splat_f32(segment.f1 - segment.f0),
            a: isa.splat_f32(segment.a),
            b: isa.splat_f32(segment.b),
        }
    }

    /// `f0 + t·(f1 − f0) − t·(1 − t)·((2 − t)·a + (1 + t)·b)`, in [`SplineSegment::eval`]'s order.
    #[inline(always)]
    fn eval(self, t: V, t_ct: V, two_minus_t: V, one_plus_t: V) -> V {
        self.f0 + t * self.rise - t_ct * (two_minus_t * self.a + one_plus_t * self.b)
    }
}

#[cfg(test)]
mod internals {
    use crate::star_detection::background::simd::{InterpolateSegment, SegmentRamp};

    impl SegmentRamp {
        /// The spline parameter at output pixel `i`: outside [0, 1] past the segment's knots,
        /// where the end segments extrapolate.
        pub(super) fn t_at(self, i: usize) -> f32 {
            self.start + i as f32 * self.step
        }
    }

    impl InterpolateSegment<'_> {
        /// The scalar reference: `SplineSegment::eval` at each pixel's parameter.
        pub(super) fn run_scalar(self) {
            for (i, (bg_px, noise_px)) in self
                .bg_out
                .iter_mut()
                .zip(self.noise_out.iter_mut())
                .enumerate()
            {
                let t = self.ramp.t_at(i);
                *bg_px = self.bg.eval(t);
                *noise_px = self.noise.eval(t);
            }
        }
    }
}

#[cfg(test)]
mod tests;
