//! Tests for SIMD convolution implementations.

#[cfg(target_arch = "aarch64")]
use crate::stacking::star_detection::convolution::simd::neon;
#[cfg(target_arch = "x86_64")]
use crate::stacking::star_detection::convolution::simd::x86;
use crate::stacking::star_detection::convolution::simd::{
    Kernel2d, convolve_2d_row, convolve_2d_row_scalar, convolve_cols_direct,
    convolve_cols_row_scalar, convolve_row, convolve_row_scalar, mirror_index,
};
use crate::testing::prelude::*;
use crate::testing::simd_check::data_shape::DataShape;

type RowFn = unsafe fn(&[f32], &mut [f32], &[f32], usize);
type ColsRowFn = unsafe fn(&[f32], &mut [f32], Size2us, usize, &[f32], usize);
type Row2dFn = unsafe fn(&[f32], &mut [f32], Size2us, usize, Kernel2d<'_>);

const ROW_BACKENDS: &[Backend<RowFn>] = &[
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Avx2Fma, x86::convolve_row_avx2),
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Sse41, x86::convolve_row_sse41),
    #[cfg(target_arch = "aarch64")]
    Backend::new(SimdTier::Neon, neon::convolve_row_neon),
];

const COLS_ROW_BACKENDS: &[Backend<ColsRowFn>] = &[
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Avx2Fma, x86::convolve_cols_row_avx2),
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Sse41, x86::convolve_cols_row_sse41),
    #[cfg(target_arch = "aarch64")]
    Backend::new(SimdTier::Neon, neon::convolve_cols_row_neon),
];

const ROW_2D_BACKENDS: &[Backend<Row2dFn>] = &[
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Avx2Fma, x86::convolve_2d_row_avx2),
    #[cfg(target_arch = "x86_64")]
    Backend::new(SimdTier::Sse41, x86::convolve_2d_row_sse41),
    #[cfg(target_arch = "aarch64")]
    Backend::new(SimdTier::Neon, neon::convolve_2d_row_neon),
];

/// An asymmetric kernel, so a tap applied in mirrored order shows.
fn asymmetric_kernel(radius: usize) -> Vec<f32> {
    (0..=2 * radius).map(|i| (i as f32 + 1.0) * 0.05).collect()
}

fn absolute(values: &[f32]) -> Vec<f32> {
    values.iter().map(|value| value.abs()).collect()
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

    for i in 0..5 {
        assert!(
            (output[i] - input[i]).abs() < 1e-6,
            "Identity kernel should preserve values"
        );
    }
}

#[test]
fn convolve_row_scalar_average() {
    let input = vec![0.0, 0.0, 3.0, 0.0, 0.0];
    let kernel = vec![1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0]; // Average kernel
    let mut output = vec![0.0; 5];

    convolve_row_scalar(&input, &mut output, &kernel, 1);

    // Center pixel should be 1.0 (3.0 / 3.0)
    assert!((output[2] - 1.0).abs() < 1e-6);
    // Neighbors should be 1.0 (3.0 / 3.0)
    assert!((output[1] - 1.0).abs() < 1e-6);
    assert!((output[3] - 1.0).abs() < 1e-6);
}

/// Every backend, at every radius through eight, over every shape at the sweep widths and at
/// every width across the alignments where the last vector lands on the mirrored edge (where an
/// off-by-one once over-read one element past the row). Radii above four are the NEON overshoot
/// case. A row needs more than `2r` samples for the mirror to stay inside it.
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
        let abs_kernel = absolute(&kernel);
        assert_simd_matches_scalar(
            ROW_BACKENDS,
            &widths,
            ScalarSimd::sum_tolerance(kernel.len()),
            |kernel_fn, shape, width| {
                let input = shape.row(width, 0);
                let mut scalar = vec![0.0f32; width];
                let mut simd = vec![0.0f32; width];
                let mut magnitude = vec![0.0f32; width];
                convolve_row_scalar(&input, &mut scalar, &kernel, radius);
                convolve_row_scalar(&absolute(&input), &mut magnitude, &abs_kernel, radius);
                // SAFETY: the harness runs only backends whose tier this CPU has.
                unsafe { kernel_fn(&input, &mut simd, &kernel, radius) };
                ScalarSimd::of_sums(scalar, simd, magnitude)
            },
        );
    }
}

