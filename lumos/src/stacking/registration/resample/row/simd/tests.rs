//! Every resample backend the host has, against the scalar path.

#[cfg(target_arch = "x86_64")]
use crate::stacking::registration::resample::kernel::LANCZOS_LUT_RESOLUTION;
use crate::stacking::registration::resample::kernel::LanczosOrder;
#[cfg(target_arch = "aarch64")]
use crate::stacking::registration::resample::row::simd::neon;
#[cfg(target_arch = "x86_64")]
use crate::stacking::registration::resample::row::simd::x86;
use crate::testing::simd_check::backend::Backend;
use imaginarium::SimdTier;

#[cfg(target_arch = "x86_64")]
use crate::testing::simd_check;
use crate::testing::simd_check::{SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar};

/// The scalar tap weights for `frac`: the table's own `weights`.
fn tap_weights<const SIZE: usize>(frac: f32) -> [f32; SIZE] {
    order_of::<SIZE>().lut().weights::<SIZE>(frac)
}

/// The order whose `2a`-tap window is `SIZE` wide.
fn order_of<const SIZE: usize>() -> LanczosOrder {
    match SIZE {
        4 => LanczosOrder::Two,
        6 => LanczosOrder::Three,
        _ => LanczosOrder::Four,
    }
}

/// Every Lanczos window backend at every window position of a `width × SIZE` image, against the
/// plain weighted sum. A window term `p·wx·wy` passes through two products and the `SIZE² − 1`
/// additions, so the two orders differ by the sum bound for `SIZE² + 1`.
fn assert_lanczos_window_matches_scalar<const SIZE: usize>() {
    type WindowFn<const SIZE: usize> =
        unsafe fn(&[f32], usize, usize, usize, &[f32; SIZE], &[f32; SIZE]) -> f32;
    // Both backends read eight floats per row for SIZE > 4, so eight is the narrowest image.
    let backends: &[Backend<WindowFn<SIZE>>] = &[
        #[cfg(target_arch = "x86_64")]
        Backend::with_min_width(SimdTier::Avx2Fma, x86::lanczos_kernel_fma::<SIZE>, 8),
        #[cfg(target_arch = "aarch64")]
        Backend::with_min_width(SimdTier::Neon, neon::lanczos_kernel_neon::<SIZE>, 8),
    ];
    let wx = tap_weights::<SIZE>(0.3);
    let wy = tap_weights::<SIZE>(0.7);
    assert_simd_matches_scalar(
        backends,
        SWEEP_WIDTHS,
        ScalarSimd::sum_tolerance(SIZE * SIZE + 1),
        |kernel_fn, shape, width| {
            let pixels: Vec<f32> = (0..SIZE).flat_map(|y| shape.row(width, y)).collect();
            let windows = 0..=width - 8;
            let mut scalar = Vec::new();
            let mut magnitude = Vec::new();
            for kx in windows.clone() {
                let mut sum = 0.0f32;
                let mut absolute = 0.0f32;
                for (j, &wyj) in wy.iter().enumerate() {
                    for (k, &wxk) in wx.iter().enumerate() {
                        let term = pixels[j * width + kx + k] * wxk * wyj;
                        sum += term;
                        absolute += term.abs();
                    }
                }
                scalar.push(sum);
                magnitude.push(absolute);
            }
            // SAFETY: the harness runs only backends whose tier this CPU has, and every window
            // reads columns `kx..kx + 8` of rows `0..SIZE`, inside the image.
            let simd = windows
                .map(|kx| unsafe { kernel_fn(&pixels, width, kx, 0, &wx, &wy) })
                .collect();
            ScalarSimd::of_sums(scalar, simd, magnitude)
        },
    );
}

#[test]
fn lanczos_window_backends_match_scalar() {
    assert_lanczos_window_matches_scalar::<4>();
    assert_lanczos_window_matches_scalar::<6>();
    assert_lanczos_window_matches_scalar::<8>();
}

/// The gathered tap weights equal the scalar lookups exactly, at every 1/1024 of a pixel and
/// just below 1: both index the same table by the same rounded distance.
#[cfg(target_arch = "x86_64")]
#[test]
fn lanczos_weight_gather_matches_scalar_lookups() {
    fn check<const A: usize, const SIZE: usize>() {
        if !simd_check::runs_here(SimdTier::Avx2Fma) {
            return;
        }
        let lut = order_of::<SIZE>().lut();
        let fracs = (0..1024)
            .map(|k| k as f32 / 1024.0)
            .chain([1.0 - f32::EPSILON / 2.0]);
        for frac in fracs {
            // SAFETY: the CPU has AVX2 and FMA, and the table holds `A·RES + 1` entries.
            let gathered = unsafe {
                x86::lanczos_weights_gather::<A, SIZE>(
                    lut.values.as_ptr(),
                    LANCZOS_LUT_RESOLUTION as f32,
                    frac,
                )
            };
            assert_eq!(gathered, tap_weights::<SIZE>(frac), "Lanczos{A} at {frac}");
        }
    }
    check::<3, 6>();
    check::<4, 8>();
}
