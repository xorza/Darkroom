//! The tone curves whose samples take a transcendental — arcsinh and the generalized hyperbolic
//! stretch — as vector kernels, on a plane and color-preserving over three.
//!
//! The per-sample `asinh`, `exp_m1` and `ln_1p` are the curves' hot spots. Every lane, the
//! partial last vector's too, computes them by [`Math`], so a sample's value does not depend on
//! where in the band it falls, and so do the one-value kernels ([`AsinhOf`], [`GhsBaseOf`],
//! [`GhsSlopeOf`]) the curves' constants and the scalar references take.

use crate::image_ops::stretch::{AsinhCurve, BlackPoint, GhsCurve, GhsShape};
use simd::Math;
use simd::{F32_LANES, F32x8, Isa, Kernel, Mask8};

/// A tone curve the kernels apply eight samples at a time: the curve's value clamped to `[0, 1]`,
/// a NaN sample to 0.
pub(super) trait VectorTone: Copy {
    fn tone<S: Isa>(self, isa: S, v: S::F32) -> S::F32;
}

/// The tone curve `T` after `black`, in place, over one band of a plane.
#[derive(Debug)]
pub(super) struct TonePlane<'a, T> {
    pub(super) plane: &'a mut [f32],
    pub(super) black: BlackPoint,
    pub(super) curve: T,
}

impl<T: VectorTone> Kernel for TonePlane<'_, T> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        let splat = Splat::new(isa, self.black);
        let (chunks, tail) = self.plane.as_chunks_mut::<F32_LANES>();
        for samples in chunks {
            self.curve
                .tone(isa, splat.rescale(isa.load_f32(samples)))
                .store(samples);
        }
        if !tail.is_empty() {
            self.curve
                .tone(isa, splat.rescale(isa.load_f32_partial(tail)))
                .store_partial(tail);
        }
    }
}

/// The color-preserving tone curve `T` after `black`, in place, over one band of three RGB-f32
/// **planes**: `color_preserve_pixel` lane by lane — black point, intensity, curve, channel
/// scale, highlight cap.
///
/// The three slices must be the same length. Callers split the planes in lockstep and hand each
/// task one band of every channel, so the Isa is chosen per band rather than per pixel.
#[derive(Debug)]
pub(super) struct ToneColorPreserve<'a, T> {
    pub(super) red: &'a mut [f32],
    pub(super) green: &'a mut [f32],
    pub(super) blue: &'a mut [f32],
    pub(super) black: BlackPoint,
    pub(super) curve: T,
}