/// The impulse response is the kernel *reversed*, which is what pins the tap order.
///
/// `convolve_row` correlates rather than convolves — `convolve_pixel_scalar` sums
/// `input[x + k - radius] · kernel[k]`, so an impulse at `p` puts `kernel[2r - t]` at column
/// `p - r + t`. That is the usual image-processing convention and production only ever passes
/// symmetric Gaussians, so it makes no difference there.
///
/// It makes every difference to this test. The test it replaces asserted the response equalled the
/// kernel *forwards*, and passed only because its kernel `[0.1, 0.2, 0.4, 0.2, 0.1]` is a
/// palindrome — it could not have distinguished either tap order. An asymmetric kernel can, which
/// is the whole point of an impulse oracle: the parity sweep above only says the two
/// implementations agree, so a kernel applied backwards would satisfy it.
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
            let expected = kernel[2 * radius - tap];
            assert!(
                (output[column] - expected).abs() < 1e-5,
                "{label}: column {column} should carry kernel[{}] = {expected}, got {}",
                2 * radius - tap,
                output[column]
            );
        }
        // Everything outside the kernel's reach stays zero.
        for (column, &value) in output.iter().enumerate() {
            if column < centre - radius || column > centre + radius {
                assert!(
                    value.abs() < 1e-6,
                    "{label}: column {column} should be zero, got {value}"
                );
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
    // Indices far out of bounds should clamp to valid range
    let len = 5;

    // Far negative: -5 would reflect to 5, but that's out of bounds, so clamp to 4
    assert!(mirror_index(-5, len) < len);
    assert!(mirror_index(-10, len) < len);

    // Far positive: large indices should also stay in bounds
    assert!(mirror_index(20, len) < len);
    assert!(mirror_index(100, len) < len);

    // All results must be valid indices
    for i in -20..30 {
        let result = mirror_index(i, len);
        assert!(
            result < len,
            "mirror_index({i}, {len}) = {result} should be < {len}"
        );
    }
}

#[test]
fn convolve_cols_uniform_input() {
    let width = 32;
    let height = 32;
    let input = vec![42.0f32; width * height];
    let kernel = vec![0.1, 0.2, 0.4, 0.2, 0.1]; // Sums to 1.0
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
            (v - 42.0).abs() < 1e-5,
            "Uniform input should stay uniform at {i}: {v}"
        );
    }
}

#[test]
fn convolve_cols_impulse_response() {
    let width = 8;
    let height = 16;
    let mut input = vec![0.0f32; width * height];
    // Single impulse at (4, 8)
    input[8 * width + 4] = 1.0;

    // Use odd-sized kernel (radius = ksize/2)
    let kernel = vec![0.1, 0.2, 0.4, 0.2, 0.1];
    let radius = kernel.len() / 2;

    let mut output = vec![0.0f32; width * height];
    convolve_cols_direct(
        &input,
        &mut output,
        Size2us::new(width, height),
        &kernel,
        radius,
    );

    // Check vertical spread at column 4
    // The impulse at y=8 spreads to y-radius..y+radius
    for (ky, &kval) in kernel.iter().enumerate() {
        let y = (8_isize + ky as isize - radius as isize) as usize;
        assert!(
            (output[y * width + 4] - kval).abs() < 1e-5,
            "Impulse response at y={}: {} vs expected {}",
            y,
            output[y * width + 4],
            kval
        );
    }

    // Other columns should be zero
    for x in 0..width {
        if x != 4 {
            for y in 0..height {
                assert!(
                    output[y * width + x].abs() < 1e-6,
                    "Non-impulse column should be zero at ({x}, {y})"
                );
            }
        }
    }
}

