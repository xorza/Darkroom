//! Every arcsinh backend the host has, against the scalar curve.

use crate::image_ops::stretching::AsinhCurve;
use crate::image_ops::stretching::simd::asinh_color_preserve_scalar;
#[cfg(target_arch = "x86_64")]
use crate::image_ops::stretching::simd::avx2;
#[cfg(target_arch = "aarch64")]
use crate::image_ops::stretching::simd::neon;
use crate::testing::simd_check::backend::Backend;
use crate::testing::simd_check::simd_tier::SimdTier;
use crate::testing::simd_check::{SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar};

type PlanesFn = unsafe fn(&mut [f32], &mut [f32], &mut [f32], f32, f32);

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
