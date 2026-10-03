//! Every arcsinh backend the host has, against the scalar curve.

use crate::image_ops::stretching::AsinhCurve;
#[cfg(target_arch = "x86_64")]
use crate::image_ops::stretching::simd::avx2;
#[cfg(target_arch = "aarch64")]
use crate::image_ops::stretching::simd::neon;
use crate::image_ops::stretching::simd::{
    ASINH_LOG_FROM, asinh_color_preserve_scalar, asinh_plane_scalar,
};
use crate::testing::simd_check;
use crate::testing::simd_check::backend::Backend;
use crate::testing::simd_check::{SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar};
use imaginarium::SimdTier;
use std::f32::consts::LN_2;

type PlanesFn = unsafe fn(&mut [f32], &mut [f32], &mut [f32], f32, f32);
type PlaneFn = unsafe fn(&mut [f32], f32, f32);

const PLANE_BACKENDS: &[Backend<PlaneFn>] = &[
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Avx2Fma, avx2::asinh_plane_avx2),
    #[cfg(target_arch = "aarch64")]
    Backend::new(SimdTier::Neon, neon::asinh_plane_neon),
];

const BACKENDS: &[Backend<PlanesFn>] = &[
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Avx2Fma, avx2::asinh_color_preserve_avx2),
    #[cfg(target_arch = "aarch64")]
    Backend::new(SimdTier::Neon, neon::asinh_color_preserve_neon),
];

/// Every backend over three planes drawn from each shape: the shapes put intensities below zero
/// (black on both paths), inside the curve, and above the highlight cap.
///
/// The backends evaluate `asinh` through a Cephes `logf` (about 2 ULP) where the scalar path
/// calls libm's `asinhf` (at most 1 ULP), so the curve values differ by up to 3 ULP, which is 3ε
/// relative at most; the scale `f(I)/I` and the cap round once more on each side. The outputs
/// therefore agree to 4ε relative.
#[test]
fn asinh_backends_match_scalar() {
    let curve = AsinhCurve::new(0.05);
    assert_simd_matches_scalar(
        BACKENDS,
        SWEEP_WIDTHS,
        4.0 * f32::EPSILON,
        |kernel, shape, width| {
            let mut scalar = [
                shape.row(width, 0),
                shape.row(width, 1),
                shape.row(width, 2),
            ];
            let mut simd = scalar.clone();
            let [r, g, b] = &mut scalar;
            asinh_color_preserve_scalar(r, g, b, curve);
            let [r, g, b] = &mut simd;
            // SAFETY: the harness runs only backends whose tier this CPU has, and the three
            // planes have one length.
            unsafe { kernel(r, g, b, curve.inv_beta, curve.inv_norm) };
            ScalarSimd::relative(scalar.concat(), simd.concat())
        },
    );
}

/// `asinh(x)` for `x ≥ 0` by the steps the vector backends take, one lane at a time with libm's
/// `ln` for the Cephes `logf`: below [`ASINH_LOG_FROM`], `log1p(u)` with `u = x + x²/(1 + √(1 +
/// x²))` — `√(1 + x²) + x − 1` without its cancellation — and `log1p(u) = ln(1 + u) · u / ((1 + u)
/// − 1)`, whose ratio cancels the rounding of `1 + u` (Goldberg 1991); past it, `ln x + ln 2`. The
/// textbook `ln(x + √(x² + 1))` instead loses all relative accuracy as `x → 0`, where `x + √(x²+1)`
/// rounds to 1.
fn asinh_pos_scalar(x: f32) -> f32 {
    if x > ASINH_LOG_FROM {
        return x.ln() + LN_2;
    }
    let s = x * x;
    let u = x + s / (1.0 + (1.0 + s).sqrt());
    let w = 1.0 + u;
    let dw = w - 1.0;
    if dw == 0.0 { u } else { w.ln() * (u / dw) }
}

/// The magnitudes the curve's `asinh` sees: 0, and every power of ten from 1e-30 to 1e30 with a
/// spread of mantissas, across
/// [`ASINH_LOG_FROM`](crate::image_ops::stretching::simd::ASINH_LOG_FROM).
fn magnitudes() -> Vec<f32> {
    let mut values = vec![0.0f32];
    for exponent in -30..=30 {
        for mantissa in [1.0f32, 1.7, 3.3, 7.9] {
            values.push(mantissa * 10f32.powi(exponent));
        }
    }
    values
}

/// The relative error of `got` against `asinh(x)` in f64.
fn relative_error(x: f32, got: f32) -> f64 {
    let truth = f64::from(x).asinh();
    if truth == 0.0 {
        f64::from(got).abs()
    } else {
        (f64::from(got) - truth).abs() / truth
    }
}

/// The steps the backends take hold `asinh` to a few ε relative at every magnitude — through
/// `x → 0`, where the textbook `ln(x + √(x² + 1))` loses its relative accuracy as `ε/x` (6e-4 off
/// at x = 1e-4), and past [`ASINH_LOG_FROM`] — and so do the backends themselves, whose Cephes
/// `logf` adds its ~2 ULP. The steps round `u` three times and `ln(1 + u) · u/((1 + u) − 1)` three
/// more: 6ε for the scalar steps, 8ε for the vector ones.
#[test]
fn asinh_is_accurate_at_every_magnitude() {
    let values = magnitudes();
    for &x in &values {
        let error = relative_error(x, asinh_pos_scalar(x));
        assert!(error <= 6.0 * f64::from(f32::EPSILON), "x = {x}: {error:e}");
    }
    for backend in PLANE_BACKENDS
        .iter()
        .filter(|backend| simd_check::runs_here(backend.tier))
    {
        let mut plane = values.clone();
        // SAFETY: `runs_here` checked the tier; the slice is the kernel's own.
        unsafe { (backend.kernel)(&mut plane, 1.0, 1.0) };
        for (&x, &got) in values.iter().zip(&plane) {
            // The plane curve clamps to [0, 1]: compare below that.
            let expected = f64::from(x).asinh();
            if expected < 1.0 {
                let error = relative_error(x, got);
                assert!(error <= 8.0 * f64::from(f32::EPSILON), "x = {x}: {error:e}");
            } else {
                assert_eq!(got, 1.0, "x = {x}");
            }
        }
    }
}

/// The plane backends against the scalar curve (libm's `asinhf`), at a curve whose `1/β` puts the
/// shapes' samples across the whole `asinh` range: both within the 8ε the vector steps keep, plus
/// the scale's and the clamp's one rounding each.
#[test]
fn asinh_plane_backends_match_scalar() {
    let curve = AsinhCurve::new(0.05);
    assert_simd_matches_scalar(
        PLANE_BACKENDS,
        SWEEP_WIDTHS,
        10.0 * f32::EPSILON,
        |kernel, shape, width| {
            let mut scalar = shape.row(width, 0);
            let mut simd = scalar.clone();
            asinh_plane_scalar(&mut scalar, curve);
            // SAFETY: the harness runs only backends whose tier this CPU has.
            unsafe { kernel(&mut simd, curve.inv_beta, curve.inv_norm) };
            ScalarSimd::relative(scalar, simd)
        },
    );
}