/// Every column backend against the scalar reference, on images whose rows are the sweep's
/// shapes: the whole output, so the mirrored top and bottom rows are compared with the interior.
#[test]
fn convolve_cols_backends_match_scalar() {
    for radius in [1, 2, 3, 5] {
        let kernel = asymmetric_kernel(radius);
        let abs_kernel = absolute(&kernel);
        let height = 2 * radius + 6;
        assert_simd_matches_scalar(
            COLS_ROW_BACKENDS,
            SWEEP_WIDTHS,
            ScalarSimd::sum_tolerance(kernel.len()),
            |kernel_fn, shape, width| {
                let size = Size2us::new(width, height);
                let input = shape_image(shape, size);
                let abs_input = absolute(&input);
                let mut scalar = vec![0.0f32; width * height];
                let mut simd = vec![0.0f32; width * height];
                let mut magnitude = vec![0.0f32; width * height];
                for y in 0..height {
                    let row = y * width..(y + 1) * width;
                    let scalar_row = &mut scalar[row.clone()];
                    convolve_cols_row_scalar(&input, scalar_row, size, y, &kernel, radius);
                    let magnitude_row = &mut magnitude[row.clone()];
                    convolve_cols_row_scalar(
                        &abs_input,
                        magnitude_row,
                        size,
                        y,
                        &abs_kernel,
                        radius,
                    );
                    // SAFETY: the harness runs only backends whose tier this CPU has.
                    unsafe { kernel_fn(&input, &mut simd[row], size, y, &kernel, radius) };
                }
                ScalarSimd::of_sums(scalar, simd, magnitude)
            },
        );
    }
}

/// Every 2D backend against the scalar reference, for each odd kernel size through nine, on the
/// two mirrored rows at each edge and one interior row.
#[test]
fn convolve_2d_row_backends_match_scalar() {
    for ksize in [3, 5, 7, 9] {
        let radius = ksize / 2;
        let weights: Vec<f32> = (0..ksize * ksize)
            .map(|i| (i as f32 + 1.0) * 0.01)
            .collect();
        let kernel = Kernel2d::new(&weights, ksize);
        // The weights are positive, so they are their own absolute values.
        let height = 2 * radius + 6;
        let widths: Vec<usize> = SWEEP_WIDTHS
            .iter()
            .copied()
            .filter(|&width| width > 2 * radius)
            .collect();
        assert_simd_matches_scalar(
            ROW_2D_BACKENDS,
            &widths,
            ScalarSimd::sum_tolerance(weights.len()),
            |kernel_fn, shape, width| {
                let size = Size2us::new(width, height);
                let input = shape_image(shape, size);
                let abs_input = absolute(&input);
                let mut scalar = Vec::new();
                let mut simd = Vec::new();
                let mut magnitude = Vec::new();
                for y in [0, 1, height / 2, height - 2, height - 1] {
                    let mut scalar_row = vec![0.0f32; width];
                    let mut simd_row = vec![0.0f32; width];
                    let mut magnitude_row = vec![0.0f32; width];
                    convolve_2d_row_scalar(&input, &mut scalar_row, size, y, kernel);
                    convolve_2d_row_scalar(&abs_input, &mut magnitude_row, size, y, kernel);
                    // SAFETY: the harness runs only backends whose tier this CPU has.
                    unsafe { kernel_fn(&input, &mut simd_row, size, y, kernel) };
                    scalar.extend(scalar_row);
                    simd.extend(simd_row);
                    magnitude.extend(magnitude_row);
                }
                ScalarSimd::of_sums(scalar, simd, magnitude)
            },
        );
    }
}

#[test]
fn convolve_2d_row_uniform() {
    let width = 16;
    let height = 16;
    let input = vec![42.0f32; width * height];

    // Normalized 5x5 kernel
    let weights = vec![1.0 / 25.0; 25];
    let kernel = Kernel2d::new(&weights, 5);

    for y in 0..height {
        let mut output = vec![0.0f32; width];
        convolve_2d_row(&input, &mut output, Size2us::new(width, height), y, kernel);

        for (x, &v) in output.iter().enumerate() {
            assert!(
                (v - 42.0).abs() < 1e-4,
                "Uniform input should stay uniform at row {y} x {x}: {v}"
            );
        }
    }
}

#[test]
fn convolve_2d_row_impulse() {
    let width = 16;
    let height = 16;
    let mut input = vec![0.0f32; width * height];
    // Impulse at (8, 8)
    input[8 * width + 8] = 1.0;

    // 3x3 identity-ish kernel
    let weights = vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
    let kernel = Kernel2d::new(&weights, 3);

    let mut output = vec![0.0f32; width];
    convolve_2d_row(&input, &mut output, Size2us::new(width, height), 8, kernel);

    // Only position 8 should have value 1.0
    assert!(
        (output[8] - 1.0).abs() < 1e-6,
        "Impulse should pass through"
    );
    for (x, &v) in output.iter().enumerate() {
        if x != 8 {
            assert!(v.abs() < 1e-6, "Non-impulse position should be zero");
        }
    }
}
