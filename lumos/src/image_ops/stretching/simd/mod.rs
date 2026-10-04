//! The arcsinh curve as vector kernels — on a plane, and color-preserving over three.
//!
//! The per-sample `asinh` is the curve's hot spot. Every lane, the partial last vector's too,
//! computes it by [`Math::asinh_f32`], so a sample's value does not depend on where in the band it
//! falls; the scalar path calls libm's `asinhf`, and the two agree to a few ULP.

use crate::image_ops::stretching::{AsinhCurve, BlackPoint};
use crate::simd::math::Math;
use crate::simd::{F32_LANES, F32x8, Isa, Kernel, Mask8};

/// Apply the arcsinh plane curve after `black` in place to one band of a plane, on the widest Isa
/// this CPU has.
pub(super) fn asinh_plane(plane: &mut [f32], black: BlackPoint, curve: AsinhCurve) {
    AsinhPlane {
        plane,
        black,
        curve,
    }
    .dispatch();
}

/// [`asinh_plane`] as a kernel: `clamp(asinh(v′ / β) / norm, 0, 1)` of `v′` after the black point,
/// a NaN sample to 0.
#[derive(Debug)]
struct AsinhPlane<'a> {
    plane: &'a mut [f32],
    black: BlackPoint,
    curve: AsinhCurve,
}

impl Kernel for AsinhPlane<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let curve = SplatCurve::splat(isa, self.black, self.curve);
        let (chunks, tail) = self.plane.as_chunks_mut::<F32_LANES>();
        for samples in chunks {
            curve.plane(isa, isa.load_f32(samples)).store(samples);
        }
        if !tail.is_empty() {
            curve
                .plane(isa, isa.load_f32_partial(tail))
                .store_partial(tail);
        }
    }
}

/// Apply the color-preserving arcsinh curve after `black` in place to one band of three RGB-f32
/// **planes**, on the widest Isa this CPU has.
///
/// The three slices must be the same length. Callers split the planes in lockstep and hand each
/// task one band of every channel, so the Isa is chosen per band rather than per pixel.
pub(super) fn asinh_color_preserve(
    red: &mut [f32],
    green: &mut [f32],
    blue: &mut [f32],
    black: BlackPoint,
    curve: AsinhCurve,
) {
    AsinhColorPreserve {
        red,
        green,
        blue,
        black,
        curve,
    }
    .dispatch();
}

/// [`asinh_color_preserve`] as a kernel: `color_preserve_pixel` lane by lane — black point,
/// intensity, curve, channel scale, highlight cap.
#[derive(Debug)]
struct AsinhColorPreserve<'a> {
    red: &'a mut [f32],
    green: &'a mut [f32],
    blue: &'a mut [f32],
    black: BlackPoint,
    curve: AsinhCurve,
}

impl Kernel for AsinhColorPreserve<'_> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        debug_assert!(
            self.red.len() == self.green.len() && self.green.len() == self.blue.len(),
            "three planes of one band"
        );
        let curve = SplatCurve::splat(isa, self.black, self.curve);
        let (red, red_tail) = self.red.as_chunks_mut::<F32_LANES>();
        let (green, green_tail) = self.green.as_chunks_mut::<F32_LANES>();
        let (blue, blue_tail) = self.blue.as_chunks_mut::<F32_LANES>();
        for ((red, green), blue) in red.iter_mut().zip(green).zip(blue) {
            let [r, g, b] = curve.color_preserve(
                isa,
                [isa.load_f32(red), isa.load_f32(green), isa.load_f32(blue)],
            );
            r.store(red);
            g.store(green);
            b.store(blue);
        }
        if !red_tail.is_empty() {
            let [r, g, b] = curve.color_preserve(
                isa,
                [
                    isa.load_f32_partial(red_tail),
                    isa.load_f32_partial(green_tail),
                    isa.load_f32_partial(blue_tail),
                ],
            );
            r.store_partial(red_tail);
            g.store_partial(green_tail);
            b.store_partial(blue_tail);
        }
    }
}

