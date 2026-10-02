//! Every resample backend the host has, against the scalar path.

use std::array;

use crate::stacking::registration::resample::kernel;
#[cfg(target_arch = "x86_64")]
use crate::stacking::registration::resample::kernel::LANCZOS_LUT_RESOLUTION;
use crate::stacking::registration::resample::row;
#[cfg(target_arch = "aarch64")]
use crate::stacking::registration::resample::row::simd::neon;
#[cfg(target_arch = "x86_64")]
use crate::stacking::registration::resample::row::simd::x86;
use crate::stacking::registration::transform::{Transform, WarpTransform};
use crate::testing::prelude::*;
use crate::testing::simd_check::data_shape::DataShape;

type BilinearFn = unsafe fn(&Buffer2<f32>, &mut [f32], usize, &Transform);

const BILINEAR_BACKENDS: &[Backend<BilinearFn>] = &[
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Avx2, x86::bilinear_avx2),
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Sse41, x86::bilinear_sse),
    #[cfg(target_arch = "aarch64")]
    Backend::new(SimdTier::Neon, neon::bilinear_neon),
];

/// An image of `width × height` whose row `y` is the shape's row for seed `y`.
fn shape_image(shape: &DataShape, width: usize, height: usize) -> Buffer2<f32> {
    let pixels = (0..height).flat_map(|y| shape.row(width, y)).collect();
    Buffer2::new(width, height, pixels)
}

/// What a bilinear backend's error at `pos` is measured against.
///
/// The backends and the scalar path blend the four corners in the same order, so at one sample
/// point they agree exactly. They differ in the point: the backends map in f32 from f32
/// coefficients, the scalar path maps in f64 and rounds once. The point then moves by a few
/// roundings of the terms that make the coordinate, `|m₀x| + |m₁y| + |m₂|` (and likewise in y),
/// and the value by up to the corners' spread times that. The blend itself, evaluated at two
/// nearby points, rounds against the corners' magnitude. A point outside the footprint reads the
/// border, 0, on both paths, so its magnitude is 0 and the two must agree exactly.
fn bilinear_magnitude(input: &Buffer2<f32>, matrix: &[f64; 9], x: usize, y: usize) -> f32 {
    let (xf, yf) = (x as f64, y as f64);
    let src_x = matrix[0] * xf + matrix[1] * yf + matrix[2];
    let src_y = matrix[3] * xf + matrix[4] * yf + matrix[5];
    let pos = Vec2::new(src_x as f32, src_y as f32);
    let size = Size2us::new(input.width(), input.height());
    if !kernel::source_footprint_contains(pos, size) {
        return 0.0;
    }
    let clamped = kernel::clamp_to_pixel_centers(pos, size);
    let x0 = clamped.x.floor() as usize;
    let y0 = clamped.y.floor() as usize;
    let x1 = (x0 + 1).min(size.width - 1);
    let y1 = (y0 + 1).min(size.height - 1);
    let corners = [
        input[(x0, y0)],
        input[(x1, y0)],
        input[(x0, y1)],
        input[(x1, y1)],
    ];
    let largest = corners.iter().fold(0.0f32, |m, c| m.max(c.abs()));
    let spread = corners.iter().fold(f32::MIN, |m, &c| m.max(c))
        - corners.iter().fold(f32::MAX, |m, &c| m.min(c));
    let coordinate = (matrix[0] * xf).abs()
        + (matrix[1] * yf).abs()
        + matrix[2].abs()
        + (matrix[3] * xf).abs()
        + (matrix[4] * yf).abs()
        + matrix[5].abs();
    4.0 * largest + spread * coordinate as f32
}

