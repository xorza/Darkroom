//! Tests for SIMD convolution implementations.

#![expect(
    clippy::cast_possible_wrap,
    reason = "test fixtures are small images, with non-negative coordinates and offsets of a few dozen pixels"
)]

use crate::internals::prelude::*;
use crate::internals::simd_check::backend::Backend;
use crate::internals::simd_check::data_shape::DataShape;
use crate::internals::simd_check::{SWEEP_WIDTHS, ScalarSimd, assert_simd_matches_scalar};
#[cfg(target_arch = "aarch64")]
use crate::star_detection::convolution::simd::neon;
use crate::star_detection::convolution::simd::{
    Kernel2d, convolve_2d_row, convolve_2d_row_scalar, convolve_cols_direct,
    convolve_cols_row_scalar, convolve_row, convolve_row_scalar, mirror_index,
};
#[cfg(target_arch = "x86_64")]
use crate::star_detection::convolution::simd::{avx2, sse41};
use imaginarium::SimdTier;

type RowFn = unsafe fn(&[f32], &mut [f32], &[f32], usize);
type ColsRowFn = unsafe fn(&[f32], &mut [f32], Size2us, usize, &[f32], usize);
type Row2dFn = unsafe fn(&[f32], &mut [f32], Size2us, usize, Kernel2d<'_>);

const ROW_BACKENDS: &[Backend<RowFn>] = &[
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Avx2, avx2::convolve_row_avx2),
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Sse41, sse41::convolve_row_sse41),
    #[cfg(target_arch = "aarch64")]
    Backend::new(SimdTier::Neon, neon::convolve_row_neon),
];

const COLS_ROW_BACKENDS: &[Backend<ColsRowFn>] = &[
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Avx2, avx2::convolve_cols_row_avx2),
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Sse41, sse41::convolve_cols_row_sse41),
    #[cfg(target_arch = "aarch64")]
    Backend::new(SimdTier::Neon, neon::convolve_cols_row_neon),
];

const ROW_2D_BACKENDS: &[Backend<Row2dFn>] = &[
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Avx2, avx2::convolve_2d_row_avx2),
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Sse41, sse41::convolve_2d_row_sse41),
    #[cfg(target_arch = "aarch64")]
    Backend::new(SimdTier::Neon, neon::convolve_2d_row_neon),
];

/// An asymmetric kernel, so a tap applied in mirrored order shows.
fn asymmetric_kernel(radius: usize) -> Vec<f32> {
    (0..=2 * radius).map(|i| (i as f32 + 1.0) * 0.05).collect()
}

/// An image of `size` whose row `y` is the shape's row for seed `y`.
fn shape_image(shape: &DataShape, size: Size2us) -> Vec<f32> {
    (0..size.height)
        .flat_map(|y| shape.row(size.width, y))
        .collect()
}

#[test]
fn convolve_row_scalar_identity() {
    let input = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let kernel = vec![0.0, 1.0, 0.0]; // Identity kernel
    let mut output = vec![0.0; 5];

    convolve_row_scalar(&input, &mut output, &kernel, 1);

    // 0·a + 1·b + 0·c is b exactly.
    assert_eq!(output, input);
}

#[test]
fn convolve_row_scalar_average() {
    let input = vec![0.0, 0.0, 3.0, 0.0, 0.0];
    let kernel = vec![1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0]; // Average kernel
    let mut output = vec![0.0; 5];

    convolve_row_scalar(&input, &mut output, &kernel, 1);

    // One nonzero term each: 3 · fl(1/3) rounds back to exactly 1.
    assert_eq!(output, [0.0, 1.0, 1.0, 1.0, 0.0]);
}

/// Every backend, at every radius through eight, over every shape at the sweep widths and at
/// every width across the alignments where the last vector lands on the mirrored edge. A row needs
/// more than `2r` samples for the mirror to stay inside it.
///
/// Every backend accumulates the taps in the scalar order with an unfused multiply then add, so
/// each lane is the scalar result bit for bit: the tolerance is zero.
#[test]
fn convolve_row_backends_match_scalar() {
    for radius in 1..=8 {
        let kernel = asymmetric_kernel(radius);
        let widths: Vec<usize> = SWEEP_WIDTHS
            .iter()
            .copied()
            .chain(2 * radius + 8..2 * radius + 48)
            .filter(|&width| width > 2 * radius)
            .collect();
        assert_simd_matches_scalar(ROW_BACKENDS, &widths, 0.0, |kernel_fn, shape, width| {
            let input = shape.row(width, 0);
            let mut scalar = vec![0.0f32; width];
            let mut simd = vec![0.0f32; width];
            convolve_row_scalar(&input, &mut scalar, &kernel, radius);
            // SAFETY: the harness runs only backends whose tier this CPU has.
            unsafe { kernel_fn(&input, &mut simd, &kernel, radius) };
            ScalarSimd::new(scalar, simd)
        });
    }
}

