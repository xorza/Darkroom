//! Tests for SIMD convolution implementations.

use crate::stacking::star_detection::convolution::simd::{
    Kernel2d, convolve_2d_row, convolve_2d_row_scalar, convolve_cols_direct,
    convolve_cols_row_scalar, convolve_row, convolve_row_scalar, mirror_index,
};
use crate::testing::prelude::*;

/// Whole-image scalar column convolution — a parity oracle for [`convolve_cols_direct`] (production
/// uses the parallel path; this serial loop over `convolve_cols_row_scalar` is the reference).
fn convolve_cols_scalar_ref(
    input: &[f32],
    output: &mut [f32],
    size: Size2us,
    kernel: &[f32],
    radius: usize,
) {
    for y in 0..size.height {
        convolve_cols_row_scalar(
            input,
            &mut output[y * size.width..(y + 1) * size.width],
            size,
            y,
            kernel,
            radius,
        );
    }
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

/// Every data shape against the scalar reference, at three radii. Replaces five tests that each
/// pinned one shape at one radius; the dense width/radius sweep below covers a different axis and
/// stays as its own regression guard.
#[test]
fn convolve_row_simd_matches_scalar_across_shapes() {
    for radius in [1usize, 2, 3] {
        let ksize = 2 * radius + 1;
        // Asymmetric, so a mirrored-tap error cannot be masked by symmetry.
        let kernel: Vec<f32> = (0..ksize).map(|i| (i as f32 + 1.0) * 0.05).collect();
        let widths: Vec<usize> = SWEEP_WIDTHS
            .iter()
            .copied()
            .filter(|&w| w > 2 * radius)
            .collect();
        assert_simd_matches_scalar(&widths, 1e-3, |shape, width| {
            let input = shape.row(width, 0);
            let mut simd = vec![0.0f32; width];
            let mut scalar = vec![0.0f32; width];
            convolve_row(&input, &mut simd, &kernel, radius);
            convolve_row_scalar(&input, &mut scalar, &kernel, radius);
            ScalarSimd { scalar, simd }
        });
    }
}

#[test]
fn convolve_row_simd_matches_scalar_width_radius_sweep() {
    // Regression guard for the row-kernel boundary off-by-one: the SIMD interior must equal the
    // mirrored scalar reference at *every* width. The old bug only surfaced when the final SIMD
    // iteration landed exactly on the safe-region edge (a width/radius alignment) — it over-read
    // one element past the row and skipped mirroring at column `width - radius`. Sweep widths
    // across those alignments for several radii, including radius > 4 (the NEON-overshoot case).
    for radius in [1usize, 2, 3, 5, 7] {
        let ksize = 2 * radius + 1;
        // Asymmetric kernel so a boundary tap error can't be masked by symmetry.
        let kernel: Vec<f32> = (0..ksize).map(|i| (i as f32 + 1.0) * 0.05).collect();
        for width in (2 * radius + 8)..(2 * radius + 48) {
            let input: Vec<f32> = (0..width).map(|i| (i as f32 * 0.37).sin() + 1.0).collect();
            let mut simd = vec![0.0f32; width];
            let mut scalar = vec![0.0f32; width];
            convolve_row(&input, &mut simd, &kernel, radius);
            convolve_row_scalar(&input, &mut scalar, &kernel, radius);
            for x in 0..width {
                assert!(
                    (simd[x] - scalar[x]).abs() < 1e-3,
                    "row SIMD != scalar at radius={radius} width={width} x={x}: {} vs {}",
                    simd[x],
                    scalar[x]
                );
            }
        }
    }
}

/// One slice of the row-convolution parity surface: SIMD against the scalar reference.
#[derive(Debug)]
struct RowParityCase {
    name: &'static str,
    widths: Vec<usize>,
    radii: Vec<usize>,
    /// The signal, built for a width. The shape matters: a smooth ramp, a wave and an impulse
    /// stress the mirrored boundary differently.
    input: fn(usize) -> Vec<f32>,
    tolerance: f32,
}

/// SIMD and scalar row convolution agree over every width, radius and signal shape that mattered.
///
/// Five separate tests each pinned one corner of this surface — widths below a register, one wide
/// row, radii one through eight, a hand-built boundary case, and a three-element row. They are one
/// table because they assert the same property and differ only in where they sample it.
///
/// Every row uses the same *asymmetric* kernel, `(i + 1)·0.05`. Three of the tests this replaces
/// used a symmetric box, which cannot catch a kernel applied in mirrored tap order — the class of
/// bug this parity check exists for.
#[test]
fn convolve_row_simd_matches_scalar_over_every_width_and_radius() {
    fn ramp(width: usize) -> Vec<f32> {
        (0..width).map(|i| i as f32 + 1.0).collect()
    }
    fn half_ramp(width: usize) -> Vec<f32> {
        (0..width).map(|i| i as f32 * 0.5).collect()
    }
    fn wave(width: usize) -> Vec<f32> {
        (0..width).map(|i| (i as f32).sin() * 10.0 + 50.0).collect()
    }
    fn impulse(width: usize) -> Vec<f32> {
        let mut input = vec![0.0f32; width];
        input[width / 2] = 1.0;
        input
    }

    let cases = [
        // Below one SIMD register the kernel never enters its vector path. Three is the narrowest
        // a radius-1 mirror can work on, so it is the floor.
        RowParityCase {
            name: "rows narrower than a SIMD register",
            widths: (3..16).collect(),
            radii: vec![1],
            input: ramp,
            tolerance: 1e-5,
        },
        // A row long enough for many vector iterations, with a wide kernel over a signal that
        // changes sign of slope repeatedly.
        RowParityCase {
            name: "a long row with a wide kernel",
            widths: vec![100],
            radii: vec![5],
            input: wave,
            tolerance: 1e-4,
        },
        // Radii four, six and eight are not in the dense sweep's {1,2,3,5,7}, so this is the only
        // place they are exercised.
        RowParityCase {
            name: "every radius through eight",
            widths: vec![64],
            radii: (1..=8).collect(),
            input: half_ramp,
            tolerance: 1e-4,
        },
        // An impulse puts the whole kernel on the boundary between zero and non-zero output,
        // where a tap misalignment shows as a shifted response rather than a small error.
        RowParityCase {
            name: "an impulse",
            widths: vec![64],
            radii: vec![2],
            input: impulse,
            tolerance: 1e-5,
        },
    ];

    for case in &cases {
        for &radius in &case.radii {
            let kernel: Vec<f32> = (0..2 * radius + 1)
                .map(|i| (i as f32 + 1.0) * 0.05)
                .collect();
            for &width in &case.widths {
                let input = (case.input)(width);
                let mut simd = vec![0.0f32; width];
                let mut scalar = vec![0.0f32; width];
                convolve_row(&input, &mut simd, &kernel, radius);
                convolve_row_scalar(&input, &mut scalar, &kernel, radius);

                for x in 0..width {
                    assert!(
                        (simd[x] - scalar[x]).abs() < case.tolerance,
                        "{}: radius {radius} width {width} column {x}: {} vs {}",
                        case.name,
                        simd[x],
                        scalar[x]
                    );
                }
            }
        }
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
            "mirror_index({}, {}) = {} should be < {}",
            i,
            len,
            result,
            len
        );
    }
}