/// Every bilinear backend, at every width (the backends have no minimum: a row shorter than one
/// vector is all remainder), on the top, middle and bottom rows, for an identity, a fractional
/// translation, two translations that put the first or the last pixel outside the footprint, and
/// a rotation with scale.
///
/// The longest chain of roundings in the point or the blend is six (coefficient, product, sum,
/// sum, quotient on the f32 side and the one f64 → f32 rounding on the other; three subtractions
/// and three products-and-sums in the blend), so they differ by the sum bound for six.
#[test]
fn bilinear_backends_match_scalar() {
    const HEIGHT: usize = 16;
    let transforms = [
        Transform::identity(),
        Transform::translation(DVec2::new(2.5, 1.7)),
        Transform::translation(DVec2::new(-0.75, 0.0)),
        Transform::translation(DVec2::new(0.75, 0.0)),
        Transform::similarity(DVec2::new(3.0, 2.0), 0.1, 1.05),
    ];
    for transform in &transforms {
        let inverse = transform.inverse();
        let warp = WarpTransform::new(inverse);
        assert_simd_matches_scalar(
            BILINEAR_BACKENDS,
            SWEEP_WIDTHS,
            ScalarSimd::sum_tolerance(6),
            |kernel_fn, shape, width| {
                let input = shape_image(shape, width, HEIGHT);
                let mut scalar = Vec::new();
                let mut simd = Vec::new();
                let mut magnitude = Vec::new();
                for y in [0, HEIGHT / 2, HEIGHT - 1] {
                    let mut scalar_row = vec![0.0f32; width];
                    let mut simd_row = vec![0.0f32; width];
                    row::bilinear_scalar(&input, &mut scalar_row, y, &warp, 0.0);
                    // SAFETY: the harness runs only backends whose tier this CPU has.
                    unsafe { kernel_fn(&input, &mut simd_row, y, &inverse) };
                    scalar.extend(scalar_row);
                    simd.extend(simd_row);
                    magnitude.extend(
                        (0..width).map(|x| bilinear_magnitude(&input, inverse.matrix(), x, y)),
                    );
                }
                ScalarSimd::of_sums(scalar, simd, magnitude)
            },
        );
    }
}

/// The scalar tap weights for `frac`, as `row::lanczos` builds them.
fn tap_weights<const A: usize, const SIZE: usize>(frac: f32) -> [f32; SIZE] {
    let lut = kernel::get_lanczos_lut(A);
    let a_minus_1 = A as i32 - 1;
    array::from_fn(|i| {
        if i < A {
            lut.lookup_positive((a_minus_1 - i as i32) as f32 + frac)
        } else {
            lut.lookup_positive((i as i32 - a_minus_1) as f32 - frac)
        }
    })
}

/// Every Lanczos window backend at every window position of a `width × SIZE` image, against the
/// plain weighted sum. A window term `p·wx·wy` passes through two products and the `SIZE² − 1`
/// additions, so the two orders differ by the sum bound for `SIZE² + 1`.
fn assert_lanczos_window_matches_scalar<const A: usize, const SIZE: usize>() {
    type WindowFn<const SIZE: usize> =
        unsafe fn(&[f32], usize, usize, usize, &[f32; SIZE], &[f32; SIZE]) -> f32;
    // Both backends read eight floats per row for SIZE > 4, so eight is the narrowest image.
    let backends: &[Backend<WindowFn<SIZE>>] = &[
        #[cfg(target_arch = "x86_64")]
        Backend::with_min_width(SimdTier::Avx2Fma, x86::lanczos_kernel_fma::<SIZE>, 8),
        #[cfg(target_arch = "aarch64")]
        Backend::with_min_width(SimdTier::Neon, neon::lanczos_kernel_neon::<SIZE>, 8),
    ];
    let wx = tap_weights::<A, SIZE>(0.3);
    let wy = tap_weights::<A, SIZE>(0.7);
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
    assert_lanczos_window_matches_scalar::<2, 4>();
    assert_lanczos_window_matches_scalar::<3, 6>();
    assert_lanczos_window_matches_scalar::<4, 8>();
}

/// The gathered tap weights equal the scalar lookups exactly, at every 1/1024 of a pixel and
/// just below 1: both index the same table by the same rounded distance.
#[cfg(target_arch = "x86_64")]
#[test]
fn lanczos_weight_gather_matches_scalar_lookups() {
    fn check<const A: usize, const SIZE: usize>() {
        if !SimdTier::Avx2Fma.runs_here() {
            return;
        }
        let lut = kernel::get_lanczos_lut(A);
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
            assert_eq!(
                gathered,
                tap_weights::<A, SIZE>(frac),
                "Lanczos{A} at {frac}"
            );
        }
    }
    check::<3, 6>();
    check::<4, 8>();
}