/// The curve's constants, splat across the lanes.
#[derive(Debug, Clone, Copy)]
struct SplatCurve<V> {
    black: V,
    inv_range: V,
    inv_beta: V,
    inv_norm: V,
    third: V,
    zero: V,
    one: V,
}

impl<V: F32x8> SplatCurve<V> {
    #[inline(always)]
    fn splat<S: Isa<F32 = V>>(isa: S, black: BlackPoint, curve: AsinhCurve) -> Self {
        Self {
            black: isa.splat_f32(black.black),
            inv_range: isa.splat_f32(black.inv_range),
            inv_beta: isa.splat_f32(curve.inv_beta),
            inv_norm: isa.splat_f32(curve.inv_norm),
            third: isa.splat_f32(1.0 / 3.0),
            zero: isa.splat_f32(0.0),
            one: isa.splat_f32(1.0),
        }
    }

    /// `v` after the black point.
    #[inline(always)]
    fn rescale(self, v: V) -> V {
        (v - self.black) * self.inv_range
    }

    /// `clamp(asinh(v′ / β) / norm, 0, 1)` of `v′` after the black point, a NaN sample to 0.
    #[inline(always)]
    fn plane<S: Isa<F32 = V>>(self, isa: S, v: V) -> V {
        self.tone(isa, self.rescale(v))
    }

    /// `clamp(asinh(v / β) / norm, 0, 1)`, a NaN sample to 0.
    #[inline(always)]
    fn tone<S: Isa<F32 = V>>(self, isa: S, v: V) -> V {
        let curved = isa.asinh_f32(v * self.inv_beta) * self.inv_norm;
        curved.max(self.zero).min(self.one)
    }

    /// `color_preserve_pixel` lane by lane: black point, intensity, curve, channel scale,
    /// highlight cap.
    #[inline(always)]
    fn color_preserve<S: Isa<F32 = V>>(self, isa: S, [r, g, b]: [V; 3]) -> [V; 3] {
        let (r, g, b) = (self.rescale(r), self.rescale(g), self.rescale(b));
        let intensity = (r + g + b) * self.third;
        let target = self.tone(isa, intensity);
        // scale = target/intensity where intensity > 0, else 0 (sub-background pixels → black).
        let scale = intensity.lanes_gt(self.zero).keep(target / intensity);
        let (r, g, b) = (r * scale, g * scale, b * scale);
        // Hue-preserving highlight cap: divide by the max channel when it exceeds 1.
        let brightest = r.max(g).max(b);
        let cap = brightest
            .lanes_gt(self.one)
            .select(self.one / brightest, self.one);
        // A channel below black, possible beside a positive intensity, clamps to 0.
        [
            (r * cap).max(self.zero),
            (g * cap).max(self.zero),
            (b * cap).max(self.zero),
        ]
    }
}

#[cfg(test)]
mod internals {
    use crate::image_ops::rgb::Rgb;
    use crate::image_ops::stretching::{AsinhCurve, BlackPoint, ToneCurve, color_preserve_pixel};

    /// The scalar plane curve the kernel is tested against: libm's `asinhf` through
    /// [`AsinhCurve::eval`], after `black`.
    pub(super) fn asinh_plane_scalar(plane: &mut [f32], black: BlackPoint, curve: AsinhCurve) {
        for value in plane {
            *value = curve.eval(black.rescale(*value));
        }
    }

    /// The scalar color-preserving curve the kernel is tested against: [`color_preserve_pixel`].
    pub(super) fn asinh_color_preserve_scalar(
        red: &mut [f32],
        green: &mut [f32],
        blue: &mut [f32],
        black: BlackPoint,
        curve: AsinhCurve,
    ) {
        for ((r, g), b) in red.iter_mut().zip(green.iter_mut()).zip(blue.iter_mut()) {
            let out = color_preserve_pixel(
                Rgb {
                    r: *r,
                    g: *g,
                    b: *b,
                },
                black,
                &curve,
            );
            *r = out.r;
            *g = out.g;
            *b = out.b;
        }
    }
}

#[cfg(test)]
mod tests;