/// The impulse response is the kernel *reversed*, which is what pins the tap order.
///
/// `convolve_row` correlates rather than convolves — `convolve_pixel_scalar` sums
/// `input[x + k - radius] · kernel[k]`, so an impulse at `p` puts `kernel[2r - t]` at column
/// `p - r + t`. That is the usual image-processing convention and production only ever passes
/// symmetric Gaussians, so it makes no difference there. It is why the kernel here is asymmetric:
/// a palindrome could not tell the two tap orders apart, and the parity sweep only says the
/// implementations agree. One tap meets the impulse, so each output is that weight exactly.
#[test]
fn convolve_row_impulse_response_is_the_reversed_kernel() {
    let width = 64;
    let radius = 2;
    let kernel = [0.1f32, 0.2, 0.4, 0.3, 0.05];
    let centre = width / 2;
    let mut input = vec![0.0f32; width];
    input[centre] = 1.0;

    // Both implementations, because an interior impulse takes the SIMD path — checking only
    // `convolve_row` would leave the scalar reference's own tap order unpinned, and the parity
    // sweep cannot tell which of the two is right.
    let mut simd = vec![0.0f32; width];
    let mut scalar = vec![0.0f32; width];
    convolve_row(&input, &mut simd, &kernel, radius);
    convolve_row_scalar(&input, &mut scalar, &kernel, radius);

    for (label, output) in [("simd", &simd), ("scalar", &scalar)] {
        for tap in 0..kernel.len() {
            let column = centre - radius + tap;
            assert_eq!(
                output[column],
                kernel[2 * radius - tap],
                "{label}: column {column}"
            );
        }
        // Everything outside the kernel's reach stays zero.
        for (column, &value) in output.iter().enumerate() {
            if column < centre - radius || column > centre + radius {
                assert_eq!(value, 0.0, "{label}: column {column}");
            }
        }
    }
}

#[test]
fn mirror_index_in_bounds() {
    // In-bounds indices should pass through unchanged
    for len in [5, 10, 100] {
        for i in 0..len {
            assert_eq!(mirror_index(i as isize, len), i);
        }
    }
}

#[test]
fn mirror_index_negative() {
    // Negative indices should reflect: -1 -> 1, -2 -> 2, etc.
    let len = 10;
    assert_eq!(mirror_index(-1, len), 1);
    assert_eq!(mirror_index(-2, len), 2);
    assert_eq!(mirror_index(-3, len), 3);
}

#[test]
fn mirror_index_overflow() {
    // Indices >= len should reflect: len -> len-2, len+1 -> len-3, etc.
    let len = 10;
    assert_eq!(mirror_index(10, len), 8); // 2*10-2-10 = 8
    assert_eq!(mirror_index(11, len), 7); // 2*10-2-11 = 7
    assert_eq!(mirror_index(12, len), 6); // 2*10-2-12 = 6
}

#[test]
fn mirror_index_far_out_of_bounds() {
    // Past one reflection the index clamps: −5 reflects to 5 and clamps to the last pixel, 4;
    // 2·5 − 2 − 20 saturates to 0.
    let len = 5;
    assert_eq!(mirror_index(-5, len), 4);
    assert_eq!(mirror_index(-10, len), 4);
    assert_eq!(mirror_index(20, len), 0);
    assert_eq!(mirror_index(100, len), 0);
    for i in -20..30 {
        assert!(mirror_index(i, len) < len, "mirror_index({i}, {len})");
    }
}

#[test]
fn convolve_cols_uniform_input() {
    // 42 times weights summing to 1 within their own rounding: five products and five sums,
    // 10ε relative.
    let width = 32;
    let height = 32;
    let input = vec![42.0f32; width * height];
    let kernel = vec![0.1, 0.2, 0.4, 0.2, 0.1];
    let radius = 2;

    let mut output = vec![0.0f32; width * height];
    convolve_cols_direct(
        &input,
        &mut output,
        Size2us::new(width, height),
        &kernel,
        radius,
    );

    for (i, &v) in output.iter().enumerate() {
        assert!(
            (v - 42.0).abs() <= 42.0 * 10.0 * f32::EPSILON,
            "Uniform input should stay uniform at {i}: {v}"
        );
    }
}

#[test]
fn convolve_cols_impulse_response() {
    // As in the row case: an impulse at (4, 8) puts `kernel[2r − t]` at row `8 − r + t` of its
    // column, exactly, and nothing anywhere else. The kernel is asymmetric so the tap order shows.
    let width = 8;
    let height = 16;
    let mut input = vec![0.0f32; width * height];
    input[8 * width + 4] = 1.0;
    let kernel = [0.1f32, 0.2, 0.4, 0.3, 0.05];
    let radius = kernel.len() / 2;

    let mut output = vec![0.0f32; width * height];
    convolve_cols_direct(
        &input,
        &mut output,
        Size2us::new(width, height),
        &kernel,
        radius,
    );

    for y in 0..height {
        for x in 0..width {
            let expected = if x == 4 && (8 - radius..=8 + radius).contains(&y) {
                kernel[2 * radius - (y + radius - 8)]
            } else {
                0.0
            };
            assert_eq!(output[y * width + x], expected, "({x}, {y})");
        }
    }
}