#[test]
fn convolve_cols_matches_scalar() {
    let width = 16;
    let height = 32;
    let input: Vec<f32> = (0..width * height)
        .map(|i| (i as f32 * 0.1).sin())
        .collect();
    let kernel = vec![0.1, 0.2, 0.4, 0.2, 0.1];
    let radius = 2;

    let mut output_simd = vec![0.0f32; width * height];
    let mut output_scalar = vec![0.0f32; width * height];

    convolve_cols_direct(
        &input,
        &mut output_simd,
        Size2us::new(width, height),
        &kernel,
        radius,
    );
    convolve_cols_scalar_ref(
        &input,
        &mut output_scalar,
        Size2us::new(width, height),
        &kernel,
        radius,
    );

    for i in 0..width * height {
        assert!(
            (output_simd[i] - output_scalar[i]).abs() < 1e-5,
            "Column convolution mismatch at {}: {} vs {}",
            i,
            output_simd[i],
            output_scalar[i]
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
            "Uniform input should stay uniform at {}: {}",
            i,
            v
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
                    "Non-impulse column should be zero at ({}, {})",
                    x,
                    y
                );
            }
        }
    }
}

#[test]
fn convolve_cols_various_sizes() {
    for (width, height) in [(8, 8), (16, 32), (64, 16), (100, 100)] {
        let input: Vec<f32> = (0..width * height).map(|i| i as f32 * 0.01).collect();
        let kernel = vec![0.25, 0.5, 0.25];
        let radius = 1;

        let mut output_simd = vec![0.0f32; width * height];
        let mut output_scalar = vec![0.0f32; width * height];

        convolve_cols_direct(
            &input,
            &mut output_simd,
            Size2us::new(width, height),
            &kernel,
            radius,
        );
        convolve_cols_scalar_ref(
            &input,
            &mut output_scalar,
            Size2us::new(width, height),
            &kernel,
            radius,
        );

        for i in 0..width * height {
            assert!(
                (output_simd[i] - output_scalar[i]).abs() < 1e-4,
                "Size {}x{} mismatch at {}: {} vs {}",
                width,
                height,
                i,
                output_simd[i],
                output_scalar[i]
            );
        }
    }
}