impl<T: VectorTone> Kernel for ToneColorPreserve<'_, T> {
    type Output = ();

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) {
        debug_assert!(
            self.red.len() == self.green.len() && self.green.len() == self.blue.len(),
            "three planes of one band"
        );
        let splat = Splat::new(isa, self.black);
        let (red, red_tail) = self.red.as_chunks_mut::<F32_LANES>();
        let (green, green_tail) = self.green.as_chunks_mut::<F32_LANES>();
        let (blue, blue_tail) = self.blue.as_chunks_mut::<F32_LANES>();
        for ((red, green), blue) in red.iter_mut().zip(green).zip(blue) {
            let [r, g, b] = splat.color_preserve(
                isa,
                self.curve,
                [isa.load_f32(red), isa.load_f32(green), isa.load_f32(blue)],
            );
            r.store(red);
            g.store(green);
            b.store(blue);
        }
        if !red_tail.is_empty() {
            let [r, g, b] = splat.color_preserve(
                isa,
                self.curve,
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

/// `asinh` of one value by [`Math::asinh_f32`], the kernels' steps: the curve's norm and its solver
/// take it, and the scalar reference the kernels are tested against.
#[derive(Debug)]
pub(super) struct AsinhOf(pub(super) f32);

impl Kernel for AsinhOf {
    type Output = f32;

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) -> f32 {
        isa.asinh_f32(isa.splat_f32(self.0)).to_array()[0]
    }
}

impl VectorTone for AsinhCurve {
    /// `clamp(asinh(v / β) / norm, 0, 1)`, a NaN sample to 0.
    #[inline(always)]
    fn tone<S: Isa>(self, isa: S, v: S::F32) -> S::F32 {
        let curved = isa.asinh_f32(v * isa.splat_f32(self.inv_beta)) * isa.splat_f32(self.inv_norm);
        curved.max(isa.splat_f32(0.0)).min(isa.splat_f32(1.0))
    }
}

/// The GHS base `T(u)` of one `u` by the kernels' steps: the curve's constants take it, and the
/// scalar reference the kernels are tested against.
#[derive(Debug)]
pub(super) struct GhsBaseOf {
    pub(super) shape: GhsShape,
    pub(super) u: f32,
}

impl Kernel for GhsBaseOf {
    type Output = f32;

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) -> f32 {
        self.shape.base_lanes(isa, isa.splat_f32(self.u)).to_array()[0]
    }
}

/// The GHS slope `T'(u)` of one `u` by the kernels' steps, for the curve's linear tails.
#[derive(Debug)]
pub(super) struct GhsSlopeOf {
    pub(super) shape: GhsShape,
    pub(super) u: f32,
}

impl Kernel for GhsSlopeOf {
    type Output = f32;

    #[inline(always)]
    fn run<S: Isa>(self, isa: S) -> f32 {
        self.shape
            .slope_lanes(isa, isa.splat_f32(self.u))
            .to_array()[0]
    }
}

impl GhsShape {
    /// The base hyperbolic function `T(u)` for `u ≥ 0`, selected on `b`. `T(0) = 0` in every case,
    /// which is what makes the curve continuous at the symmetry point.
    ///
    /// Written through `ln_1p`/`exp_m1` so it stays accurate as `b` nears its two limits: the
    /// textbook `1 − (1 + b·d·u)^(−1/b)` cancels as `b → 0` (in f32, 18.6% off at `b = 2e-6`,
    /// `d = 5`) and the `b < 0` form divides by `b + 1` as `b → −1`. Only the limits themselves,
    /// `b = 0` (exponential) and `b = −1` (logarithmic), take their own forms — the general ones
    /// are `0/0` there exactly. Each branch fixes its own scale, which the curve's normalization
    /// divides out. A negation multiplies by −1, which flips the sign of a zero as `-x` does.
    #[inline(always)]
    fn base_lanes<S: Isa>(self, isa: S, u: S::F32) -> S::F32 {
        let Self { d, b, .. } = self;
        let minus_one = isa.splat_f32(-1.0);
        if b == 0.0 {
            minus_one * isa.exp_m1_f32(isa.splat_f32(-d) * u)
        } else if b == -1.0 {
            isa.ln_1p_f32(isa.splat_f32(d) * u) / isa.splat_f32(d)
        } else if b < 0.0 {
            let log = isa.ln_1p_f32(isa.splat_f32(-b * d) * u);
            minus_one * isa.exp_m1_f32(isa.splat_f32((b + 1.0) / b) * log)
                / isa.splat_f32(d * (b + 1.0))
        } else {
            let log = minus_one * isa.ln_1p_f32(isa.splat_f32(b * d) * u);
            minus_one * isa.exp_m1_f32(log / isa.splat_f32(b))
        }
    }

    /// The derivative `T'(u)` of [`Self::base_lanes`] — the slope of the linear shadow/highlight
    /// tails.
    #[inline(always)]
    fn slope_lanes<S: Isa>(self, isa: S, u: S::F32) -> S::F32 {
        let Self { d, b, .. } = self;
        if b == 0.0 {
            isa.splat_f32(d) * isa.exp_f32(isa.splat_f32(-d) * u)
        } else if b < 0.0 {
            isa.exp_f32(isa.ln_1p_f32(isa.splat_f32(-b * d) * u) / isa.splat_f32(b))
        } else {
            let log = isa.ln_1p_f32(isa.splat_f32(b * d) * u);
            isa.splat_f32(d) * isa.exp_f32(isa.splat_f32(-((1.0 + b) / b)) * log)
        }
    }
}

impl VectorTone for GhsCurve {
    /// The base mirrored about `sp` between the protection points, the linear tails outside them,
    /// normalized and clamped to `[0, 1]`, a NaN sample to 0: it fails every compare, takes the
    /// highlight tail and leaves it NaN.
    #[inline(always)]
    fn tone<S: Isa>(self, isa: S, v: S::F32) -> S::F32 {
        let GhsShape { sp, lp, hp, .. } = self.shape;
        let sp_v = isa.splat_f32(sp);
        let below_sp = v.lanes_lt(sp_v);
        let base = self
            .shape
            .base_lanes(isa, below_sp.select(sp_v - v, v - sp_v));
        let curve = below_sp.select(isa.splat_f32(-1.0) * base, base);
        let shadow =
            isa.splat_f32(self.tp_sp_lp) * (v - isa.splat_f32(lp)) - isa.splat_f32(self.t_sp_lp);
        let highlight =
            isa.splat_f32(self.tp_hp_sp) * (v - isa.splat_f32(hp)) + isa.splat_f32(self.t_hp_sp);
        let raw = v.lanes_lt(isa.splat_f32(lp)).select(
            shadow,
            v.lanes_lt(isa.splat_f32(hp)).select(curve, highlight),
        );
        ((raw - isa.splat_f32(self.t0)) * isa.splat_f32(self.inv_range))
            .max(isa.splat_f32(0.0))
            .min(isa.splat_f32(1.0))
    }
}

/// The black point's constants and the color-preserving steps' own, splat across the lanes.
#[derive(Debug, Clone, Copy)]
struct Splat<V> {
    black: V,
    inv_range: V,
    third: V,
    zero: V,
    one: V,
}

impl<V: F32x8> Splat<V> {
    #[inline(always)]
    fn new<S: Isa<F32 = V>>(isa: S, black: BlackPoint) -> Self {
        Self {
            black: isa.splat_f32(black.black),
            inv_range: isa.splat_f32(black.inv_range),
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

    #[inline(always)]
    fn color_preserve<S: Isa<F32 = V>, T: VectorTone>(
        self,
        isa: S,
        curve: T,
        [r, g, b]: [V; 3],
    ) -> [V; 3] {
        let (r, g, b) = (self.rescale(r), self.rescale(g), self.rescale(b));
        let intensity = (r + g + b) * self.third;
        let target = curve.tone(isa, intensity);
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
    use crate::image::rgb::Rgb;
    use crate::image_ops::stretch::simd::{ToneColorPreserve, TonePlane};
    use crate::image_ops::stretch::{ToneCurve, color_preserve_pixel};

    impl<T: ToneCurve> TonePlane<'_, T> {
        /// The scalar plane curve the kernel is tested against: `T::eval` after the black point.
        pub(super) fn run_scalar(self) {
            for value in self.plane {
                *value = self.curve.eval(self.black.rescale(*value));
            }
        }
    }

    impl<T: ToneCurve> ToneColorPreserve<'_, T> {
        /// The scalar color-preserving curve the kernel is tested against: `color_preserve_pixel`.
        pub(super) fn run_scalar(self) {
            let pixels = self
                .red
                .iter_mut()
                .zip(self.green.iter_mut())
                .zip(self.blue.iter_mut());
            for ((r, g), b) in pixels {
                let out = color_preserve_pixel(
                    Rgb {
                        r: *r,
                        g: *g,
                        b: *b,
                    },
                    self.black,
                    &self.curve,
                );
                *r = out.r;
                *g = out.g;
                *b = out.b;
            }
        }
    }
}

#[cfg(test)]
mod tests;