/// Every column backend against the scalar reference, on images whose rows are the sweep's
/// shapes: the whole output, so the mirrored top and bottom rows are compared with the interior.
/// Bit for bit, as for rows.
#[test]
fn convolve_cols_backends_match_scalar() {
    for radius in [1, 2, 3, 5] {
        let kernel = asymmetric_kernel(radius);
        let height = 2 * radius + 6;
        assert_simd_matches_scalar(
            COLS_ROW_BACKENDS,
            SWEEP_WIDTHS,
            0.0,
            |kernel_fn, shape, width| {
                let size = Size2us::new(width, height);
                let input = shape_image(shape, size);
                let mut scalar = vec![0.0f32; width * height];
                let mut simd = vec![0.0f32; width * height];
                for y in 0..height {
                    let row = y * width..(y + 1) * width;
                    convolve_cols_row_scalar(
                        &input,
                        &mut scalar[row.clone()],
                        size,
                        y,
                        &kernel,
                        radius,
                    );
                    // SAFETY: the harness runs only backends whose tier this CPU has.
                    unsafe { kernel_fn(&input, &mut simd[row], size, y, &kernel, radius) };
                }
                ScalarSimd::new(scalar, simd)
            },
        );
    }
}

/// Every 2D backend against the scalar reference, for each odd kernel size through nine, on the
/// two mirrored rows at each edge and one interior row. Bit for bit, as for rows; the weights
/// rise across the kernel, so no reflection of it is itself.
#[test]
fn convolve_2d_row_backends_match_scalar() {
    for ksize in [3, 5, 7, 9] {
        let radius = ksize / 2;
        let weights: Vec<f32> = (0..ksize * ksize)
            .map(|i| (i as f32 + 1.0) * 0.01)
            .collect();
        let kernel = Kernel2d::new(&weights, ksize);
        let height = 2 * radius + 6;
        let widths: Vec<usize> = SWEEP_WIDTHS
            .iter()
            .copied()
            .filter(|&width| width > 2 * radius)
            .collect();
        assert_simd_matches_scalar(ROW_2D_BACKENDS, &widths, 0.0, |kernel_fn, shape, width| {
            let size = Size2us::new(width, height);
            let input = shape_image(shape, size);
            let mut scalar = Vec::new();
            let mut simd = Vec::new();
            for y in [0, 1, height / 2, height - 2, height - 1] {
                let mut scalar_row = vec![0.0f32; width];
                let mut simd_row = vec![0.0f32; width];
                convolve_2d_row_scalar(&input, &mut scalar_row, size, y, kernel);
                // SAFETY: the harness runs only backends whose tier this CPU has.
                unsafe { kernel_fn(&input, &mut simd_row, size, y, kernel) };
                scalar.extend(scalar_row);
                simd.extend(simd_row);
            }
            ScalarSimd::new(scalar, simd)
        });
    }
}

#[test]
fn convolve_2d_row_uniform() {
    // 42 times 25 weights of 1/25: 25 products and 25 sums, 50ε relative.
    let width = 16;
    let height = 16;
    let input = vec![42.0f32; width * height];
    let weights = vec![1.0 / 25.0; 25];
    let kernel = Kernel2d::new(&weights, 5);

    for y in 0..height {
        let mut output = vec![0.0f32; width];
        convolve_2d_row(&input, &mut output, Size2us::new(width, height), y, kernel);
        for (x, &v) in output.iter().enumerate() {
            assert!(
                (v - 42.0).abs() <= 42.0 * 50.0 * f32::EPSILON,
                "row {y} x {x}: {v}"
            );
        }
    }
}

#[test]
fn convolve_2d_row_impulse_response() {
    // An impulse at (8, 8) through a 3×3 kernel of distinct weights: row `8 + dy` carries the
    // kernel's row `1 − dy` reversed, exactly — both axes correlate, as `convolve_row` does.
    let width = 16;
    let height = 16;
    let mut input = vec![0.0f32; width * height];
    input[8 * width + 8] = 1.0;
    let weights: Vec<f32> = (1..=9).map(|w| w as f32 * 0.1).collect();
    let kernel = Kernel2d::new(&weights, 3);

    for y in 0..height {
        let mut output = vec![0.0f32; width];
        convolve_2d_row(&input, &mut output, Size2us::new(width, height), y, kernel);
        for (x, &value) in output.iter().enumerate() {
            let expected = if (7..=9).contains(&y) && (7..=9).contains(&x) {
                kernel.at(9 - y, 9 - x)
            } else {
                0.0
            };
            assert_eq!(value, expected, "({x}, {y})");
        }
    }
}