#[test]
fn convolve_2d_row_matches_scalar() {
    let width = 32;
    let height = 32;
    let input: Vec<f32> = (0..width * height)
        .map(|i| (i as f32 * 0.1).sin())
        .collect();

    // Gaussian-like 3x3 kernel, sums to 1.0
    let weights = vec![
        0.0625, 0.125, 0.0625, 0.125, 0.25, 0.125, 0.0625, 0.125, 0.0625,
    ];
    let kernel = Kernel2d::new(&weights, 3);

    for y in 0..height {
        let mut output_simd = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];

        convolve_2d_row(
            &input,
            &mut output_simd,
            Size2us::new(width, height),
            y,
            kernel,
        );
        convolve_2d_row_scalar(
            &input,
            &mut output_scalar,
            Size2us::new(width, height),
            y,
            kernel,
        );

        for x in 0..width {
            assert!(
                (output_simd[x] - output_scalar[x]).abs() < 1e-5,
                "2D row {} mismatch at x={}: {} vs {}",
                y,
                x,
                output_simd[x],
                output_scalar[x]
            );
        }
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
                "Uniform input should stay uniform at row {} x {}: {}",
                y,
                x,
                v
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

#[test]
fn convolve_2d_row_various_kernel_sizes() {
    let width = 32;
    let height = 32;
    let input: Vec<f32> = (0..width * height).map(|i| i as f32 * 0.01).collect();

    for ksize in [3, 5, 7, 9] {
        let weights: Vec<f32> = vec![1.0 / (ksize * ksize) as f32; ksize * ksize];
        let kernel = Kernel2d::new(&weights, ksize);

        for y in [0, height / 2, height - 1] {
            let mut output_simd = vec![0.0f32; width];
            let mut output_scalar = vec![0.0f32; width];

            convolve_2d_row(
                &input,
                &mut output_simd,
                Size2us::new(width, height),
                y,
                kernel,
            );
            convolve_2d_row_scalar(
                &input,
                &mut output_scalar,
                Size2us::new(width, height),
                y,
                kernel,
            );

            for x in 0..width {
                assert!(
                    (output_simd[x] - output_scalar[x]).abs() < 1e-4,
                    "Kernel size {}, row {}, x {}: {} vs {}",
                    ksize,
                    y,
                    x,
                    output_simd[x],
                    output_scalar[x]
                );
            }
        }
    }
}

#[test]
fn convolve_2d_row_boundary_handling() {
    let width = 8;
    let height = 8;
    let input: Vec<f32> = (0..width * height).map(|i| (i + 1) as f32).collect();

    let weights: Vec<f32> = vec![1.0 / 25.0; 25];
    let kernel = Kernel2d::new(&weights, 5);

    // Test boundary rows
    for y in [0, 1, height - 2, height - 1] {
        let mut output_simd = vec![0.0f32; width];
        let mut output_scalar = vec![0.0f32; width];

        convolve_2d_row(
            &input,
            &mut output_simd,
            Size2us::new(width, height),
            y,
            kernel,
        );
        convolve_2d_row_scalar(
            &input,
            &mut output_scalar,
            Size2us::new(width, height),
            y,
            kernel,
        );

        for x in 0..width {
            assert!(
                (output_simd[x] - output_scalar[x]).abs() < 1e-4,
                "Boundary row {}, x {}: {} vs {}",
                y,
                x,
                output_simd[x],
                output_scalar[x]
            );
            assert!(output_simd[x].is_finite(), "Output should be finite");
        }
    }
}
